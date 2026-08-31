//! User settings: a flat `key = value` file under the XDG config directory.
//!
//! Not GSettings, which wants a schema compiled and installed system-wide —
//! a packaging step this app does not have yet — and not a serialization crate
//! for two dozen lines of parsing.

use std::path::PathBuf;

/// Frames live in RAM as RGBA, so this is the number that decides how much of a
/// long video can be open at once. 1.2 GB is around 450 frames at 1280x720.
const DEFAULT_MAX_IMPORT_BYTES: usize = 1_200 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub max_import_bytes: usize,
    /// Overrides the locale from the environment. `None` means follow LANGUAGE
    /// and friends, which is what a desktop user expects.
    pub language: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            max_import_bytes: DEFAULT_MAX_IMPORT_BYTES,
            language: None,
        }
    }
}

impl Settings {
    /// Read the config file, falling back to defaults for anything missing or
    /// unparseable. A broken settings file should not stop the app opening.
    pub fn load() -> Self {
        let mut settings = Settings::default();
        let Some(path) = path() else { return settings };

        let Ok(text) = std::fs::read_to_string(&path) else {
            write_template(&path);
            return settings;
        };
        settings.apply(&text);
        settings
    }

    /// Anything missing or unparseable leaves the current value alone.
    fn apply(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "max_import_memory_mb" => {
                    if let Ok(mb) = value.trim().parse::<usize>()
                        && mb > 0
                    {
                        self.max_import_bytes = mb << 20;
                    }
                }
                "language" => {
                    let value = value.trim();
                    self.language = (!value.is_empty()).then(|| value.to_string());
                }
                _ => {}
            }
        }
    }
}

pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("gif-editor").join("settings.conf"))
}

/// Leave a commented default behind on first run, so the file is discoverable
/// without the app having to advertise it. Best effort: a read-only config
/// directory is not worth failing an import over.
fn write_template(path: &std::path::Path) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let _ = std::fs::write(
        path,
        format!(
            "# gif-editor settings\n\
             \n\
             # Most memory an import may use for decoded frames, in MB.\n\
             # Videos that need more are refused until you choose a smaller\n\
             # size or a lower frame rate in the import dialog.\n\
             max_import_memory_mb = {}\n\
             \n\
             # Interface language, as a po/ catalog name such as de or ja.\n\
             # Leave empty to follow LANGUAGE, LC_ALL, LC_MESSAGES or LANG.\n\
             language =\n",
            DEFAULT_MAX_IMPORT_BYTES >> 20
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The parser has to survive whatever is in the file, because the file is
    /// hand-edited and a bad line must not cost the user their app.
    #[test]
    fn unparseable_lines_fall_back_to_defaults() {
        let parse = |text: &str| {
            let mut settings = Settings::default();
            settings.apply(text);
            settings
        };

        assert_eq!(
            parse("max_import_memory_mb = 512").max_import_bytes,
            512 << 20
        );
        assert_eq!(
            parse("  max_import_memory_mb=64  ").max_import_bytes,
            64 << 20
        );

        assert_eq!(parse("language = de").language.as_deref(), Some("de"));
        assert_eq!(
            parse("language =").language,
            None,
            "empty means follow the environment"
        );

        let default = Settings::default().max_import_bytes;
        for junk in [
            "",
            "# max_import_memory_mb = 512",
            "max_import_memory_mb = 0",
            "max_import_memory_mb = lots",
            "nonsense",
            "other_key = 4",
        ] {
            assert_eq!(parse(junk).max_import_bytes, default, "on {junk:?}");
        }
    }

    #[test]
    fn the_config_path_follows_xdg() {
        let Some(path) = path() else { return };
        assert!(
            path.ends_with("gif-editor/settings.conf"),
            "{}",
            path.display()
        );
        assert!(path.is_absolute());
    }
}
