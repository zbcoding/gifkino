//! Rebindable keyboard shortcuts.
//!
//! Format decision (todo.md): a flat `action = chord` file beside
//! `settings.conf`, not JSON. The whole keymap is two dozen lines of text that
//! a user can read and a diff can review; serde would be a dependency and a
//! derive for something `split_once('=')` already handles. Import and export
//! are a file copy, which is what "import/export a keybindings file" means in
//! practice.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gtk4::gdk;

use crate::i18n::n;

/// Everything that can carry a shortcut. The tool entries are deliberately
/// separate actions: they are allowed to share a key and cycle (AGENTS.md).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Action {
    Open,
    Export,
    Undo,
    Redo,
    PlayPause,
    SelectAll,
    Delete,
    ShowShortcuts,
    ToolText,
    ToolRect,
    ToolEllipse,
    ToolArrow,
    ToolCrop,
    FrameDelete,
    FrameDuplicate,
    FrameCut,
    FrameCopy,
    FramePaste,
    FrameReverse,
    ZoomToSelection,
    StripZoomIn,
    StripZoomOut,
    StripZoomReset,
}

use Action::*;

/// Order is the order the shortcuts window lists them in.
pub const ACTIONS: [Action; 23] = [
    Open,
    Export,
    Undo,
    Redo,
    PlayPause,
    SelectAll,
    Delete,
    ShowShortcuts,
    ToolText,
    ToolRect,
    ToolEllipse,
    ToolArrow,
    ToolCrop,
    FrameDelete,
    FrameDuplicate,
    FrameCut,
    FrameCopy,
    FramePaste,
    FrameReverse,
    ZoomToSelection,
    StripZoomIn,
    StripZoomOut,
    StripZoomReset,
];

impl Action {
    /// Stable key in the config file.
    pub fn id(self) -> &'static str {
        match self {
            Open => "open",
            Export => "export",
            Undo => "undo",
            Redo => "redo",
            PlayPause => "play-pause",
            SelectAll => "select-all",
            Delete => "delete",
            ShowShortcuts => "show-shortcuts",
            ToolText => "tool-text",
            ToolRect => "tool-rectangle",
            ToolEllipse => "tool-ellipse",
            ToolArrow => "tool-arrow",
            ToolCrop => "tool-crop",
            FrameDelete => "frame-delete",
            FrameDuplicate => "frame-duplicate",
            FrameCut => "frame-cut",
            FrameCopy => "frame-copy",
            FramePaste => "frame-paste",
            FrameReverse => "frame-reverse",
            ZoomToSelection => "zoom-to-selection",
            StripZoomIn => "strip-zoom-in",
            StripZoomOut => "strip-zoom-out",
            StripZoomReset => "strip-zoom-reset",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Open => n("Open"),
            Export => n("Export GIF"),
            Undo => n("Undo"),
            Redo => n("Redo"),
            PlayPause => n("Play/pause"),
            SelectAll => n("Select all frames"),
            Delete => n("Delete selection"),
            ShowShortcuts => n("Keyboard shortcuts"),
            ToolText => n("Text tool"),
            ToolRect => n("Rectangle tool"),
            ToolEllipse => n("Ellipse tool"),
            ToolArrow => n("Arrow tool"),
            // Translators: Draws a box on the canvas for cropping or zooming.
            ToolCrop => n("Crop tool"),
            FrameDelete => n("Delete frames"),
            FrameDuplicate => n("Duplicate frames"),
            FrameCut => n("Cut frames"),
            FrameCopy => n("Copy frames"),
            // Translators: Inserts the cut or copied frames after the frame on screen.
            FramePaste => n("Paste frames"),
            FrameReverse => n("Reverse frames"),
            // Translators: Fills the canvas from the crop box, on the frame on screen.
            ZoomToSelection => n("Zoom this frame"),
            // Translators: Makes the frame thumbnails in the timeline strip larger.
            StripZoomIn => n("Zoom in the frame strip"),
            // Translators: Makes the frame thumbnails in the timeline strip smaller.
            StripZoomOut => n("Zoom out the frame strip"),
            StripZoomReset => n("Reset the frame strip zoom"),
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            Open | Export => n("File"),
            Undo | Redo | Delete | SelectAll | ShowShortcuts | PlayPause => n("Edit"),
            ToolText | ToolRect | ToolEllipse | ToolArrow | ToolCrop => n("Tools"),
            FrameDelete | FrameDuplicate | FrameReverse | ZoomToSelection => n("Frames"),
            FrameCut | FrameCopy | FramePaste => n("Frames"),
            StripZoomIn | StripZoomOut | StripZoomReset => n("Frames"),
        }
    }

    /// Tools share a key on purpose, so a clash between two of them is a cycle,
    /// not a mistake to warn about.
    pub fn is_tool(self) -> bool {
        matches!(
            self,
            ToolText | ToolRect | ToolEllipse | ToolArrow | ToolCrop
        )
    }
}

/// A modifier *held* during a canvas drag, rather than a chord pressed on its
/// own. Separate from `Action` because a chord needs a key and these are the
/// modifier alone: Impasto's transform tool reads Alt, Shift and Ctrl this way
/// (`BaseTransformTool.OnMouseDown`), and they are rebindable here for the same
/// reason every other shortcut is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Modal {
    Rotate,
    KeepAspect,
    FromCenter,
}

pub const MODALS: [Modal; 3] = [Modal::Rotate, Modal::KeepAspect, Modal::FromCenter];

impl Modal {
    pub fn id(self) -> &'static str {
        match self {
            Modal::Rotate => "canvas-rotate",
            Modal::KeepAspect => "canvas-keep-aspect",
            Modal::FromCenter => "canvas-from-center",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            // Translators: Held while dragging an overlay on the canvas.
            Modal::Rotate => n("Rotate instead of move"),
            Modal::KeepAspect => n("Keep the aspect ratio"),
            Modal::FromCenter => n("Resize from the center"),
        }
    }
}

/// A set of held modifiers. Empty means unbound, which is never satisfied.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Mods {
    pub fn is_empty(self) -> bool {
        self == Mods::default()
    }

    /// The modifiers now down, given the key that was pressed. A key press
    /// reports the state *before* itself, so a bare Alt has to be read off the
    /// key name rather than the mask.
    pub fn from_event(key: gdk::Key, state: gdk::ModifierType) -> Option<Self> {
        let mut mods = Mods {
            ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
            shift: state.contains(gdk::ModifierType::SHIFT_MASK),
            alt: state.contains(gdk::ModifierType::ALT_MASK),
        };
        match key.name()?.as_str() {
            name if name.starts_with("Control") => mods.ctrl = true,
            name if name.starts_with("Shift") => mods.shift = true,
            name if name.starts_with("Alt") || name.starts_with("Meta") => mods.alt = true,
            _ => return None,
        }
        Some(mods)
    }

    pub fn parse(text: &str) -> Self {
        let mut mods = Mods::default();
        for part in text.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "shift" => mods.shift = true,
                "alt" | "meta" => mods.alt = true,
                _ => {}
            }
        }
        mods
    }

    /// Whether these modifiers are all down. A superset is still a match, so
    /// Alt+Shift can rotate and constrain at once.
    pub fn held(self, state: gdk::ModifierType) -> bool {
        !self.is_empty()
            && (!self.ctrl || state.contains(gdk::ModifierType::CONTROL_MASK))
            && (!self.shift || state.contains(gdk::ModifierType::SHIFT_MASK))
            && (!self.alt || state.contains(gdk::ModifierType::ALT_MASK))
    }

    pub fn display(self) -> String {
        let mut out = Vec::new();
        if self.ctrl {
            out.push("Ctrl");
        }
        if self.alt {
            out.push("Alt");
        }
        if self.shift {
            out.push("Shift");
        }
        out.join("+")
    }
}

/// One chord. `alt` is here because a user may bind it, not because anything
/// ships bound to it.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Chord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// gdk key name, lowercased when it is a plain letter.
    pub key: String,
}

impl Chord {
    pub fn new(ctrl: bool, shift: bool, alt: bool, key: &str) -> Self {
        Chord {
            ctrl,
            shift,
            alt,
            key: normalize(key),
        }
    }

    pub fn from_event(key: gdk::Key, state: gdk::ModifierType) -> Option<Self> {
        let name = key.name()?.to_string();
        // A bare modifier is not a chord; it is the user still pressing one.
        if name.starts_with("Control") || name.starts_with("Shift") || name.starts_with("Alt") {
            return None;
        }
        Some(Chord {
            ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
            shift: state.contains(gdk::ModifierType::SHIFT_MASK),
            alt: state.contains(gdk::ModifierType::ALT_MASK),
            key: normalize(&name),
        })
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut chord = Chord {
            ctrl: false,
            shift: false,
            alt: false,
            key: String::new(),
        };
        for part in text.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" | "meta" => chord.alt = true,
                _ => chord.key = normalize(part),
            }
        }
        (!chord.key.is_empty()).then_some(chord)
    }

    /// Shift is only part of the identity of a letter chord. On a punctuation
    /// key the shift is how you type the character at all: Ctrl+? is Ctrl+Shift
    /// +slash on most layouts, and demanding the flag would make it unbindable.
    fn shift_matters(&self) -> bool {
        self.key.len() == 1 && self.key.chars().all(|c| c.is_ascii_alphanumeric())
    }

    pub fn matches(&self, other: &Chord) -> bool {
        self.ctrl == other.ctrl
            && self.alt == other.alt
            && self.key == other.key
            && (!self.shift_matters() || self.shift == other.shift)
    }

    /// What the tooltips and the shortcuts window show.
    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.ctrl {
            out.push_str("Ctrl+");
        }
        if self.alt {
            out.push_str("Alt+");
        }
        if self.shift && self.shift_matters() {
            out.push_str("Shift+");
        }
        out.push_str(&pretty(&self.key));
        out
    }
}

fn normalize(name: &str) -> String {
    let name = name.trim();
    match name.to_ascii_lowercase().as_str() {
        "question" | "?" => "question".into(),
        "space" => "space".into(),
        "del" => "delete".into(),
        other => other.into(),
    }
}

fn pretty(key: &str) -> String {
    match key {
        "question" => "?".into(),
        "space" => "Space".into(),
        k if k.chars().count() == 1 => k.to_ascii_uppercase(),
        k => {
            let mut c = k.chars();
            match c.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    /// An action may have more than one chord; Ctrl+Y and Ctrl+Shift+Z both
    /// redo, and taking that away to simplify the type would be a regression.
    binds: HashMap<Action, Vec<Chord>>,
    /// Modifiers held during a canvas drag. One each, because "hold two
    /// different things to rotate" is not a thing anyone wants.
    modals: HashMap<Modal, Mods>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut binds = HashMap::new();
        let mut set = |action: Action, chords: &[&str]| {
            binds.insert(
                action,
                chords.iter().filter_map(|c| Chord::parse(c)).collect(),
            );
        };
        set(Open, &["Ctrl+O"]);
        set(Export, &["Ctrl+E"]);
        set(Undo, &["Ctrl+Z"]);
        set(Redo, &["Ctrl+Shift+Z", "Ctrl+Y"]);
        set(PlayPause, &["space"]);
        set(SelectAll, &["Ctrl+A"]);
        set(Delete, &["Delete"]);
        set(ShowShortcuts, &["Ctrl+?"]);
        set(ToolText, &["T"]);
        set(ToolRect, &["R"]);
        set(ToolEllipse, &["O"]);
        set(ToolArrow, &["A"]);
        set(ToolCrop, &["C"]);
        set(FrameDelete, &[]);
        set(FrameDuplicate, &["Ctrl+D"]);
        set(FrameCut, &["Ctrl+X"]);
        set(FrameCopy, &["Ctrl+C"]);
        set(FramePaste, &["Ctrl+V"]);
        set(FrameReverse, &[]);
        set(ZoomToSelection, &[]);
        set(StripZoomIn, &["Ctrl+Up", "Ctrl+plus", "Ctrl+equal"]);
        set(StripZoomOut, &["Ctrl+Down", "Ctrl+minus"]);
        set(StripZoomReset, &["Ctrl+0"]);

        // Impasto's transform tool: Alt rotates, Shift keeps the aspect ratio,
        // Ctrl resizes from the center.
        let modals = HashMap::from([
            (
                Modal::Rotate,
                Mods {
                    alt: true,
                    ..Default::default()
                },
            ),
            (
                Modal::KeepAspect,
                Mods {
                    shift: true,
                    ..Default::default()
                },
            ),
            (
                Modal::FromCenter,
                Mods {
                    ctrl: true,
                    ..Default::default()
                },
            ),
        ]);
        Keymap { binds, modals }
    }
}

impl Keymap {
    pub fn load() -> Self {
        let mut map = Keymap::default();
        let Some(path) = path() else { return map };
        match std::fs::read_to_string(&path) {
            Ok(text) => map.apply(&text),
            Err(_) => map.save_to(&path),
        }
        map
    }

    /// A line names an action and everything it is bound to. A line with no
    /// chords unbinds; a line naming an unknown action is ignored, so a file
    /// from a newer version does not lose the bindings it does understand.
    pub fn apply(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if let Some(modal) = MODALS.iter().find(|m| m.id() == key) {
                self.modals.insert(*modal, Mods::parse(value));
                continue;
            }
            let Some(action) = ACTIONS.iter().find(|a| a.id() == key) else {
                continue;
            };
            self.binds
                .insert(*action, value.split(',').filter_map(Chord::parse).collect());
        }
    }

    pub fn chords(&self, action: Action) -> &[Chord] {
        self.binds.get(&action).map_or(&[], Vec::as_slice)
    }

    /// Every action this chord fires, in `ACTIONS` order. More than one is only
    /// legitimate for tools, which cycle.
    pub fn actions_for(&self, chord: &Chord) -> Vec<Action> {
        ACTIONS
            .iter()
            .copied()
            .filter(|a| self.chords(*a).iter().any(|c| c.matches(chord)))
            .collect()
    }

    pub fn set(&mut self, action: Action, chords: Vec<Chord>) {
        self.binds.insert(action, chords);
    }

    pub fn mods(&self, modal: Modal) -> Mods {
        self.modals.get(&modal).copied().unwrap_or_default()
    }

    pub fn set_mods(&mut self, modal: Modal, mods: Mods) {
        self.modals.insert(modal, mods);
    }

    /// Actions whose chord collides with a different action's, excluding the
    /// tool-cycling case. This is what the editor paints red.
    pub fn conflicts(&self) -> Vec<Action> {
        ACTIONS
            .iter()
            .copied()
            .filter(|a| {
                self.chords(*a).iter().any(|c| {
                    self.actions_for(c)
                        .iter()
                        .any(|b| *b != *a && !(a.is_tool() && b.is_tool()))
                })
            })
            .collect()
    }

    /// "Undo (Ctrl+Z)" — tooltips are built, never typed, so a rebind moves
    /// them (AGENTS.md, Keybindings).
    pub fn tip(&self, text: &str, action: Action) -> String {
        match self.chords(action).first() {
            Some(chord) => format!("{text} ({})", chord.display()),
            None => text.to_string(),
        }
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from(
            "# gifkino keybindings\n\
             # One action per line. Separate alternatives with a comma;\n\
             # leave the right side empty to unbind.\n\n",
        );
        for action in ACTIONS {
            let chords: Vec<String> = self.chords(action).iter().map(Chord::display).collect();
            out.push_str(&format!("{} = {}\n", action.id(), chords.join(", ")));
        }
        out.push_str("\n# Modifiers held while dragging an overlay on the canvas.\n");
        for modal in MODALS {
            out.push_str(&format!(
                "{} = {}\n",
                modal.id(),
                self.mods(modal).display()
            ));
        }
        out
    }

    pub fn save(&self) {
        if let Some(path) = path() {
            self.save_to(&path);
        }
    }

    /// Best effort: a read-only config directory is not worth an error dialog
    /// the user cannot act on.
    fn save_to(&self, path: &Path) {
        let Some(dir) = path.parent() else { return };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let _ = std::fs::write(path, self.to_text());
    }
}

pub fn path() -> Option<PathBuf> {
    Some(crate::settings::path()?.with_file_name("keybindings.conf"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_round_trip_through_their_display_form() {
        for text in ["Ctrl+Z", "Ctrl+Shift+Z", "Space", "Delete", "T", "Ctrl+?"] {
            let chord = Chord::parse(text).expect(text);
            assert_eq!(
                Chord::parse(&chord.display()),
                Some(chord.clone()),
                "{text} -> {}",
                chord.display()
            );
        }
        // The file is hand-edited, so casing and spacing must not matter.
        assert_eq!(Chord::parse(" ctrl + z "), Chord::parse("Ctrl+Z"));
        assert_eq!(Chord::parse(""), None);
        assert_eq!(
            Chord::parse("Ctrl"),
            None,
            "a modifier alone is not a chord"
        );
    }

    /// Shift distinguishes Ctrl+Z from Ctrl+Shift+Z, but on punctuation it is
    /// just how the character is typed.
    #[test]
    fn shift_is_part_of_a_letter_chord_and_not_of_a_symbol_one() {
        let undo = Chord::parse("Ctrl+Z").unwrap();
        let redo = Chord::parse("Ctrl+Shift+Z").unwrap();
        assert!(!undo.matches(&redo));
        assert!(undo.matches(&Chord::new(true, false, false, "z")));

        let help = Chord::parse("Ctrl+?").unwrap();
        assert!(help.matches(&Chord::new(true, true, false, "question")));
        assert_eq!(help.display(), "Ctrl+?", "no phantom Shift in the tooltip");
    }

    #[test]
    fn the_defaults_are_the_shortcuts_that_were_hardcoded() {
        let map = Keymap::default();
        let fires = |ctrl, shift, key| map.actions_for(&Chord::new(ctrl, shift, false, key));
        assert_eq!(fires(true, false, "z"), vec![Undo]);
        assert_eq!(fires(true, true, "z"), vec![Redo]);
        assert_eq!(
            fires(true, false, "y"),
            vec![Redo],
            "a second chord still redoes"
        );
        assert_eq!(fires(false, false, "space"), vec![PlayPause]);
        assert_eq!(fires(false, false, "t"), vec![ToolText]);
        // The clipboard keys everyone else uses, on frames.
        assert_eq!(fires(true, false, "x"), vec![FrameCut]);
        assert_eq!(fires(true, false, "c"), vec![FrameCopy]);
        assert_eq!(fires(true, false, "v"), vec![FramePaste]);
        assert!(fires(false, false, "q").is_empty());
    }

    #[test]
    fn a_rebind_moves_the_tooltip_with_it() {
        let mut map = Keymap::default();
        assert_eq!(map.tip("Undo", Undo), "Undo (Ctrl+Z)");
        map.set(Undo, vec![Chord::parse("Ctrl+Shift+U").unwrap()]);
        assert_eq!(map.tip("Undo", Undo), "Undo (Ctrl+Shift+U)");
        // An action with no binding still gets a tooltip, just no hint.
        map.set(Undo, Vec::new());
        assert_eq!(map.tip("Undo", Undo), "Undo");
    }

    /// Two tools on one key cycle; anything else on one key is a mistake worth
    /// pointing at.
    #[test]
    fn duplicates_are_conflicts_unless_both_sides_are_tools() {
        let mut map = Keymap::default();
        assert!(map.conflicts().is_empty(), "{:?}", map.conflicts());

        map.set(ToolRect, vec![Chord::parse("T").unwrap()]);
        assert_eq!(
            map.actions_for(&Chord::new(false, false, false, "t")),
            vec![ToolText, ToolRect]
        );
        assert!(map.conflicts().is_empty(), "tools may share a key");

        map.set(Export, vec![Chord::parse("Ctrl+Z").unwrap()]);
        let conflicts = map.conflicts();
        assert!(
            conflicts.contains(&Export) && conflicts.contains(&Undo),
            "{conflicts:?}"
        );
    }

    /// The canvas modifiers live in the same file as everything else, so a
    /// rebind survives a restart the same way a chord does.
    #[test]
    fn canvas_modifiers_default_to_impastos_and_round_trip() {
        let map = Keymap::default();
        assert_eq!(
            map.mods(Modal::Rotate),
            Mods {
                alt: true,
                ..Default::default()
            }
        );
        assert_eq!(map.mods(Modal::Rotate).display(), "Alt");

        let alt_shift = gdk::ModifierType::ALT_MASK | gdk::ModifierType::SHIFT_MASK;
        assert!(
            map.mods(Modal::Rotate).held(alt_shift),
            "rotate and constrain at once"
        );
        assert!(!map.mods(Modal::Rotate).held(gdk::ModifierType::SHIFT_MASK));
        assert!(!Mods::default().held(alt_shift), "unbound is never held");

        let mut map = Keymap::default();
        map.set_mods(Modal::Rotate, Mods::parse("Ctrl+Shift"));
        let mut reloaded = Keymap::default();
        reloaded.apply(&map.to_text());
        assert_eq!(
            reloaded.mods(Modal::Rotate),
            Mods {
                ctrl: true,
                shift: true,
                alt: false
            }
        );
        assert_eq!(reloaded, map);

        // An empty right side unbinds, and a modal line is not an action line.
        let mut map = Keymap::default();
        map.apply("canvas-rotate =\n");
        assert!(map.mods(Modal::Rotate).is_empty());
    }

    #[test]
    fn the_file_round_trips_and_survives_junk() {
        let mut map = Keymap::default();
        map.set(FrameReverse, vec![Chord::parse("Ctrl+Alt+R").unwrap()]);
        let mut reloaded = Keymap::default();
        reloaded.apply(&map.to_text());
        assert_eq!(reloaded, map);

        let mut map = Keymap::default();
        map.apply("nonsense\n# undo = Ctrl+Q\nnot_an_action = Ctrl+Q\nundo =\n");
        assert!(map.chords(Undo).is_empty(), "an empty right side unbinds");
        assert_eq!(map.chords(Redo).len(), 2, "other actions are untouched");
    }
}
