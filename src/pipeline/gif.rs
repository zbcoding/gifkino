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

pub fn decode_path(
    path: impl AsRef<Path>,
    progress: &mut dyn FnMut(usize, Option<usize>) -> bool,
) -> Result<Vec<Frame>> {
    let path = path.as_ref();
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    decode(std::io::BufReader::new(file), progress)
}

/// Decode to full-canvas RGBA frames, honoring disposal so each frame stands
/// alone in the document. `progress` gets the frame count as it grows; a GIF
/// header carries no frame total, so there is nothing to estimate with, and
/// returning false stops the decode where it stands.
pub fn decode(
    reader: impl Read,
    progress: &mut dyn FnMut(usize, Option<usize>) -> bool,
) -> Result<Vec<Frame>> {
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
        if !progress(frames.len(), None) {
            break;
        }
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
/// `progress` runs after every written frame with the 1-based count and the
/// frame total, so the export bar reads in frames like every other job's.
pub fn encode(
    out: impl Write,
    enc: &Encodable,
    settings: &ExportSettings,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<()> {
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

    let total = enc.frames.len();
    for (i, (img, delay)) in enc.frames.iter().enumerate() {
        let frame = gif::Frame {
            width: w as u16,
            height: h as u16,
            delay: *delay,
            transparent: transparent_index,
            buffer: index_frame(img, &quant, transparent_index, settings.dither).into(),
            ..Default::default()
        };
        encoder.write_frame(&frame).context("writing GIF frame")?;
        progress(i + 1, total);
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
            if px[3] < 128
                && let Some(t) = transparent
            {
                out[i] = t;
                continue;
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

pub fn export_path(
    path: &Path,
    enc: &Encodable,
    settings: &ExportSettings,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<u64> {
    let file =
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    encode(std::io::BufWriter::new(file), enc, settings, progress)?;
    optimize(path, settings.lossy)?;
    Ok(std::fs::metadata(path)?.len())
}

/// Encoded size without touching the disk, for the export dialog's readout.
pub fn encoded_size(enc: &Encodable, settings: &ExportSettings) -> Result<usize> {
    let mut buf = Vec::new();
    // The size preview has its own "sizing…" readout; it stays off the bar.
    encode(&mut buf, enc, settings, &mut |_, _| {})?;
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
    fn encode_reports_each_frame_with_its_total() {
        // The export bar's data source: one call per written frame, 1-based,
        // with the frame count as the total. A bar wired to anything else
        // would finish early or never reach full.
        for (frames, delays) in [(1, vec![7]), (3, vec![7, 3, 42])] {
            let doc = Document::from_frames(
                delays
                    .into_iter()
                    .enumerate()
                    .map(|(i, delay)| Frame::new(flat(8, 8, [i as u8 * 40, 30, 30, 255]), delay))
                    .collect(),
            );
            let enc = Encodable::from_document(&doc, &no_text, &ExportSettings::default());
            let mut seen = Vec::new();
            let mut bytes = Vec::new();
            encode(
                &mut bytes,
                &enc,
                &ExportSettings::default(),
                &mut |done, total| seen.push((done, total)),
            )
            .unwrap();
            let want: Vec<(usize, usize)> = (1..=frames).map(|done| (done, frames)).collect();
            assert_eq!(seen, want, "{frames} frames");
        }
    }

    #[test]
    fn round_trip_preserves_per_frame_delays() {
        let doc = source();
        let enc = Encodable::from_document(&doc, &no_text, &ExportSettings::default());
        let mut bytes = Vec::new();
        encode(&mut bytes, &enc, &ExportSettings::default(), &mut |_, _| {}).unwrap();

        let decoded = decode(&mut std::io::Cursor::new(bytes), &mut |_, _| true).unwrap();
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
        encode(&mut bytes, &enc, &ExportSettings::default(), &mut |_, _| {}).unwrap();
        let decoded = decode(&mut std::io::Cursor::new(bytes), &mut |_, _| true).unwrap();
        assert_eq!(decoded[0].pixels.get_pixel(0, 0).0[3], 0);
        assert_eq!(decoded[0].pixels.get_pixel(4, 4).0[3], 255);
    }

    /// The progress callback is the X beside the import bar. A GIF decode has
    /// no ffmpeg child to kill, so returning false is the only thing that
    /// stops it, and it has to stop between frames, not after the file.
    #[test]
    fn returning_false_from_progress_stops_the_decode() {
        let enc = Encodable::from_document(&source(), &no_text, &ExportSettings::default());
        let mut bytes = Vec::new();
        encode(&mut bytes, &enc, &ExportSettings::default(), &mut |_, _| {}).unwrap();

        let mut seen = Vec::new();
        let decoded = decode(&mut std::io::Cursor::new(bytes), &mut |done, expected| {
            seen.push((done, expected));
            done < 2
        })
        .unwrap();
        assert_eq!(decoded.len(), 2, "stopped where it was told to");
        assert_eq!(
            seen.iter().map(|(done, _)| *done).collect::<Vec<_>>(),
            vec![1, 2],
            "one report per decoded frame"
        );
        assert!(
            seen.iter().all(|(_, expected)| expected.is_none()),
            "a GIF header carries no frame count to promise"
        );
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

    /// Palette for the hand-built GIFs below: red, green, blue, and a black
    /// slot the frames never draw with.
    const PALETTE: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0];
    const RED: [u8; 4] = [255, 0, 0, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// A 4x4 GIF written straight with the `gif` crate, so the frames can use
    /// offsets and disposal methods this app's own encoder never writes.
    fn handmade(frames: &[gif::Frame<'_>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = gif::Encoder::new(&mut bytes, 4, 4, &PALETTE).unwrap();
            for frame in frames {
                encoder.write_frame(frame).unwrap();
            }
        }
        bytes
    }

    /// A `w`x`h` patch of one palette colour at (`left`, `top`).
    fn patch(
        left: u16,
        top: u16,
        w: u16,
        h: u16,
        index: u8,
        dispose: gif::DisposalMethod,
    ) -> gif::Frame<'static> {
        gif::Frame {
            left,
            top,
            width: w,
            height: h,
            delay: 5,
            dispose,
            buffer: vec![index; w as usize * h as usize].into(),
            ..Default::default()
        }
    }

    fn decoded(bytes: Vec<u8>) -> Vec<Frame> {
        decode(std::io::Cursor::new(bytes), &mut |_, _| true).unwrap()
    }

    /// Red background, then a green patch in the corner disposed with
    /// `dispose`, then a blue pixel elsewhere: the third frame shows what the
    /// disposal left under the patch.
    fn after_disposing_a_patch(dispose: gif::DisposalMethod) -> Vec<Frame> {
        use gif::DisposalMethod::Keep;
        decoded(handmade(&[
            patch(0, 0, 4, 4, 0, Keep),
            patch(0, 0, 2, 2, 1, dispose),
            patch(3, 3, 1, 1, 2, Keep),
        ]))
    }

    /// "Restore to previous" puts back the canvas as it was before the patch
    /// was drawn — not after, and not cleared — while the frame that carried
    /// the patch still shows it.
    #[test]
    fn restore_to_previous_disposal_brings_back_the_canvas_under_the_frame() {
        let frames = after_disposing_a_patch(gif::DisposalMethod::Previous);
        assert_eq!(frames[1].pixels.get_pixel(0, 0).0, GREEN, "the patch shows");
        assert_eq!(frames[1].pixels.get_pixel(3, 3).0, RED);
        assert_eq!(
            frames[2].pixels.get_pixel(0, 0).0,
            RED,
            "restored under the patch"
        );
        assert_eq!(frames[2].pixels.get_pixel(1, 1).0, RED);
        assert_eq!(
            frames[2].pixels.get_pixel(3, 3).0,
            BLUE,
            "the next frame still draws"
        );
    }

    /// "Restore to background" clears the patch's rectangle to transparent
    /// and leaves the rest of the canvas as it was.
    #[test]
    fn background_disposal_clears_only_the_frames_rectangle() {
        let frames = after_disposing_a_patch(gif::DisposalMethod::Background);
        assert_eq!(frames[1].pixels.get_pixel(0, 0).0, GREEN, "the patch shows");
        assert_eq!(
            frames[2].pixels.get_pixel(0, 0).0[3],
            0,
            "cleared under the patch"
        );
        assert_eq!(frames[2].pixels.get_pixel(1, 1).0[3], 0);
        assert_eq!(
            frames[2].pixels.get_pixel(2, 2).0,
            RED,
            "outside it untouched"
        );
        assert_eq!(frames[2].pixels.get_pixel(3, 3).0, BLUE);
    }

    /// A frame that hangs off the logical screen is legal enough that other
    /// encoders write it: the part inside the canvas is drawn, the rest is
    /// dropped, and the canvas keeps its size.
    #[test]
    fn a_frame_hanging_off_the_canvas_draws_only_what_is_inside() {
        use gif::DisposalMethod::Keep;
        let frames = decoded(handmade(&[
            patch(0, 0, 4, 4, 0, Keep),
            patch(3, 3, 3, 3, 2, Keep),
        ]));
        assert_eq!(frames[1].pixels.dimensions(), (4, 4));
        assert_eq!(frames[1].pixels.get_pixel(3, 3).0, BLUE);
        assert_eq!(frames[1].pixels.get_pixel(2, 3).0, RED);
        assert_eq!(frames[1].pixels.get_pixel(3, 2).0, RED);
    }

    /// A document with no frames has no canvas size, and every op reads one:
    /// an empty GIF is refused at the door.
    #[test]
    fn a_gif_with_no_frames_is_an_error_not_an_empty_document() {
        let bytes = handmade(&[]);
        assert!(decode(std::io::Cursor::new(bytes), &mut |_, _| true).is_err());
    }

    /// Nothing to encode is an error on both the export and the estimate,
    /// rather than a header-only file or an extrapolation from nothing.
    #[test]
    fn an_empty_export_is_refused() {
        let empty = Encodable { frames: Vec::new() };
        let settings = ExportSettings::default();
        assert!(encode(Vec::new(), &empty, &settings, &mut |_, _| {}).is_err());
        assert!(estimate_size(&empty, 10, &settings).is_err());
    }

    /// The loop count setting reaches the file: a finite count is written as
    /// that count, and no count loops forever.
    #[test]
    fn the_loop_count_is_written_into_the_file() {
        let enc = Encodable::from_document(&source(), &no_text, &ExportSettings::default());
        for (loops, want) in [
            (Some(3), gif::Repeat::Finite(3)),
            (None, gif::Repeat::Infinite),
        ] {
            let settings = ExportSettings {
                loops,
                ..Default::default()
            };
            let mut bytes = Vec::new();
            encode(&mut bytes, &enc, &settings, &mut |_, _| {}).unwrap();
            let mut decoder = gif::DecodeOptions::new()
                .read_info(std::io::Cursor::new(bytes))
                .unwrap();
            while decoder.read_next_frame().unwrap().is_some() {}
            assert_eq!(decoder.repeat(), want, "{loops:?}");
        }
    }

    /// Dithering trades per-pixel accuracy for area accuracy: with too few
    /// colours for a gradient, each column's average stays close to the
    /// source instead of snapping to the nearest palette entry.
    #[test]
    fn dithering_keeps_a_gradient_closer_on_average() {
        let (w, h) = (64u32, 32u32);
        let img = RgbaImage::from_fn(w, h, |x, _| {
            let v = (x * 4) as u8;
            Rgba([v, v, v, 255])
        });
        let doc = Document::from_frames(vec![Frame::new(img.clone(), 5)]);
        let column_error = |dither: bool| {
            let settings = ExportSettings {
                colors: 4,
                dither,
                ..Default::default()
            };
            let enc = Encodable::from_document(&doc, &no_text, &settings);
            let mut bytes = Vec::new();
            encode(&mut bytes, &enc, &settings, &mut |_, _| {}).unwrap();
            let out = &decoded(bytes)[0].pixels;
            (0..w)
                .map(|x| {
                    let mean =
                        (0..h).map(|y| out.get_pixel(x, y).0[0] as f64).sum::<f64>() / h as f64;
                    (mean - img.get_pixel(x, 0).0[0] as f64).abs()
                })
                .sum::<f64>()
                / w as f64
        };
        let (plain, dithered) = (column_error(false), column_error(true));
        assert!(
            dithered < plain / 2.0,
            "dithered {dithered:.1} vs plain {plain:.1} mean column error"
        );
    }

    fn temp_gif(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("gifkino-{name}-{}.gif", std::process::id()))
    }

    /// The file on disk is the export, optimized or not: the size reported is
    /// the size written, and gifsicle's inter-frame differencing (when it is
    /// installed) decodes back to exactly the frames that went in.
    #[test]
    fn an_exported_file_decodes_to_the_frames_that_went_in() {
        let doc = moving_doc(6, 32, 16);
        let settings = ExportSettings::default();
        let enc = Encodable::from_document(&doc, &no_text, &settings);
        let mut plain = Vec::new();
        encode(&mut plain, &enc, &settings, &mut |_, _| {}).unwrap();
        let want = decoded(plain);

        let path = temp_gif("export-path");
        let size = export_path(&path, &enc, &settings, &mut |_, _| {}).unwrap();
        let written = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(size, written.len() as u64, "the size reported is the file");
        let got = decoded(written);
        assert_eq!(got.len(), want.len());
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(g.delay_cs, w.delay_cs, "frame {i} delay");
            assert_eq!(g.pixels.as_raw(), w.pixels.as_raw(), "frame {i} pixels");
        }
    }

    /// Missing gifsicle is not an error, but a gifsicle that ran and failed
    /// is: the export must not claim an optimization that never happened.
    #[test]
    fn a_failing_gifsicle_is_an_error() {
        if !crate::pipeline::caps::Caps::probe().gifsicle {
            eprintln!("skipping: no gifsicle");
            return;
        }
        let path = temp_gif("not-a-gif");
        std::fs::write(&path, b"this is not a GIF").unwrap();
        let result = optimize(&path, 0);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err(), "{result:?}");
    }

    /// The lossy setting reaches gifsicle: on noise, where LZW finds nothing
    /// to repeat, letting it approximate makes the file smaller.
    #[test]
    fn a_lossy_export_is_smaller() {
        if !crate::pipeline::caps::Caps::probe().gifsicle {
            eprintln!("skipping: no gifsicle");
            return;
        }
        let mut seed = 0x2545_f491_u32;
        let img = RgbaImage::from_fn(64, 64, |_, _| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let [r, g, b, _] = seed.to_le_bytes();
            Rgba([r, g, b, 255])
        });
        let doc = Document::from_frames(vec![Frame::new(img, 5)]);
        let size = |lossy: u16| {
            let settings = ExportSettings {
                lossy,
                ..Default::default()
            };
            let enc = Encodable::from_document(&doc, &no_text, &settings);
            let path = temp_gif(&format!("lossy-{lossy}"));
            let size = export_path(&path, &enc, &settings, &mut |_, _| {}).unwrap();
            let _ = std::fs::remove_file(&path);
            size
        };
        let (exact, lossy) = (size(0), size(80));
        assert!(lossy < exact, "lossy {lossy} vs exact {exact}");
    }
}
