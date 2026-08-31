//! GIF in and out.
//!
//! Existing GIFs are decoded with the `gif` crate rather than ffmpeg: ffmpeg
//! normalizes toward constant frame rate and discards per-frame delays and
//! disposal methods, which is exactly the data an editor has to preserve. The
//! same is true on the way out, which is why ffmpeg does not write the GIF
//! either.

use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use color_quant::NeuQuant;
use image::{Rgba, RgbaImage};

use crate::core::{Document, Frame};

pub fn decode_path(path: impl AsRef<Path>) -> Result<Vec<Frame>> {
    let path = path.as_ref();
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    decode(std::io::BufReader::new(file))
}

/// Decode to full-canvas RGBA frames, honoring disposal so each frame stands
/// alone in the document.
pub fn decode(reader: impl Read) -> Result<Vec<Frame>> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(reader).context("reading GIF header")?;

    let (w, h) = (decoder.width() as u32, decoder.height() as u32);
    let mut canvas = RgbaImage::new(w, h);
    let mut frames = Vec::new();

    while let Some(frame) = decoder.read_next_frame().context("reading GIF frame")? {
        let saved = matches!(frame.dispose, gif::DisposalMethod::Previous).then(|| canvas.clone());

        for y in 0..frame.height as u32 {
            for x in 0..frame.width as u32 {
                let (dx, dy) = (x + frame.left as u32, y + frame.top as u32);
                if dx >= w || dy >= h {
                    continue;
                }
                let i = ((y * frame.width as u32 + x) * 4) as usize;
                let px = &frame.buffer[i..i + 4];
                if px[3] == 0 {
                    continue; // transparent pixels leave what is underneath
                }
                canvas.put_pixel(dx, dy, Rgba([px[0], px[1], px[2], px[3]]));
            }
        }

        frames.push(Frame::new(canvas.clone(), frame.delay.max(1)));

        match frame.dispose {
            gif::DisposalMethod::Background => {
                for y in 0..frame.height as u32 {
                    for x in 0..frame.width as u32 {
                        let (dx, dy) = (x + frame.left as u32, y + frame.top as u32);
                        if dx < w && dy < h {
                            canvas.put_pixel(dx, dy, Rgba([0, 0, 0, 0]));
                        }
                    }
                }
            }
            gif::DisposalMethod::Previous => {
                if let Some(saved) = saved {
                    canvas = saved;
                }
            }
            _ => {}
        }
    }

    if frames.is_empty() {
        bail!("the file decoded to zero frames");
    }
    Ok(frames)
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExportSettings {
    /// Target width; height follows the source aspect. None keeps 100%.
    pub width: Option<u32>,
    /// 1.0 is the source speed. 2.0 halves every delay.
    pub speed: f32,
    pub colors: u16,
    pub dither: bool,
    /// None loops forever.
    pub loops: Option<u16>,
    /// gifsicle's `--lossy`; 0 turns it off.
    pub lossy: u16,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            width: None,
            speed: 1.0,
            colors: 256,
            dither: false,
            loops: None,
            lossy: 0,
        }
    }
}

/// Composited frames plus their delays — what the export path consumes, so the
/// size preview and the real export run the same code.
pub struct Encodable {
    pub frames: Vec<(RgbaImage, u16)>,
}

/// Frames a size estimate encodes. The encoder writes every frame full-canvas
/// against one global palette, so a frame's cost does not depend on its
/// neighbours and a spread of eight extrapolates well.
pub const ESTIMATE_SAMPLES: usize = 8;

/// Header, logical screen descriptor and the global palette, paid once for the
/// whole file. A 256-colour palette is 768 bytes; the rest is fixed-size
/// records. Only the extrapolation uses this, and only to avoid charging it
/// once per frame.
const HEADER_BYTES: usize = 800;

impl Encodable {
    pub fn from_document(
        doc: &Document,
        text: crate::core::render::TextRasterizer<'_>,
        settings: &ExportSettings,
    ) -> Self {
        Self::build(doc, text, settings, (0..doc.frames.len()).collect())
    }

    /// Up to `count` evenly-spaced frames. Spread rather than taken from the
    /// front: the front of a clip is rarely typical of the rest of it.
    pub fn sample_document(
        doc: &Document,
        text: crate::core::render::TextRasterizer<'_>,
        settings: &ExportSettings,
        count: usize,
    ) -> Self {
        let n = doc.frames.len();
        let count = count.clamp(1, n.max(1));
        let picked = (0..count).map(|i| (i * n + n / 2) / count.max(1)).collect();
        Self::build(doc, text, settings, picked)
    }

    fn build(
        doc: &Document,
        text: crate::core::render::TextRasterizer<'_>,
        settings: &ExportSettings,
        indices: Vec<usize>,
    ) -> Self {
        let (sw, sh) = doc.size();
        let scaled = settings.width.filter(|w| *w != sw).map(|w| {
            let h = ((w as f32 / sw.max(1) as f32) * sh as f32).round().max(1.0) as u32;
            (w, h)
        });

        let frames = indices
            .into_iter()
            .filter_map(|i| {
                let img = crate::core::render::composite(doc, i, text)?;
                let img = match scaled {
                    Some((w, h)) => {
                        image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3)
                    }
                    None => img,
                };
                let delay = (doc.frames[i].delay_cs as f32 / settings.speed).round();
                Some((img, delay.clamp(1.0, u16::MAX as f32) as u16))
            })
            .collect();
        Encodable { frames }
    }
}

/// Quantize against one global palette and write with exact per-frame delays.
pub fn encode(out: impl Write, enc: &Encodable, settings: &ExportSettings) -> Result<()> {
    let Some((first, _)) = enc.frames.first() else {
        bail!("nothing to export")
    };
    let (w, h) = first.dimensions();
    let transparent = enc
        .frames
        .iter()
        .any(|(f, _)| f.pixels().any(|p| p.0[3] < 128));

    let reserved = transparent as usize;
    let wanted = (settings.colors.clamp(2, 256) as usize)
        .saturating_sub(reserved)
        .max(2);
    let quant = NeuQuant::new(10, wanted, &sample(&enc.frames));

    let mut palette = quant.color_map_rgb();
    let color_count = palette.len() / 3;
    let transparent_index = transparent.then_some(color_count as u8);
    if transparent {
        palette.extend_from_slice(&[0, 0, 0]);
    }
    // GIF palettes are a power of two.
    let slots = (palette.len() / 3).next_power_of_two().max(2);
    palette.resize(slots * 3, 0);

    let mut encoder =
        gif::Encoder::new(out, w as u16, h as u16, &palette).context("writing GIF header")?;
    encoder.set_repeat(match settings.loops {
        None => gif::Repeat::Infinite,
        Some(n) => gif::Repeat::Finite(n),
    })?;

    for (img, delay) in &enc.frames {
        let mut frame = gif::Frame::default();
        frame.width = w as u16;
        frame.height = h as u16;
        frame.delay = *delay;
        frame.transparent = transparent_index;
        frame.buffer = index_frame(img, &quant, transparent_index, settings.dither).into();
        encoder.write_frame(&frame).context("writing GIF frame")?;
    }
    Ok(())
}

/// Up to 16 evenly-spaced frames, strided so the sample stays around a
/// megapixel. A global palette built from one frame flatters the preview and
/// then fails to match the export.
fn sample(frames: &[(RgbaImage, u16)]) -> Vec<u8> {
    let step = (frames.len() / 16).max(1);
    let picked: Vec<&RgbaImage> = frames.iter().step_by(step).map(|(f, _)| f).collect();
    let total: usize = picked.iter().map(|f| f.pixels().len()).sum();
    let stride = (total / 1_000_000).max(1);

    let mut out = Vec::with_capacity(total.min(1_000_000) * 4);
    for frame in picked {
        for px in frame.pixels().step_by(stride) {
            out.extend_from_slice(&px.0);
        }
    }
    if out.is_empty() {
        out.extend_from_slice(&[0, 0, 0, 255]);
    }
    out
}

fn index_frame(
    img: &RgbaImage,
    quant: &NeuQuant,
    transparent: Option<u8>,
    dither: bool,
) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let mut out = vec![0u8; (w * h) as usize];
    // Floyd-Steinberg error, one row of lookahead plus the current row.
    let mut error = vec![[0i16; 3]; (w as usize + 2) * 2];
    let palette = quant.color_map_rgb();

    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y).0;
            let i = (y * w + x) as usize;
            if px[3] < 128 {
                if let Some(t) = transparent {
                    out[i] = t;
                    continue;
                }
            }
            let e = if dither {
                error[x as usize + 1]
            } else {
                [0; 3]
            };
            let want = [
                (px[0] as i16 + e[0]).clamp(0, 255) as u8,
                (px[1] as i16 + e[1]).clamp(0, 255) as u8,
                (px[2] as i16 + e[2]).clamp(0, 255) as u8,
                255,
            ];
            let idx = quant.index_of(&want) as u8;
            out[i] = idx;

            if dither {
                let got = &palette[idx as usize * 3..idx as usize * 3 + 3];
                let diff = [
                    want[0] as i16 - got[0] as i16,
                    want[1] as i16 - got[1] as i16,
                    want[2] as i16 - got[2] as i16,
                ];
                let row = w as usize + 2;
                for (offset, weight) in [
                    (x as usize + 2, 7),
                    (x as usize + row, 3),
                    (x as usize + 1 + row, 5),
                    (x as usize + 2 + row, 1),
                ] {
                    if offset < error.len() {
                        for c in 0..3 {
                            error[offset][c] =
                                (error[offset][c] + diff[c] * weight / 16).clamp(-255, 255);
                        }
                    }
                }
            }
        }
        if dither {
            let row = w as usize + 2;
            error.copy_within(row.., 0);
            for slot in &mut error[row..] {
                *slot = [0; 3];
            }
        }
    }
    out
}

/// Inter-frame differencing and lossy compression, twenty years of it, as a
/// subprocess so its GPL-2 stays away from this code. Missing gifsicle is not
/// an error: the unoptimized file is still a valid GIF.
pub fn optimize(path: &Path, lossy: u16) -> Result<bool> {
    let mut cmd = Command::new("gifsicle");
    cmd.arg("-O3");
    if lossy > 0 {
        cmd.arg(format!("--lossy={lossy}"));
    }
    cmd.arg("-b").arg(path);

    match cmd.status() {
        Ok(status) if status.success() => Ok(true),
        Ok(status) => bail!("gifsicle exited with {status}"),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).context("running gifsicle"),
    }
}

pub fn export_path(path: &Path, enc: &Encodable, settings: &ExportSettings) -> Result<u64> {
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    encode(std::io::BufWriter::new(file), enc, settings)?;
    optimize(path, settings.lossy)?;
    Ok(std::fs::metadata(path)?.len())
}

/// Encoded size without touching the disk, for the export dialog's readout.
pub fn encoded_size(enc: &Encodable, settings: &ExportSettings) -> Result<usize> {
    let mut buf = Vec::new();
    encode(&mut buf, enc, settings)?;
    Ok(buf.len())
}

/// Size of the whole animation, extrapolated from an encoded sample. Real
/// encoder, real palette, real LZW — only the frame count is arithmetic, which
/// is arithmetic, which is why this is the only thing allowed to name a size.
pub fn estimate_size(
    sample: &Encodable,
    total_frames: usize,
    settings: &ExportSettings,
) -> Result<usize> {
    let sampled = sample.frames.len();
    if sampled == 0 {
        bail!("nothing to estimate");
    }
    let measured = encoded_size(sample, settings)?;
    if sampled >= total_frames {
        return Ok(measured);
    }
    let header = HEADER_BYTES.min(measured);
    let per_frame = (measured - header) as f64 / sampled as f64;
    Ok(header + (per_frame * total_frames as f64) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::render::no_text;

    fn flat(w: u32, h: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(color))
    }

    fn source() -> Document {
        Document::from_frames(vec![
            Frame::new(flat(8, 8, [200, 30, 30, 255]), 7),
            Frame::new(flat(8, 8, [30, 200, 30, 255]), 3),
            Frame::new(flat(8, 8, [30, 30, 200, 255]), 42),
        ])
    }

    #[test]
    fn round_trip_preserves_per_frame_delays() {
        let doc = source();
        let enc = Encodable::from_document(&doc, &no_text, &ExportSettings::default());
        let mut bytes = Vec::new();
        encode(&mut bytes, &enc, &ExportSettings::default()).unwrap();

        let decoded = decode(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(decoded.len(), 3);
        assert_eq!(
            decoded.iter().map(|f| f.delay_cs).collect::<Vec<_>>(),
            vec![7, 3, 42],
            "delays are the reason this app does not use ffmpeg for GIF"
        );
        // colors survive quantization on flat input
        assert_eq!(decoded[1].pixels.get_pixel(4, 4).0[1], 200);
    }

    #[test]
    fn speed_rescales_delays() {
        let doc = source();
        let settings = ExportSettings {
            speed: 2.0,
            ..Default::default()
        };
        let enc = Encodable::from_document(&doc, &no_text, &settings);
        assert_eq!(
            enc.frames.iter().map(|(_, d)| *d).collect::<Vec<_>>(),
            vec![4, 2, 21]
        );
    }

    #[test]
    fn resize_follows_the_source_aspect() {
        let doc = Document::from_frames(vec![Frame::new(flat(100, 50, [1, 2, 3, 255]), 5)]);
        let settings = ExportSettings {
            width: Some(40),
            ..Default::default()
        };
        let enc = Encodable::from_document(&doc, &no_text, &settings);
        assert_eq!(enc.frames[0].0.dimensions(), (40, 20));
    }

    #[test]
    fn transparency_survives_the_round_trip() {
        let mut img = flat(8, 8, [255, 0, 0, 255]);
        img.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        let doc = Document::from_frames(vec![Frame::new(img, 5)]);
        let enc = Encodable::from_document(&doc, &no_text, &ExportSettings::default());
        let mut bytes = Vec::new();
        encode(&mut bytes, &enc, &ExportSettings::default()).unwrap();
        let decoded = decode(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(decoded[0].pixels.get_pixel(0, 0).0[3], 0);
        assert_eq!(decoded[0].pixels.get_pixel(4, 4).0[3], 255);
    }

    /// A moving subject on a flat field: frames differ, so a sample taken from
    /// the front alone would misjudge the whole.
    fn moving_doc(frames: usize, w: u32, h: u32) -> Document {
        Document::from_frames(
            (0..frames)
                .map(|i| {
                    let mut img = flat(w, h, [20, 20, 40, 255]);
                    let x = (i as u32 * 3) % w.saturating_sub(8).max(1);
                    for dy in 0..h.min(8) {
                        for dx in 0..8u32.min(w) {
                            img.put_pixel(x + dx, dy, Rgba([240, 200, 40, 255]));
                        }
                    }
                    Frame::new(img, 5)
                })
                .collect(),
        )
    }

    /// The whole point of the slow estimate: within a few percent of the real
    /// encode, having encoded a fraction of the frames.
    #[test]
    fn the_sampled_estimate_tracks_a_real_encode() {
        let doc = moving_doc(120, 96, 64);
        let settings = ExportSettings::default();

        let whole = Encodable::from_document(&doc, &no_text, &settings);
        let actual = encoded_size(&whole, &settings).unwrap();

        let sample = Encodable::sample_document(&doc, &no_text, &settings, ESTIMATE_SAMPLES);
        assert_eq!(sample.frames.len(), ESTIMATE_SAMPLES, "encodes a fraction");
        let estimate = estimate_size(&sample, doc.frames.len(), &settings).unwrap();

        let error = (estimate as f64 / actual as f64 - 1.0).abs();
        assert!(
            error < 0.10,
            "estimate {estimate} vs actual {actual} ({:.1}%)",
            error * 100.0
        );
    }

    #[test]
    fn the_sample_is_spread_across_the_document_not_taken_from_the_front() {
        let doc = moving_doc(80, 32, 32);
        let sample = Encodable::sample_document(&doc, &no_text, &ExportSettings::default(), 8);
        let heads = Encodable::build(&doc, &no_text, &ExportSettings::default(), (0..8).collect());
        assert_ne!(
            sample.frames[7].0.as_raw(),
            heads.frames[7].0.as_raw(),
            "the last sample is not the eighth frame"
        );
    }

    /// Asking for more samples than there are frames just encodes the document,
    /// and then there is nothing to extrapolate.
    #[test]
    fn a_short_document_is_measured_rather_than_extrapolated() {
        let doc = moving_doc(4, 32, 32);
        let settings = ExportSettings::default();
        let sample = Encodable::sample_document(&doc, &no_text, &settings, ESTIMATE_SAMPLES);
        assert_eq!(sample.frames.len(), 4);
        assert_eq!(
            estimate_size(&sample, 4, &settings).unwrap(),
            encoded_size(&sample, &settings).unwrap(),
            "no guesswork left in it"
        );
    }

    #[test]
    fn the_estimate_scales_with_the_frame_count() {
        let doc = moving_doc(40, 32, 32);
        let settings = ExportSettings::default();
        let sample = Encodable::sample_document(&doc, &no_text, &settings, 4);
        let (short, long) = (
            estimate_size(&sample, 100, &settings).unwrap(),
            estimate_size(&sample, 400, &settings).unwrap(),
        );
        // Four times the frames, minus the header that is only paid once.
        assert!(long > short * 3 && long < short * 4, "{short} -> {long}");
    }

    #[test]
    fn fewer_colors_makes_a_smaller_file() {
        let mut img = RgbaImage::new(64, 64);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgba([(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8, 255]);
        }
        let doc = Document::from_frames(vec![Frame::new(img, 5)]);
        let big = ExportSettings {
            colors: 256,
            ..Default::default()
        };
        let small = ExportSettings {
            colors: 16,
            ..Default::default()
        };
        let enc = Encodable::from_document(&doc, &no_text, &big);
        assert!(encoded_size(&enc, &small).unwrap() < encoded_size(&enc, &big).unwrap());
    }
}
