//! Video import: ffmpeg decodes straight to raw RGBA over a pipe. No temp PNG
//! sequence, and the same path serves recordings and any other video.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use image::RgbaImage;

use crate::core::Frame;
use crate::core::ops::delays_for_fps;

#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration_s: Option<f64>,
}

impl VideoInfo {
    /// Frames to expect at `fps`, for the progress page. An estimate:
    /// containers lie.
    pub fn estimated_frames(&self, fps: f64) -> Option<usize> {
        self.duration_s.map(|d| (d * fps).round().max(1.0) as usize)
    }
}

pub fn probe(path: &Path) -> Result<VideoInfo> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .arg("stream=width,height,r_frame_rate:format=duration")
        .args(["-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path)
        .output()
        .context("running ffprobe")?;
    if !out.status.success() {
        bail!("ffprobe could not read {}", path.display());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let width = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    let height = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .unwrap_or(0);
    let fps = lines.next().map(parse_rate).unwrap_or(0.0);
    let duration_s = lines.next().and_then(|l| l.trim().parse::<f64>().ok());

    if width == 0 || height == 0 {
        bail!("{} has no video stream", path.display());
    }
    Ok(VideoInfo {
        width,
        height,
        fps: if fps > 0.0 { fps } else { 25.0 },
        duration_s,
    })
}

fn parse_rate(text: &str) -> f64 {
    match text.trim().split_once('/') {
        Some((n, d)) => {
            let (n, d) = (
                n.parse::<f64>().unwrap_or(0.0),
                d.parse::<f64>().unwrap_or(1.0),
            );
            if d == 0.0 { 0.0 } else { n / d }
        }
        None => text.trim().parse().unwrap_or(0.0),
    }
}

/// What part of the input to take, before it is scaled or thinned. Two ways
/// of saying it, because a clip is scrubbed in seconds while the GIF it is
/// becoming is counted in frames. A frame index counts at the source's own
/// rate, and both ends are inclusive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trim {
    Seconds { start: f64, end: Option<f64> },
    Frames { first: usize, last: Option<usize> },
}

impl Trim {
    /// Where the decode starts, and how many seconds it takes, resolved
    /// against `source`. `None` for the span means "to the end", which is all
    /// a container with no duration can offer.
    pub fn resolve(&self, source: &VideoInfo) -> (f64, Option<f64>) {
        let rate = if source.fps > 0.0 { source.fps } else { 25.0 };
        let (start, end) = match *self {
            Trim::Seconds { start, end } => (start.max(0.0), end),
            // Inclusive: frames 234 to 1234 is 1001 frames, so the end is the
            // moment frame 1235 would have started.
            Trim::Frames { first, last } => {
                (first as f64 / rate, last.map(|l| (l + 1) as f64 / rate))
            }
        };
        let clamp = |s: f64| match source.duration_s {
            Some(d) => s.clamp(0.0, d),
            None => s.max(0.0),
        };
        let start = clamp(start);
        (start, end.map(|e| (clamp(e) - start).max(0.0)))
    }
}

/// Frames left after dropping one in every `n`, counted the way
/// `Document::drop_every_nth` drops them: frame 0 always stays and every Nth
/// goes. Fewer than two keeps everything.
pub fn kept_frames(decoded: usize, drop_nth: Option<usize>) -> usize {
    match drop_nth.filter(|n| *n >= 2) {
        Some(n) => decoded - decoded / n,
        None => decoded,
    }
}

#[derive(Clone, Debug)]
pub struct ImportOptions {
    /// Frames live in RAM as RGBA so scrubbing is instant; 1920x1080 across 300
    /// frames is 2.5 GB, so cap the long edge on the way in.
    pub max_width: Option<u32>,
    /// Explicit output size, what the import dialog's resolution picker sets.
    /// Overrides `max_width` and is free to change the aspect ratio.
    pub target: Option<(u32, u32)>,
    /// Resample on import. Anything above 50 is fiction in a GIF.
    pub fps: Option<f64>,
    /// Take only this span of the input, leaving the rest undecoded.
    pub trim: Option<Trim>,
    /// Drop one frame in every N on the way in, the import-time twin of the
    /// Optimize menu's own drop. Under two keeps every frame.
    pub drop_nth: Option<usize>,
    /// Ceiling on the decoded pixel data. Without it a two-minute 1080p clip
    /// asks for tens of gigabytes and the machine spends the import swapping.
    pub max_bytes: usize,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions {
            max_width: Some(1280),
            target: None,
            fps: None,
            trim: None,
            drop_nth: None,
            max_bytes: 1_200 << 20,
        }
    }
}

/// What an import would do to a file, settled from the probe alone so the UI
/// can warn before committing to a decode that takes minutes.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportPlan {
    pub source: VideoInfo,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// Where in the input the decode starts, and how long it runs — the trim,
    /// resolved against the source. A span of `None` runs to the end.
    pub start_s: f64,
    pub span_s: Option<f64>,
    /// One frame in every N is dropped as the frames come off the decoder,
    /// which is why it counts against the budget as the smaller number.
    pub drop_nth: Option<usize>,
    /// Frames the memory budget buys at this size, which is the only ceiling
    /// an import has: a clip is refused when it wants more frames than this,
    /// and the decode stops here when the container named no duration to plan
    /// against. A frame count on its own is not a cost — 4000 frames of
    /// 64x64 is 65 MB — so nothing counts frames except through their bytes.
    pub cap: usize,
}

impl ImportPlan {
    /// Frames this plan lands: the trimmed span at the planned rate, less the
    /// ones the drop takes out. Not clamped to `cap` — an over-budget plan is
    /// refused, not trimmed, so the honest count is the one the settings ask
    /// for. `None` when the container admits no duration and the trim named no
    /// end, leaving nothing to count.
    pub fn frames(&self) -> Option<usize> {
        Some(kept_frames(self.decoded_frames()?, self.drop_nth))
    }

    /// Frames the decoder hands over before the drop takes any out. What
    /// `-frames:v` and the progress bar count.
    pub fn decoded_frames(&self) -> Option<usize> {
        let seconds = match self.span_s {
            Some(span) => span,
            None => (self.source.duration_s? - self.start_s).max(0.0),
        };
        Some((seconds * self.fps).round().max(1.0) as usize)
    }

    pub fn bytes(&self) -> Option<usize> {
        self.frames()
            .map(|n| n * self.width as usize * self.height as usize * 4)
    }

    /// These settings need more memory than the budget allows. Import is
    /// refused until the size, the rate, the span or the drop brings it down:
    /// quietly cutting the end off someone's clip is not a decision to make on
    /// their behalf.
    pub fn over_budget(&self) -> bool {
        self.frames().is_some_and(|n| n > self.cap)
    }

    /// The rate the imported frames play at: the decode's rate less what the
    /// drop takes out, which is what keeps a thinned import the same length as
    /// the clip it came from.
    pub fn playback_fps(&self) -> f64 {
        match self.drop_nth {
            Some(n) if n >= 2 => self.fps * (n - 1) as f64 / n as f64,
            _ => self.fps,
        }
    }
}

pub fn plan(path: &Path, options: &ImportOptions) -> Result<ImportPlan> {
    Ok(plan_for(probe(path)?, options))
}

/// Sizes the resolution picker offers, largest first: the standard heights that
/// fit inside the source, at the source's aspect ratio.
pub fn size_presets(source: &VideoInfo) -> Vec<(u32, u32)> {
    [720u32, 540, 480, 360, 270]
        .iter()
        .filter(|h| **h < source.height)
        .map(|h| {
            (
                even(source.width as f64 * *h as f64 / source.height.max(1) as f64),
                *h,
            )
        })
        .collect()
}

/// ffmpeg's scalers and every encoder downstream want even dimensions.
fn even(value: f64) -> u32 {
    (value.round().max(2.0) as u32) & !1
}

/// Over budget, thin the frame rate rather than cutting the clip short: the
/// whole video at a lower rate is closer to what a GIF wants anyway.
pub fn plan_for(source: VideoInfo, options: &ImportOptions) -> ImportPlan {
    let (mut width, mut height) = match options.target {
        Some((w, h)) => (even(w as f64), even(h as f64)),
        None => (source.width, source.height),
    };
    if options.target.is_none()
        && let Some(max) = options.max_width.filter(|m| width > *m)
    {
        height = even((max as f64 / width as f64) * height as f64);
        width = max;
    }
    let frame_bytes = (width as usize * height as usize * 4).max(1);
    let cap = (options.max_bytes / frame_bytes).max(1);
    let (start_s, span_s) = match options.trim {
        Some(trim) => trim.resolve(&source),
        None => (0.0, None),
    };
    let drop_nth = options.drop_nth.filter(|n| *n >= 2);
    // Seconds the automatic rate has to fit its frames into: the trimmed span
    // when there is one, the rest of the clip otherwise.
    let planned_seconds = span_s.or_else(|| source.duration_s.map(|d| (d - start_s).max(0.0)));

    // An explicit rate is the user's call: honour it and let `cap` truncate,
    // which the preview says out loud. Only the automatic rate gets thinned to
    // fit, because there is nobody to ask. The drop is counted in: it is what
    // decides how many of the decoded frames are kept.
    let mut fps = options.fps.unwrap_or(source.fps);
    if options.fps.is_none()
        && let Some(seconds) = planned_seconds.filter(|d| *d > 0.5)
    {
        let affordable = match drop_nth {
            Some(n) => cap as f64 * n as f64 / (n - 1) as f64,
            None => cap as f64,
        };
        fps = fps.min((affordable / seconds).max(2.0));
    }
    ImportPlan {
        source,
        width,
        height,
        fps: fps.max(1.0),
        start_s,
        span_s,
        drop_nth,
        cap,
    }
}

/// Decode to frames, calling `progress(done, expected)`; returning false from
/// it cancels the decode.
pub fn import(
    path: &Path,
    options: &ImportOptions,
    progress: &mut dyn FnMut(usize, Option<usize>) -> bool,
) -> Result<Vec<Frame>> {
    let plan = plan_for(probe(path)?, options);
    import_planned(path, &plan, progress)
}

/// A spread of single frames at the plan's output size, for a size estimate
/// before anything has been imported. One seek-and-decode per frame: `-ss`
/// ahead of `-i` jumps to a keyframe instead of decoding the clip to get there,
/// which is the whole reason this is seconds rather than minutes.
pub fn sample_frames(path: &Path, plan: &ImportPlan, count: usize) -> Result<Vec<RgbaImage>> {
    let count = count.max(1);
    let frame_bytes = plan.width as usize * plan.height as usize * 4;
    let mut out = Vec::with_capacity(count);

    for i in 0..count {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error"]);
        // Spread the samples across what the plan actually takes, which is the
        // trimmed span when there is one — a sample from outside it would be
        // describing a file nobody is importing.
        let span = plan
            .span_s
            .or_else(|| plan.source.duration_s.map(|d| (d - plan.start_s).max(0.0)));
        match span.filter(|d| *d > 0.5) {
            Some(seconds) => {
                let at = plan.start_s + seconds * (i as f64 + 0.5) / count as f64;
                cmd.arg("-ss").arg(format!("{at:.3}"));
            }
            // No span to spread across, so take the opening frames instead.
            None => {
                let at = plan.start_s + i as f64 / plan.fps.max(1.0);
                if at > 0.0 {
                    cmd.arg("-ss").arg(format!("{at:.3}"));
                }
            }
        }
        let mut out_bytes = cmd
            .arg("-i")
            .arg(path)
            .args(["-frames:v", "1", "-vf"])
            .arg(format!("scale={}:{}", plan.width, plan.height))
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .output()
            .context("running ffmpeg")?;

        // A seek past the end returns nothing; that is the end of the sample,
        // not a failure.
        if out_bytes.stdout.len() < frame_bytes {
            break;
        }
        out_bytes.stdout.truncate(frame_bytes);
        let img = RgbaImage::from_raw(plan.width, plan.height, out_bytes.stdout)
            .context("ffmpeg returned a short frame")?;
        out.push(img);
    }

    if out.is_empty() {
        bail!("could not read any frames from {}", path.display());
    }
    Ok(out)
}

/// Decode exactly this plan. The import dialog hands back a plan the user
/// edited, and running it verbatim is what makes the preview honest.
pub fn import_planned(
    path: &Path,
    plan: &ImportPlan,
    progress: &mut dyn FnMut(usize, Option<usize>) -> bool,
) -> Result<Vec<Frame>> {
    let ImportPlan {
        width: w,
        height: h,
        fps,
        start_s,
        span_s,
        drop_nth,
        cap,
        ..
    } = *plan;

    let mut filters = Vec::new();
    if (w, h) != (plan.source.width, plan.source.height) {
        // Explicit height, not -2: the picker is allowed to change the aspect.
        filters.push(format!("scale={w}:{h}"));
    }
    if (fps - plan.source.fps).abs() > 0.01 {
        filters.push(format!("fps={fps}"));
    }

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error"]);
    // Ahead of `-i`, so the seek jumps by keyframe instead of decoding the
    // clip to get there. `-t` after the input bounds the output.
    if start_s > 0.0 {
        cmd.arg("-ss").arg(format!("{start_s:.3}"));
    }
    cmd.arg("-i").arg(path);
    if let Some(span) = span_s {
        cmd.arg("-t").arg(format!("{span:.3}"));
    }
    if !filters.is_empty() {
        cmd.arg("-vf").arg(filters.join(","));
    }
    // A backstop, not the policy: a plan that fits never reaches this, and an
    // over-budget one is refused before it gets here. It exists for streams
    // whose container names no duration, where there is nothing to plan
    // against. It counts decoded frames, so the drop has to be added back:
    // `cap` is a ceiling on the frames that are kept.
    let decode_cap = match drop_nth {
        Some(n) => (cap.saturating_mul(n) / (n - 1)).saturating_add(1),
        None => cap,
    };
    cmd.arg("-frames:v").arg(decode_cap.to_string());
    cmd.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("running ffmpeg")?;
    let mut stdout = child.stdout.take().expect("piped");

    let expected = plan.frames();
    let frame_bytes = w as usize * h as usize * 4;
    let mut buf = vec![0u8; frame_bytes];
    let mut frames = Vec::new();
    let mut decoded = 0usize;
    let mut cancelled = false;

    loop {
        match stdout.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e).context("reading frames from ffmpeg"),
        }
        decoded += 1;
        // Every Nth frame goes, frame 0 stays: the same arithmetic
        // `Document::drop_every_nth` uses, applied before the pixels are kept
        // rather than after, so the frames dropped never cost memory.
        if drop_nth.is_some_and(|n| decoded.is_multiple_of(n)) {
            continue;
        }
        let pixels = std::mem::replace(&mut buf, vec![0u8; frame_bytes]);
        let img = RgbaImage::from_raw(w, h, pixels).context("ffmpeg returned a short frame")?;
        frames.push(Frame::new(img, 4));
        if !progress(frames.len(), expected) || frames.len() >= cap {
            cancelled = true;
            break;
        }
    }

    if cancelled {
        let _ = child.kill();
    }
    let status = child.wait().context("waiting for ffmpeg")?;
    if !cancelled && !status.success() {
        bail!("ffmpeg failed to decode {}", path.display());
    }
    if frames.is_empty() {
        bail!("{} decoded to zero frames", path.display());
    }

    // The drop leaves fewer frames covering the same stretch of time, so the
    // delays come from the rate they actually play at, not the decode's.
    let kept_fps = match (drop_nth, decoded) {
        (Some(_), d) if d > frames.len() && !frames.is_empty() => {
            fps * frames.len() as f64 / d as f64
        }
        _ => fps,
    };
    let delays = delays_for_fps(kept_fps, frames.len());
    for (frame, delay) in frames.iter_mut().zip(delays) {
        frame.delay_cs = delay;
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a clip with lavfi. Returns None when ffmpeg is missing, which is
    /// a skip rather than a failure: the decode tests are about our arguments,
    /// not about whether the machine has ffmpeg.
    fn fixture(name: &str, size: &str, rate: u32, seconds: u32) -> Option<std::path::PathBuf> {
        if !crate::pipeline::caps::Caps::probe().can_import() {
            eprintln!("skipping {name}: no ffmpeg");
            return None;
        }
        let path = std::env::temp_dir().join(format!("gifkino-{name}.mp4"));
        let status = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!(
                "testsrc=size={size}:rate={rate}:duration={seconds}"
            ))
            .args(["-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .expect("running ffmpeg");
        assert!(status.success(), "could not build the fixture clip");
        Some(path)
    }

    /// End to end through real ffmpeg: a clip too big for the budget comes back
    /// scaled, thinned, and still the length it started.
    #[test]
    fn a_real_decode_stays_inside_the_budget() {
        let Some(path) = fixture("budget", "640x480", 30, 6) else {
            return;
        };
        let options = ImportOptions {
            max_width: Some(320),
            target: None,
            fps: None,
            trim: None,
            drop_nth: None,
            // Room for roughly 20 frames at 320x240.
            max_bytes: 20 * 320 * 240 * 4,
        };

        let mut seen = Vec::new();
        let frames = import(&path, &options, &mut |done, expected| {
            seen.push((done, expected));
            true
        })
        .unwrap();

        let (w, h) = frames[0].pixels.dimensions();
        assert_eq!((w, h), (320, 240), "scaled on the way in");
        assert!(frames.len() <= 20, "over the cap: {}", frames.len());
        assert!(frames.len() >= 15, "thinned too hard: {}", frames.len());

        let total_cs: u32 = frames.iter().map(|f| f.delay_cs as u32).sum();
        assert!(
            (total_cs as i32 - 600).abs() <= 60,
            "6 s of clip, got {total_cs} cs"
        );

        assert_eq!(seen.len(), frames.len(), "progress reported once per frame");
        assert!(
            seen.iter()
                .all(|(_, expected)| expected.is_some_and(|e| e <= 20))
        );

        let _ = std::fs::remove_file(&path);
    }

    /// The progress callback is also the cancel button: the import dialog's
    /// window closing has to stop the decode, not wait it out.
    #[test]
    fn returning_false_from_progress_stops_the_decode() {
        let Some(path) = fixture("cancel", "320x240", 30, 5) else {
            return;
        };
        let options = ImportOptions {
            max_width: None,
            ..Default::default()
        };

        let frames = import(&path, &options, &mut |done, _| done < 3).unwrap();
        assert_eq!(frames.len(), 3, "stopped where it was told to");

        let _ = std::fs::remove_file(&path);
    }

    /// Sampling by seeking is what keeps the exact estimate to seconds. It has
    /// to land the plan's output size, spread across the clip.
    #[test]
    fn sampling_seeks_across_the_clip_at_the_planned_size() {
        let Some(path) = fixture("sample", "640x480", 30, 6) else {
            return;
        };
        let options = ImportOptions {
            target: Some((160, 120)),
            fps: Some(10.0),
            ..Default::default()
        };
        let plan = plan_for(probe(&path).unwrap(), &options);

        let t = std::time::Instant::now();
        let samples = sample_frames(&path, &plan, 8).unwrap();
        let took = t.elapsed();

        assert_eq!(samples.len(), 8);
        assert!(samples.iter().all(|f| f.dimensions() == (160, 120)));
        assert!(
            samples[0].as_raw() != samples[7].as_raw(),
            "seeking, not re-reading the first frame"
        );
        // Decoding all 180 frames would cost far more; seeking must not.
        assert!(took.as_secs() < 15, "sampling took {took:?}");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn frame_rates_parse() {
        assert_eq!(parse_rate("30/1"), 30.0);
        assert!((parse_rate("30000/1001") - 29.97).abs() < 0.01);
        assert_eq!(parse_rate("25"), 25.0);
        assert_eq!(parse_rate("0/0"), 0.0);
    }

    #[test]
    fn a_long_clip_loses_frame_rate_not_its_ending() {
        let info = VideoInfo {
            width: 1920,
            height: 1080,
            fps: 60.0,
            duration_s: Some(120.0),
        };
        let opts = ImportOptions::default();
        let plan = plan_for(info, &opts);
        assert_eq!((plan.width, plan.height), (1280, 720));
        assert!(
            plan.bytes().unwrap() <= opts.max_bytes,
            "stays inside the RAM budget"
        );
        assert!(
            (2.0..60.0).contains(&plan.fps),
            "thinned, not frozen: {}",
            plan.fps
        );
        assert!(!plan.over_budget(), "the whole clip fits");
        assert_eq!(
            plan.playback_fps(),
            plan.fps,
            "nothing dropped, so nothing to slow down"
        );
    }

    #[test]
    fn a_short_clip_keeps_its_frame_rate_and_raises_nothing() {
        let info = VideoInfo {
            width: 640,
            height: 480,
            fps: 30.0,
            duration_s: Some(5.0),
        };
        let plan = plan_for(info, &ImportOptions::default());
        assert_eq!(plan.fps, 30.0);
        assert_eq!(plan.frames(), Some(150));
        assert_eq!(plan.playback_fps(), 30.0);
    }

    #[test]
    fn scaling_alone_leaves_the_rate_and_the_span_alone() {
        let info = VideoInfo {
            width: 1920,
            height: 1080,
            fps: 24.0,
            duration_s: Some(8.0),
        };
        let plan = plan_for(info, &ImportOptions::default());
        assert_eq!((plan.width, plan.height), (1280, 720));
        assert_eq!(plan.fps, 24.0);
        assert_eq!((plan.start_s, plan.span_s), (0.0, None));
    }

    #[test]
    fn an_endless_clip_is_capped_but_unknowable() {
        let info = VideoInfo {
            width: 320,
            height: 240,
            fps: 25.0,
            duration_s: None,
        };
        let plan = plan_for(info, &ImportOptions::default());
        assert_eq!(plan.fps, 25.0, "no duration, nothing to thin against");
        assert_eq!(plan.frames(), None);
        assert!(!plan.over_budget(), "nothing to be over budget against");
    }

    /// A trim in seconds and the same trim in frames name the same span, and
    /// both ends are inclusive: frames 234 to 1234 is 1001 frames, not 1000.
    #[test]
    fn a_trim_resolves_to_the_same_span_either_way() {
        let info = VideoInfo {
            width: 640,
            height: 480,
            fps: 25.0,
            duration_s: Some(100.0),
        };
        assert_eq!(
            Trim::Seconds {
                start: 10.0,
                end: Some(20.0)
            }
            .resolve(&info),
            (10.0, Some(10.0))
        );
        let (start, span) = Trim::Frames {
            first: 234,
            last: Some(1234),
        }
        .resolve(&info);
        assert_eq!(start, 234.0 / 25.0);
        assert_eq!((span.unwrap() * 25.0).round() as usize, 1001);

        // An open end runs to the end of the clip, and a span past it stops
        // there rather than promising frames the file does not have.
        assert_eq!(
            Trim::Seconds {
                start: 90.0,
                end: None
            }
            .resolve(&info),
            (90.0, None)
        );
        assert_eq!(
            Trim::Seconds {
                start: 90.0,
                end: Some(500.0)
            }
            .resolve(&info),
            (90.0, Some(10.0))
        );
    }

    /// The trim and the drop are what the plan counts: a ten-second window of
    /// a long clip costs ten seconds of frames, and dropping one in three
    /// leaves two of every three paying for memory.
    #[test]
    fn the_plan_counts_the_trim_and_the_drop() {
        let info = VideoInfo {
            width: 320,
            height: 240,
            fps: 30.0,
            duration_s: Some(600.0),
        };
        let options = ImportOptions {
            fps: Some(30.0),
            trim: Some(Trim::Seconds {
                start: 60.0,
                end: Some(70.0),
            }),
            drop_nth: Some(3),
            ..Default::default()
        };
        let plan = plan_for(info.clone(), &options);
        assert_eq!((plan.start_s, plan.span_s), (60.0, Some(10.0)));
        assert_eq!(plan.decoded_frames(), Some(300), "ten seconds at 30 fps");
        assert_eq!(plan.frames(), Some(200), "one in every three dropped");
        assert_eq!(plan.bytes(), Some(200 * 320 * 240 * 4));
        assert_eq!(plan.playback_fps(), 20.0, "same ten seconds, fewer frames");

        // A drop of one is not a drop, and neither is none.
        for keep_all in [None, Some(1)] {
            let plan = plan_for(
                info.clone(),
                &ImportOptions {
                    fps: Some(30.0),
                    drop_nth: keep_all,
                    ..Default::default()
                },
            );
            assert_eq!(plan.drop_nth, None, "{keep_all:?} keeps every frame");
            assert_eq!(plan.frames(), Some(18000));
        }
    }

    /// The drop is a budget lever, not just an optimization: it is counted
    /// before the refusal, so a clip that will not fit whole can be brought in
    /// thinned instead of not at all.
    #[test]
    fn dropping_frames_buys_a_clip_that_would_not_fit() {
        let info = VideoInfo {
            width: 640,
            height: 360,
            fps: 30.0,
            duration_s: Some(120.0),
        };
        // 3600 frames of 640x360 is 3.2 GB; a fifth of a gigabyte buys 227.
        let budget = ImportOptions {
            fps: Some(30.0),
            max_bytes: 210 << 20,
            ..Default::default()
        };
        assert!(plan_for(info.clone(), &budget).over_budget());

        let trimmed_and_thinned = ImportOptions {
            trim: Some(Trim::Seconds {
                start: 0.0,
                end: Some(10.0),
            }),
            drop_nth: Some(2),
            ..budget.clone()
        };
        let plan = plan_for(info, &trimmed_and_thinned);
        assert_eq!(plan.frames(), Some(150), "ten seconds, every other frame");
        assert!(!plan.over_budget(), "which the budget can afford");
    }

    /// End to end through real ffmpeg: the decode takes the trimmed span, the
    /// drop takes every other frame out of it, and what is left still plays
    /// for as long as the span it came from.
    #[test]
    fn a_real_decode_trims_and_drops() {
        let Some(path) = fixture("trim", "320x240", 30, 6) else {
            return;
        };
        let options = ImportOptions {
            max_width: None,
            fps: Some(30.0),
            trim: Some(Trim::Seconds {
                start: 2.0,
                end: Some(4.0),
            }),
            drop_nth: Some(2),
            ..Default::default()
        };
        let frames = import(&path, &options, &mut |_, _| true).unwrap();

        // Two seconds at 30 fps is 60 decoded, 30 kept. ffmpeg's seek lands on
        // a keyframe, so allow a frame or two either way.
        assert!(
            (26..=34).contains(&frames.len()),
            "half of two seconds at 30 fps: {}",
            frames.len()
        );
        let total_cs: u32 = frames.iter().map(|f| f.delay_cs as u32).sum();
        assert!(
            (total_cs as i32 - 200).abs() <= 40,
            "the kept frames still cover two seconds: {total_cs} cs"
        );
    }

    /// Regression: the cap is what the memory budget buys at the planned size,
    /// and nothing else. A long 1080p clip asked for gigabytes before it
    /// existed and the machine spent the import swapping; a second, hidden
    /// ceiling on the frame count refused cheap imports that cost nothing —
    /// 18000 frames of 160x120 is 1.3 GB, but 4000 of them is 307 MB and has
    /// no business being refused under a 1200 MB budget.
    #[test]
    fn the_cap_is_what_the_memory_budget_buys() {
        let opts = ImportOptions::default();

        let big = VideoInfo {
            width: 1280,
            height: 720,
            fps: 30.0,
            duration_s: Some(600.0),
        };
        let plan = plan_for(big, &opts);
        assert!(
            plan.cap * 1280 * 720 * 4 <= opts.max_bytes,
            "the cap is what the budget buys"
        );
        // Ten minutes of 720p will not fit even at the lowest automatic rate,
        // and that is now a refusal rather than a silent trim.
        assert!(plan.over_budget());

        let cheap = VideoInfo {
            width: 160,
            height: 120,
            fps: 30.0,
            duration_s: Some(133.0),
        };
        let plan = plan_for(
            cheap,
            &ImportOptions {
                fps: Some(30.0),
                ..opts.clone()
            },
        );
        assert_eq!(plan.frames(), Some(3990));
        assert!(
            !plan.over_budget(),
            "3990 frames of 160x120 is 307 MB: cheap, and no frame ceiling to trip"
        );
    }

    /// Whatever the budget does to the frame rate, the clip still plays for as
    /// long as it did.
    #[test]
    fn thinning_the_frame_rate_preserves_the_running_time() {
        let info = VideoInfo {
            width: 1920,
            height: 1080,
            fps: 60.0,
            duration_s: Some(90.0),
        };
        let plan = plan_for(info, &ImportOptions::default());
        let delays = delays_for_fps(plan.fps, plan.frames().unwrap());
        let total_cs: u32 = delays.iter().map(|d| *d as u32).sum();
        assert!(
            (total_cs as f64 - 9000.0).abs() <= 100.0,
            "90 s, got {total_cs} cs"
        );
    }

    #[test]
    fn expected_frame_count_needs_a_duration() {
        let info = VideoInfo {
            width: 100,
            height: 50,
            fps: 25.0,
            duration_s: Some(4.0),
        };
        assert_eq!(info.estimated_frames(info.fps), Some(100));
        assert_eq!(info.estimated_frames(10.0), Some(40));
        assert_eq!(
            VideoInfo {
                duration_s: None,
                ..info
            }
            .estimated_frames(25.0),
            None
        );
    }
}
