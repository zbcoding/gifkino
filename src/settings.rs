//! User settings: a flat `key = value` file under the XDG config directory.
//!
//! Not GSettings, which wants a schema compiled and installed system-wide —
//! a packaging step this app does not have yet — and not a serialization crate
//! for two dozen lines of parsing.

use std::path::{Path, PathBuf};

/// Frames live in RAM as RGBA, so these are the numbers that decide how much
/// of a long video can be open at once. Three budgets: imports cap what one
/// decode may land, operations cap what one edit (a resize, …) may produce,
/// and the total caps old frames plus new — undo keeps the old ones while a
/// worker holds the new, so the transient peak is the sum. 4 GB is around
/// 1100 frames at 1280x720.
const DEFAULT_MAX_IMPORT_BYTES: usize = 4_096 << 20;
const DEFAULT_MAX_OPER_BYTES: usize = 4_096 << 20;
const DEFAULT_MAX_TOTAL_BYTES: usize = 8_192 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub max_import_bytes: usize,
    pub max_oper_bytes: usize,
    pub max_total_bytes: usize,
    /// Overrides the locale from the environment. `None` means follow LANGUAGE
    /// and friends, which is what a desktop user expects.
    pub language: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            max_import_bytes: DEFAULT_MAX_IMPORT_BYTES,
            max_oper_bytes: DEFAULT_MAX_OPER_BYTES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
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
        for (key, value) in entries(text) {
            match key {
                "max_import_memory_mb" => {
                    self.max_import_bytes = parse_mb(value).unwrap_or(self.max_import_bytes);
                }
                "max_oper_memory_mb" => {
                    self.max_oper_bytes = parse_mb(value).unwrap_or(self.max_oper_bytes);
                }
                "max_total_memory_mb" => {
                    self.max_total_bytes = parse_mb(value).unwrap_or(self.max_total_bytes);
                }
                "language" => {
                    self.language = (!value.is_empty()).then(|| value.to_string());
                }
                _ => {}
            }
        }
    }
}

/// A positive count of megabytes, as bytes. Zero is no budget at all, so it
/// is as unparseable as a word.
fn parse_mb(value: &str) -> Option<usize> {
    let mb = value.parse::<usize>().ok().filter(|mb| *mb > 0)?;
    Some(mb << 20)
}

/// The `key = value` lines of a config file, both sides trimmed. Blank lines,
/// `#` comments and lines with no `=` are skipped.
pub fn entries(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
}

/// Write a config file, creating its directory. Best effort: a read-only
/// config directory is not worth an error the user cannot act on.
pub fn write_best_effort(path: &Path, text: &str) {
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let _ = std::fs::write(path, text);
}

/// An XDG base directory: `var` when it names an absolute path, which is the
/// only kind the spec allows, otherwise `$HOME/home_fallback`.
pub fn xdg_dir(var: &str, home_fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(home_fallback)))
}

pub fn path() -> Option<PathBuf> {
    Some(
        xdg_dir("XDG_CONFIG_HOME", ".config")?
            .join("gifkino")
            .join("settings.conf"),
    )
}

/// Leave a commented default behind on first run, so the file is discoverable
/// without the app having to advertise it.
fn write_template(path: &Path) {
    write_best_effort(
        path,
        &format!(
            "# gifkino settings\n\
             \n\
             # Most memory an import may use for decoded frames, in MB.\n\
             # Videos that need more are refused until you choose a smaller\n\
             # size or a lower frame rate in the import dialog.\n\
             max_import_memory_mb = {}\n\
             \n\
             # Most memory one frame operation (a resize, …) may produce, in MB.\n\
             max_oper_memory_mb = {}\n\
             \n\
             # Most memory frames may hold in total, in MB: the document plus\n\
             # what the operation produces, since undo keeps the old frames.\n\
             max_total_memory_mb = {}\n\
             \n\
             # Interface language, as a po/ catalog name such as de or ja.\n\
             # Leave empty to follow LANGUAGE, LC_ALL, LC_MESSAGES or LANG.\n\
             language =\n",
            DEFAULT_MAX_IMPORT_BYTES >> 20,
            DEFAULT_MAX_OPER_BYTES >> 20,
            DEFAULT_MAX_TOTAL_BYTES >> 20,
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
        assert_eq!(
            parse("max_oper_memory_mb = 2048").max_oper_bytes,
            2048 << 20
        );
        assert_eq!(
            parse("max_total_memory_mb = 10240").max_total_bytes,
            10240 << 20
        );

        assert_eq!(parse("language = de").language.as_deref(), Some("de"));
        assert_eq!(
            parse("language =").language,
            None,
            "empty means follow the environment"
        );

        let default = Settings::default();
        for junk in [
            "",
            "# max_import_memory_mb = 512",
            "max_import_memory_mb = 0",
            "max_import_memory_mb = lots",
            "max_oper_memory_mb = 0",
            "max_total_memory_mb = lots",
            "nonsense",
            "other_key = 4",
        ] {
            let parsed = parse(junk);
            assert_eq!(
                parsed.max_import_bytes, default.max_import_bytes,
                "on {junk:?}"
            );
            assert_eq!(parsed.max_oper_bytes, default.max_oper_bytes, "on {junk:?}");
            assert_eq!(
                parsed.max_total_bytes, default.max_total_bytes,
                "on {junk:?}"
            );
        }
    }

    #[test]
    fn the_config_path_follows_xdg() {
        let Some(path) = path() else { return };
        assert!(
            path.ends_with("gifkino/settings.conf"),
            "{}",
            path.display()
        );
        assert!(path.is_absolute());
    }

    /// The first-run template is the documentation for the file, so it has to
    /// be the file: every key in it is one the parser reads, and read back it
    /// sets exactly the defaults. The directories it lives in may not exist yet.
    #[test]
    fn the_first_run_template_reads_back_as_the_defaults() {
        let root = std::env::temp_dir().join(format!("gifkino-settings-{}", std::process::id()));
        let path = root.join("config/gifkino/settings.conf");
        write_template(&path);
        let text = std::fs::read_to_string(&path);
        let _ = std::fs::remove_dir_all(&root);

        let mut settings = Settings {
            max_import_bytes: 1,
            max_oper_bytes: 1,
            max_total_bytes: 1,
            language: Some("xx".into()),
        };
        settings.apply(&text.expect("the template was written"));
        assert_eq!(settings, Settings::default());
    }
}
