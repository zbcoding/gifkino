//! Translations, in the gettext `.po` format Impasto uses.
//!
//! The catalog is the `.po` file itself, parsed at startup, rather than a `.mo`
//! compiled by `msgfmt`. Same format, same tooling on the translator's side —
//! but no build step, no install prefix and no dependency, which matters
//! because this app has none of those yet (see `settings.rs` for the same
//! reasoning about GSettings). A few hundred strings parse in well under a
//! millisecond, which is the only thing `.mo` was ever buying.
//!
//! One deliberate difference from `msgfmt`: fuzzy entries are used, not
//! dropped. Here `#, fuzzy` marks an AI draft awaiting human review, and a
//! draft nobody can see is a draft nobody will ever correct.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Translate. The msgid is the US English string, so an untranslated build
/// reads correctly rather than showing keys.
///
/// The `'static` bound is what lets the fallback be returned by reference: an
/// untranslated string is the literal in the source, already alive for the
/// program.
pub fn t(msgid: &'static str) -> &'static str {
    CATALOG
        .get_or_init(load_catalog)
        .get(msgid)
        .map(String::as_str)
        .unwrap_or(msgid)
}

/// Mark a literal for extraction without translating it here. gettext's `N_`:
/// for strings defined far from where they are shown, such as the action labels
/// in `keymap.rs` and the history labels stored in a `Change`.
pub fn n(msgid: &'static str) -> &'static str {
    msgid
}

/// Translate a string that is not a literal — a history label carried in a
/// `Change`, say. Allocates, which is why `t` exists for the common case.
pub fn lookup(msgid: &str) -> String {
    CATALOG
        .get_or_init(load_catalog)
        .get(msgid)
        .cloned()
        .unwrap_or_else(|| msgid.to_string())
}

/// Two-form plural. ponytail: this is a choice between two msgids, not the
/// Plural-Forms expression gettext evaluates, so it is right for languages with
/// one plural (de, en) and for languages with none (ja) and wrong for Slavic
/// ones. Swap in a real Plural-Forms evaluator when the first such locale
/// arrives; the call sites do not change.
pub fn tn(one: &'static str, many: &'static str, n: usize) -> &'static str {
    if n == 1 { t(one) } else { t(many) }
}

/// Fill `{name}` placeholders. `format!` needs a literal, and a translated
/// string is never one; named placeholders also survive a translator reordering
/// the sentence, which positional ones do not.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in values {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

/// The locale the catalog was loaded for, for the About dialog and for tests.
pub fn locale() -> String {
    current_locale().unwrap_or_else(|| "en_US".into())
}

/// Explicit setting first, then the environment in the order gettext reads it.
fn current_locale() -> Option<String> {
    if let Some(lang) = crate::settings::Settings::load().language {
        return Some(lang);
    }
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .map(|value| value.trim().to_string())
        .find(|value| !value.is_empty() && value != "C" && value != "POSIX")
        // LANGUAGE is a colon-separated priority list; the rest are single
        // locales, and splitting on ':' leaves those untouched.
        .and_then(|value| value.split(':').next().map(normalize_locale))
}

/// `de_DE.UTF-8@euro` is `de_DE`.
fn normalize_locale(value: &str) -> String {
    value
        .split(['.', '@'])
        .next()
        .unwrap_or(value)
        .replace('-', "_")
        .to_string()
}

/// `de_DE` tries `de_DE.po` and then `de.po`, which is what lets one German
/// catalog serve de_DE, de_AT and de_CH.
fn candidates(locale: &str) -> Vec<String> {
    let mut names = vec![locale.to_string()];
    if let Some((base, _)) = locale.split_once('_') {
        names.push(base.to_string());
    }
    names
}

fn load_catalog() -> HashMap<String, String> {
    let Some(locale) = current_locale() else {
        return HashMap::new();
    };
    for dir in search_dirs() {
        for name in candidates(&locale) {
            let path = dir.join(format!("{name}.po"));
            if let Ok(text) = std::fs::read_to_string(&path) {
                return parse_po(&text);
            }
        }
    }
    HashMap::new()
}

/// Where a `po` directory might be. The source tree comes first so that
/// `cargo run` picks up an edit without an install, which is the whole workflow
/// while the catalogs are being written.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("GIFKINO_PO_DIR") {
        dirs.push(PathBuf::from(dir));
    }
    // target/debug/gifkino and target/debug/deps/gifkino-abc123 both
    // reach the repository root inside four steps.
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..4 {
            let Some(current) = dir else { break };
            dirs.push(current.join("po"));
            dir = current.parent().map(Path::to_path_buf);
        }
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data).join("gifkino/po"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/gifkino/po"));
    }
    dirs.push(PathBuf::from("/usr/share/gifkino/po"));
    dirs.push(PathBuf::from("/app/share/gifkino/po"));
    dirs
}

/// Enough of the `.po` grammar to read a catalog: comments, `msgid`/`msgstr`,
/// and the continuation lines both use. `msgctxt` and `msgid_plural` are not
/// parsed because nothing emits them; an entry carrying either is skipped
/// rather than half-read.
pub fn parse_po(text: &str) -> HashMap<String, String> {
    let mut catalog = HashMap::new();
    let (mut id, mut msg) = (String::new(), String::new());
    let mut in_msgstr = false;
    let mut skip = false;

    let mut flush = |id: &mut String, msg: &mut String, skip: &mut bool| {
        // The empty msgid is the header, and an empty translation means the
        // entry is untranslated: both fall back to the source string.
        if !*skip && !id.is_empty() && !msg.is_empty() {
            catalog.insert(std::mem::take(id), std::mem::take(msg));
        }
        id.clear();
        msg.clear();
        *skip = false;
    };

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            flush(&mut id, &mut msg, &mut skip);
            in_msgstr = false;
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            // A msgctxt precedes its msgid, so the flag it set belongs to the
            // entry starting here, not to the one just ended.
            let carried = skip;
            flush(&mut id, &mut msg, &mut skip);
            skip = carried;
            in_msgstr = false;
            id = unquote(rest);
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            in_msgstr = true;
            msg = unquote(rest);
        } else if line.starts_with("msgctxt ") || line.starts_with("msgid_plural ") {
            skip = true;
        } else if line.starts_with('#') {
            // Comments carry the translator notes and the fuzzy flag. Both are
            // for people; neither changes what the app shows.
            continue;
        } else if line.starts_with('"') {
            let part = unquote(line);
            if in_msgstr {
                msg.push_str(&part);
            } else {
                id.push_str(&part);
            }
        }
    }
    flush(&mut id, &mut msg, &mut skip);
    catalog
}

/// Strip the surrounding quotes and undo the escapes gettext writes.
fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    let inner = raw
        .strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(raw);
    unescape(inner)
}

/// Undo gettext's escapes on the inside of a quoted string. Separate from
/// `unquote` because a msgid assembled from continuation lines has already
/// lost its quotes.
fn unescape(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// The reverse, for the extraction script's tests and for anything that writes
/// a `.po` back out.
pub fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catalog_reads_entries_and_skips_the_header() {
        let catalog = parse_po(
            r#"
# German translation
msgid ""
msgstr "Content-Type: text/plain; charset=UTF-8\n"

#. Translators: the export button
msgid "Export GIF"
msgstr "GIF exportieren"

msgid "Open…"
msgstr "Öffnen…"
"#,
        );
        assert_eq!(
            catalog.get("Export GIF").map(String::as_str),
            Some("GIF exportieren")
        );
        assert_eq!(catalog.get("Open…").map(String::as_str), Some("Öffnen…"));
        assert_eq!(catalog.len(), 2, "the header is not an entry: {catalog:?}");
    }

    /// An untranslated entry has to fall through to the source string, or a
    /// half-finished catalog turns the UI into blanks.
    #[test]
    fn empty_translations_are_not_entries() {
        let catalog =
            parse_po("msgid \"Undo\"\nmsgstr \"\"\n\nmsgid \"Redo\"\nmsgstr \"Wiederholen\"\n");
        assert!(!catalog.contains_key("Undo"));
        assert_eq!(catalog.get("Redo").map(String::as_str), Some("Wiederholen"));
    }

    /// Impasto's convention marks AI drafts fuzzy so Weblate flags them. Unlike
    /// msgfmt we keep them: an unreviewed translation nobody can see is one
    /// nobody will ever correct.
    #[test]
    fn fuzzy_entries_are_used_rather_than_dropped() {
        let catalog = parse_po(
            "#. Translators: Optimize menu item\n\
             #. AI-generated translation; human review requested.\n\
             #, fuzzy\n\
             msgid \"Crop all frames…\"\n\
             msgstr \"Alle Bilder zuschneiden…\"\n",
        );
        assert_eq!(
            catalog.get("Crop all frames…").map(String::as_str),
            Some("Alle Bilder zuschneiden…")
        );
    }

    #[test]
    fn multiline_strings_and_escapes_survive_the_round_trip() {
        let catalog = parse_po(
            "msgid \"\"\n\
             \"Drag a box \"\n\
             \"on the canvas.\"\n\
             msgstr \"\"\n\
             \"Ziehen Sie einen Rahmen\\n\"\n\
             \"auf der Leinwand.\"\n",
        );
        assert_eq!(
            catalog.get("Drag a box on the canvas.").map(String::as_str),
            Some("Ziehen Sie einen Rahmen\nauf der Leinwand.")
        );

        for original in [
            "plain",
            "with \"quotes\"",
            "two\nlines",
            "a\\backslash",
            "tab\there",
        ] {
            let round = parse_po(&format!(
                "msgid {}\nmsgstr {}\n",
                quote("k"),
                quote(original)
            ));
            assert_eq!(
                round.get("k").map(String::as_str),
                Some(original),
                "{original:?}"
            );
        }
    }

    /// Entries this parser does not understand must be skipped whole, never
    /// half-read into a wrong translation.
    #[test]
    fn contextual_and_plural_entries_are_skipped_not_mangled() {
        let catalog = parse_po(
            "msgctxt \"verb\"\nmsgid \"Export\"\nmsgstr \"Exportieren\"\n\n\
             msgid \"Frame\"\nmsgid_plural \"Frames\"\nmsgstr[0] \"Bild\"\n\n\
             msgid \"Undo\"\nmsgstr \"Rückgängig\"\n",
        );
        assert!(!catalog.contains_key("Export"), "{catalog:?}");
        assert!(!catalog.contains_key("Frame"));
        assert_eq!(catalog.get("Undo").map(String::as_str), Some("Rückgängig"));
    }

    /// The shipped catalogs, parsed for real. Catches a mangled `.po`, a msgid
    /// renamed in the source without `scripts/i18n.py merge`, and an escape the
    /// parser and the writer disagree about.
    ///
    /// The msgids come from `template_msgids`, which joins an entry written
    /// across continuation lines. Reading only the lines that start `msgid `
    /// dropped those: a long entry's first line is `msgid ""`, which looks
    /// exactly like the header and was filtered out with it, so the two
    /// multi-line strings in the template went unchecked in every catalog.
    #[test]
    fn every_shipped_catalog_covers_every_marked_string() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let template =
            std::fs::read_to_string(root.join("po/messages.pot")).expect("run scripts/i18n.py pot");
        // The template stores escapes; a parsed catalog's keys do not.
        let wanted: Vec<String> = template_msgids(&template)
            .iter()
            .map(|id| unescape(id))
            .collect();
        assert!(
            wanted.len() > 100,
            "only {} strings extracted",
            wanted.len()
        );
        assert!(
            wanted.iter().any(|id| id.contains('\n')),
            "no multi-line entry among the {} extracted, so this test would \
             not notice losing them again",
            wanted.len()
        );

        let linguas = std::fs::read_to_string(root.join("po/LINGUAS")).unwrap();
        for lang in linguas.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let path = root.join(format!("po/{lang}.po"));
            let catalog = parse_po(&std::fs::read_to_string(&path).expect(lang));
            let missing: Vec<&String> = wanted
                .iter()
                .filter(|id| !catalog.contains_key(*id))
                .collect();
            assert!(missing.is_empty(), "{lang} is missing {missing:?}");
        }
    }

    /// The test above only reports entries a catalog is *missing*, so a msgid
    /// can outlive the string it came from: delete a feature, skip
    /// `scripts/i18n.py`, and the template keeps the entry while every catalog
    /// keeps a translation for a string the app can no longer show. Dropping
    /// screen recording left three behind exactly that way.
    ///
    /// The template stores each literal as it was written, escapes and all, so
    /// the file it was extracted from has to still contain it verbatim.
    #[test]
    fn no_template_entry_outlives_the_string_it_came_from() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let potfiles = std::fs::read_to_string(root.join("po/POTFILES.in")).unwrap();
        let mut sources = String::new();
        for name in potfiles
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let text = std::fs::read_to_string(root.join(name)).expect(name);
            sources.push_str(&join_rustfmt_continuations(&text));
        }

        let template =
            std::fs::read_to_string(root.join("po/messages.pot")).expect("run scripts/i18n.py pot");
        let stale: Vec<String> = template_msgids(&template)
            .into_iter()
            .filter(|id| !sources.contains(id))
            .collect();
        assert!(
            stale.is_empty(),
            "run scripts/i18n.py pot && scripts/i18n.py merge; \
             no source in POTFILES.in marks {stale:?}"
        );
    }

    /// rustfmt breaks a literal too long for one line with a trailing `\`,
    /// which Rust reads as neither the newline nor the next line's indentation
    /// being part of the string. Undo that, so a joined msgid can be looked
    /// for as plain text. `scripts/i18n.py` joins the same seam from the other
    /// side, which is how seven msgids once fell out of the template.
    fn join_rustfmt_continuations(source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        let mut chars = source.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch != '\\' || chars.peek() != Some(&'\n') {
                out.push(ch);
                continue;
            }
            chars.next();
            while chars.next_if(|c| *c == ' ' || *c == '\t').is_some() {}
        }
        out
    }

    /// Every msgid in a template, still carrying gettext's escapes and with the
    /// continuation lines of a long entry joined back together. `parse_po` is
    /// no use here: it unescapes, and it drops an entry whose msgstr is empty,
    /// which in a template is all of them.
    fn template_msgids(template: &str) -> Vec<String> {
        let inner = |line: &str| {
            line.trim()
                .trim_start_matches('"')
                .trim_end_matches('"')
                .to_string()
        };
        let mut ids = Vec::new();
        let mut current: Option<String> = None;
        for line in template.lines() {
            if let Some(rest) = line.strip_prefix("msgid ") {
                current = Some(inner(rest));
            } else if line.starts_with('"') {
                if let Some(id) = current.as_mut() {
                    id.push_str(&inner(line));
                }
            } else if let Some(id) = current.take() {
                // A blank line, a msgstr or a comment ends the entry. The
                // header's msgid is empty and is not a string anyone marked.
                if !id.is_empty() {
                    ids.push(id);
                }
            }
        }
        ids.extend(current.filter(|id| !id.is_empty()));
        ids
    }

    /// A translation that loses a placeholder renders `{count}` to the user, or
    /// silently drops a number. Cheaper to catch here than in a screenshot.
    #[test]
    fn translations_keep_every_placeholder_the_source_has() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let names = |s: &str| {
            let mut out: Vec<String> = s
                .split('{')
                .skip(1)
                .filter_map(|part| part.split_once('}').map(|(name, _)| name.to_string()))
                .filter(|name| name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .collect();
            out.sort();
            out
        };
        let linguas = std::fs::read_to_string(root.join("po/LINGUAS")).unwrap();
        for lang in linguas.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let text = std::fs::read_to_string(root.join(format!("po/{lang}.po"))).unwrap();
            for (msgid, msgstr) in parse_po(&text) {
                assert_eq!(
                    names(&msgid),
                    names(&msgstr),
                    "{lang}: {msgid:?} -> {msgstr:?}"
                );
            }
        }
    }

    #[test]
    fn locales_narrow_from_the_region_to_the_language() {
        assert_eq!(normalize_locale("de_DE.UTF-8@euro"), "de_DE");
        assert_eq!(normalize_locale("ja_JP.UTF-8"), "ja_JP");
        assert_eq!(normalize_locale("pt-BR"), "pt_BR");
        assert_eq!(candidates("pt_BR"), vec!["pt_BR", "pt"]);
        assert_eq!(candidates("ja"), vec!["ja"], "nothing to fall back to");
    }

    #[test]
    fn placeholders_are_named_so_a_sentence_can_be_reordered() {
        let english = "Exported to {path} · {size}";
        let german = "{size} nach {path} exportiert";
        let values = [("path", "/tmp/a.gif"), ("size", "412 KB")];
        assert_eq!(fill(english, &values), "Exported to /tmp/a.gif · 412 KB");
        assert_eq!(fill(german, &values), "412 KB nach /tmp/a.gif exportiert");
        // An unknown placeholder is left alone rather than swallowed, so a
        // typo in a catalog is visible instead of silent.
        assert_eq!(fill("{nope}", &values), "{nope}");
    }
}
