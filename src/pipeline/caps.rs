//! Capabilities are probed once at startup, not at the moment of use.
//! Discovering that recording is unavailable after picking a region is the
//! worst possible time to find out.

use std::process::{Command, Stdio};

use crate::i18n::n;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    pub ffmpeg: bool,
    pub ffprobe: bool,
    /// Optional. Without it, exports skip the -O3 pass and stay valid GIFs.
    pub gifsicle: bool,
    /// ffmpeg's Wayland capture source.
    pub pipewiregrab: bool,
    pub x11grab: bool,
}

impl Caps {
    pub fn probe() -> Self {
        let sources = ffmpeg_sources();
        Caps {
            ffmpeg: runs("ffmpeg"),
            ffprobe: runs("ffprobe"),
            gifsicle: runs("gifsicle"),
            pipewiregrab: sources.contains("pipewiregrab"),
            x11grab: sources.contains("x11grab"),
        }
    }

    pub fn can_import(&self) -> bool {
        self.ffmpeg && self.ffprobe
    }

    /// Reason to attach to a disabled action, or None when it works.
    pub fn import_blocker(&self) -> Option<&'static str> {
        (!self.can_import()).then_some(n("ffmpeg is not available in this runtime"))
    }

    pub fn record_blocker(&self) -> Option<&'static str> {
        if !self.ffmpeg {
            Some(n("ffmpeg is not available in this runtime"))
        } else if !self.pipewiregrab && !self.x11grab {
            Some(n("this ffmpeg build has no screen capture source"))
        } else {
            None
        }
    }
}

fn runs(program: &str) -> bool {
    Command::new(program)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn ffmpeg_sources() -> String {
    let mut text = String::new();
    for arg in ["-devices", "-filters"] {
        if let Ok(out) = Command::new("ffmpeg").args(["-hide_banner", arg]).output() {
            text.push_str(&String::from_utf8_lossy(&out.stdout));
        }
    }
    text
}
