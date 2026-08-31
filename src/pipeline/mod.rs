pub mod caps;
pub mod gif;
pub mod video;

use std::path::Path;

use anyhow::Result;

use crate::core::Frame;

/// One entry point for everything openable. Existing GIFs go through the `gif`
/// crate; everything else goes through ffmpeg.
pub fn import_any(
    path: &Path,
    options: &video::ImportOptions,
    progress: &mut dyn FnMut(usize, Option<usize>) -> bool,
) -> Result<Vec<Frame>> {
    let is_gif = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    if is_gif {
        gif::decode_path(path)
    } else {
        video::import(path, options, progress)
    }
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
