//! Capabilities are probed once at startup, not at the moment of use.
//! Discovering that import is unavailable after picking a file is the worst
//! possible time to find out.

use std::process::{Command, Stdio};

use crate::i18n::n;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    pub ffmpeg: bool,
    pub ffprobe: bool,
    /// Optional. Without it, exports skip the -O3 pass and stay valid GIFs.
    pub gifsicle: bool,
}

impl Caps {
    pub fn probe() -> Self {
        Caps {
            ffmpeg: runs("ffmpeg"),
            ffprobe: runs("ffprobe"),
            gifsicle: runs("gifsicle"),
        }
    }

    pub fn can_import(&self) -> bool {
        self.ffmpeg && self.ffprobe
    }

    /// Reason to attach to a disabled action, or None when it works.
    pub fn import_blocker(&self) -> Option<&'static str> {
        (!self.can_import()).then_some(n("ffmpeg is not available in this runtime"))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Import needs both halves of ffmpeg: ffprobe plans the decode and ffmpeg
    /// runs it. gifsicle only optimizes exports, so it neither enables import
    /// nor blocks it.
    #[test]
    fn import_is_blocked_unless_both_ffmpeg_and_ffprobe_run() {
        for ffmpeg in [false, true] {
            for ffprobe in [false, true] {
                for gifsicle in [false, true] {
                    let caps = Caps {
                        ffmpeg,
                        ffprobe,
                        gifsicle,
                    };
                    assert_eq!(
                        caps.import_blocker().is_none(),
                        ffmpeg && ffprobe,
                        "{caps:?}"
                    );
                }
            }
        }
    }
}
