pub mod caps;
pub mod gif;
pub mod video;

use std::path::Path;

use anyhow::{Context, Result};

use crate::core::Frame;

/// One entry point for everything openable. Existing GIFs go through the `gif`
/// crate, still images through `image`, everything else through ffmpeg — so a
/// PNG splices in on a build with no ffmpeg at all.
pub fn import_any(
    path: &Path,
    options: &video::ImportOptions,
    progress: &mut dyn FnMut(usize, Option<usize>) -> bool,
) -> Result<Vec<Frame>> {
    if has_extension(path, "gif") {
        return gif::decode_path(path, progress);
    }
    if is_still_image(path) {
        let frame = decode_still(path)?;
        progress(1, Some(1));
        return Ok(vec![frame]);
    }
    video::import(path, options, progress)
}

/// A single still image as one frame. `delay_cs` is what a frame appended to
/// an empty document gets; a splice re-delays it from its neighbour.
pub fn decode_still(path: &Path) -> Result<Frame> {
    let pixels = image::open(path)
        .with_context(|| format!("decoding {}", path.display()))?
        .to_rgba8();
    Ok(Frame::new(pixels, STILL_DELAY_CS))
}

/// A still spliced into an empty document has no neighbour to take a delay
/// from: 10cs is the same default the frame list uses elsewhere.
const STILL_DELAY_CS: u16 = 10;

/// What `import_any` decodes with the `image` crate as one frame rather than
/// handing to ffmpeg. Formats that can animate — GIF, WebP, APNG — are not in
/// here: they go to a decoder that can see every frame.
pub fn is_still_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "bmp"
                | "tif"
                | "tiff"
                | "tga"
                | "ico"
                | "qoi"
                | "ppm"
                | "pgm"
                | "pbm"
                | "pnm"
        )
    })
}

pub fn has_extension(path: &Path, want: &str) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(want))
}

/// Measured GIF size for a file that has not been imported yet: sample it by
/// seeking, encode the sample for real, extrapolate to the plan's frame count.
/// Costs a few seconds and several ffmpeg spawns, so it belongs behind a button
/// and off the main thread.
pub fn estimate_gif_size(path: &Path, plan: &video::ImportPlan) -> Result<usize> {
    let samples = video::sample_frames(path, plan, gif::ESTIMATE_SAMPLES)?;
    let delay = (100.0 / plan.fps.max(1.0))
        .round()
        .clamp(1.0, u16::MAX as f64) as u16;
    let enc = gif::Encodable {
        frames: samples.into_iter().map(|img| (img, delay)).collect(),
    };
    let total = plan.frames().unwrap_or(plan.cap);
    gif::estimate_size(&enc, total, &gif::ExportSettings::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the `image` dependency was built with no format features,
    /// so every still image failed to decode and "Add frame from image" could
    /// never add one. The formats `is_still_image` claims have to be formats
    /// that are actually compiled in.
    #[test]
    fn every_still_format_this_claims_actually_decodes() {
        // RGB rather than RGBA: JPEG has no alpha channel to encode, and what
        // is under test is the decoder being compiled in at all.
        let pixels = image::RgbImage::from_pixel(6, 4, image::Rgb([10, 20, 30]));
        for extension in ["png", "jpg", "bmp", "tiff", "tga", "qoi", "ppm"] {
            let path = std::env::temp_dir().join(format!("gifkino-still-test.{extension}"));
            pixels
                .save(&path)
                .unwrap_or_else(|e| panic!("encoding {extension}: {e}"));
            assert!(is_still_image(&path), "{extension} is claimed as a still");
            let frame = decode_still(&path).unwrap_or_else(|e| panic!("{extension}: {e}"));
            assert_eq!(frame.pixels.dimensions(), (6, 4), "{extension}");
            let _ = std::fs::remove_file(&path);
        }
    }

    /// A still image reaches the timeline through the same entry point as a
    /// GIF or a video, so a splice does not need a second decode path.
    #[test]
    fn import_any_reads_a_still_image_as_one_frame() {
        let path = std::env::temp_dir().join("gifkino-import-any-test.png");
        image::RgbaImage::from_pixel(9, 3, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .expect("encoding a png");
        let mut reported = Vec::new();
        let frames = import_any(
            &path,
            &video::ImportOptions::default(),
            &mut |done, total| {
                reported.push((done, total));
                true
            },
        )
        .expect("decoding a png");
        let _ = std::fs::remove_file(&path);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].pixels.dimensions(), (9, 3));
        assert_eq!(reported, vec![(1, Some(1))], "one frame, and it is known");
    }

    /// End to end over real files, the way "Add frames from file" runs: decode
    /// a GIF, decode a PNG that is a different size and shape, and splice the
    /// two together under each fit. The canvas has to stay uniform whichever
    /// one is picked, or every op that reads `Document::size()` off the first
    /// frame is reading a lie.
    #[test]
    fn a_png_splices_into_a_decoded_gif_under_every_fit() {
        use crate::core::{Document, FitMode, fit};

        let gif_path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-8-frames.gif");
        let frames = import_any(&gif_path, &video::ImportOptions::default(), &mut |_, _| {
            true
        })
        .expect("decoding the fixture gif");
        let document = Document::from_frames(frames);
        assert_eq!(document.size(), (160, 120), "the fixture's canvas");

        let still = std::env::temp_dir().join("gifkino-splice-test.png");
        image::RgbImage::from_pixel(64, 96, image::Rgb([200, 30, 30]))
            .save(&still)
            .expect("encoding a png");

        for mode in FitMode::ALL {
            let incoming = import_any(&still, &video::ImportOptions::default(), &mut |_, _| true)
                .expect("decoding the png");
            let mut spliced = document.clone();
            let at = spliced.frames.len();
            let added =
                fit::plan_splice(&spliced, at, incoming, mode, |_, _| {}).apply(&mut spliced);
            assert_eq!(added, 1, "{mode:?} added the one frame");
            assert_eq!(spliced.frames.len(), 9, "{mode:?}");
            let canvas = spliced.size();
            let expected = if mode.grows_canvas() {
                (64, 96)
            } else {
                (160, 120)
            };
            assert_eq!(canvas, expected, "{mode:?} canvas");
            for (i, frame) in spliced.frames.iter().enumerate() {
                assert_eq!(
                    frame.pixels.dimensions(),
                    canvas,
                    "{mode:?} left frame {i} off the canvas"
                );
            }
        }
        let _ = std::fs::remove_file(&still);
    }
}
