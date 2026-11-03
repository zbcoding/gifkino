//! GIF files: the byte codec in `gifkino_core::gif` plus what only the desktop
//! has — a filesystem, and gifsicle to run over the result.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use gifkino_core::Frame;
use gifkino_core::gif::{Encodable, ExportSettings, decode, encode};

pub fn decode_path(
    path: impl AsRef<Path>,
    progress: &mut dyn FnMut(usize, Option<usize>) -> bool,
) -> Result<Vec<Frame>> {
    let path = path.as_ref();
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    decode(std::io::BufReader::new(file), progress)
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

#[cfg(test)]
mod tests {
    use super::*;
    use gifkino_core::Document;
    use gifkino_core::render::no_text;
    use image::{Rgba, RgbaImage};

    fn decoded(bytes: Vec<u8>) -> Vec<Frame> {
        decode(std::io::Cursor::new(bytes), &mut |_, _| true).unwrap()
    }

    fn temp_gif(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("gifkino-{name}-{}.gif", std::process::id()))
    }

    /// A moving subject on a flat field, so the frames differ.
    fn moving_doc(frames: usize, w: u32, h: u32) -> Document {
        Document::from_frames(
            (0..frames)
                .map(|i| {
                    let mut img = RgbaImage::from_pixel(w, h, Rgba([20, 20, 40, 255]));
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
