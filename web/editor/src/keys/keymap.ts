// Rebindable keyboard shortcuts: desktop `keymap.rs`. The file format is the
// desktop's flat `action = Ctrl+Z` text, so a keybindings.conf moves between
// the desktop and the browser unchanged.

import { DEFAULT_MODIFIERS, MODALS, modsLabel } from "../modals";
import type { Modal, Mods } from "../modals";
import { entries } from "../settings/conf";

export interface Action {
  /** Desktop `Action::id`: the config key and the command id it runs. */
  id: string;
  /** Untranslated desktop label, shown in the shortcuts window. */
  label: string;
  /** Untranslated shortcuts-window group. */
  group: string;
}

const action = (id: string, label: string, group: string): Action => ({ id, label, group });

/** Desktop `keymap::ACTIONS`, in the order the shortcuts window lists them. */
export const ACTIONS: readonly Action[] = [
  action("open", "Open", "File"),
  action("export", "Export GIF", "File"),
  action("undo", "Undo", "Edit"),
  action("redo", "Redo", "Edit"),
  action("play-pause", "Play/pause", "Edit"),
  action("select-all", "Select all frames", "Edit"),
  action("delete", "Delete selection", "Edit"),
  action("show-shortcuts", "Keyboard shortcuts", "Edit"),
  action("tool-text", "Text tool", "Tools"),
  action("tool-rectangle", "Rectangle tool", "Tools"),
  action("tool-ellipse", "Ellipse tool", "Tools"),
  action("tool-arrow", "Arrow tool", "Tools"),
  action("tool-crop", "Crop tool", "Tools"),
  action("toggle-snap", "Snap to edges and center", "Tools"),
  action("frame-delete", "Delete frames", "Frames"),
  action("frame-duplicate", "Duplicate frames", "Frames"),
  action("frame-cut", "Cut frames", "Frames"),
  action("frame-copy", "Copy frames", "Frames"),
  action("frame-paste", "Paste frames", "Frames"),
  action("frame-reverse", "Reverse frames", "Frames"),
  action("zoom-to-selection", "Zoom this frame", "Frames"),
  action("strip-zoom-in", "Zoom in the frame strip", "Frames"),
  action("strip-zoom-out", "Zoom out the frame strip", "Frames"),
  action("strip-zoom-reset", "Reset the frame strip zoom", "Frames"),
  action("mirror-horizontal", "Mirror horizontally", "Effects"),
  action("mirror-vertical", "Mirror vertically", "Effects"),
  action("fade-in", "Fade in…", "Effects"),
  action("fade-out", "Fade out…", "Effects"),
  action("loop-fade", "Cross-fade the loop…", "Effects"),
];

/** Desktop `Action::is_tool`: tools share a key on purpose, and cycle. */
const TOOLS: Record<string, true> = {
  "tool-text": true,
  "tool-rectangle": true,
  "tool-ellipse": true,
  "tool-arrow": true,
  "tool-crop": true,
};

/** One chord. `key` is the GDK key name, lowercased: `z`, `space`, `question`, `up`. */
export interface Chord {
  ctrl: boolean;
  shift: boolean;
  alt: boolean;
  key: string;
}

export interface Keymap {
  /** An action may have several chords: Ctrl+Shift+Z and Ctrl+Y both redo. */
  binds: Record<string, Chord[]>;
  modals: Record<Modal, Mods>;
}

const DEFAULT_CHORDS: Record<string, string[]> = {
  open: ["Ctrl+O"],
  export: ["Ctrl+E"],
  undo: ["Ctrl+Z"],
  redo: ["Ctrl+Shift+Z", "Ctrl+Y"],
  "play-pause": ["space"],
  "select-all": ["Ctrl+A"],
  delete: ["Delete"],
  "show-shortcuts": ["Ctrl+?"],
  "tool-text": ["T"],
  "tool-rectangle": ["R"],
  "tool-ellipse": ["O"],
  "tool-arrow": ["A"],
  "tool-crop": ["C"],
  "toggle-snap": ["Ctrl+Alt+G"],
  "frame-duplicate": ["Ctrl+D"],
  "frame-cut": ["Ctrl+X"],
  "frame-copy": ["Ctrl+C"],
  "frame-paste": ["Ctrl+V"],
  "strip-zoom-in": ["Ctrl+Up", "Ctrl+plus", "Ctrl+equal"],
  "strip-zoom-out": ["Ctrl+Down", "Ctrl+minus"],
  "strip-zoom-reset": ["Ctrl+0"],
};

export function defaultKeymap(): Keymap {
  const binds = Object.fromEntries(
    ACTIONS.map(({ id }) => [id, (DEFAULT_CHORDS[id] ?? []).flatMap((c) => parseChord(c) ?? [])]),
  );
  return { binds, modals: { ...DEFAULT_MODIFIERS } };
}

/**
 * The browser's names for keys GDK spells out. A chord stores GDK's name so
 * the file reads the same as the desktop's.
 */
const GDK_NAMES: Record<string, string> = {
  " ": "space",
  "?": "question",
  "+": "plus",
  "-": "minus",
  "=": "equal",
  "/": "slash",
  "\\": "backslash",
  ",": "comma",
  ".": "period",
  ";": "semicolon",
  ":": "colon",
  "'": "apostrophe",
  '"': "quotedbl",
  "`": "grave",
  "~": "asciitilde",
  "!": "exclam",
  "@": "at",
  "#": "numbersign",
  $: "dollar",
  "%": "percent",
  "^": "asciicircum",
  "&": "ampersand",
  "*": "asterisk",
  "(": "parenleft",
  ")": "parenright",
  _: "underscore",
  "[": "bracketleft",
  "]": "bracketright",
  "{": "braceleft",
  "}": "braceright",
  "|": "bar",
  "<": "less",
  ">": "greater",
  ArrowUp: "up",
  ArrowDown: "down",
  ArrowLeft: "left",
  ArrowRight: "right",
  Enter: "return",
};

/** `KeyboardEvent.key` values that are the user still pressing a modifier, or no key at all. */
const NOT_KEYS: Record<string, true> = {
  Control: true,
  Shift: true,
  Alt: true,
  AltGraph: true,
  Meta: true,
  OS: true,
  CapsLock: true,
  Dead: true,
  Unidentified: true,
  Process: true,
};

/** Desktop `normalize`: the stored spelling of a key name from the file. */
function normalize(name: string): string {
  const trimmed = name.trim();
  const lower = trimmed.toLowerCase();
  if (lower === "del") return "delete";
  return GDK_NAMES[trimmed] ?? lower;
}

/** A browser key name as GDK's lowercased one: `PageUp` → `page_up`, `F5` → `f5`. */
function gdkKey(key: string): string | null {
  if (NOT_KEYS[key]) return null;
  const named = GDK_NAMES[key];
  if (named) return named;
  if (/^.$/u.test(key)) return key.toLowerCase();
  return key.replace(/([a-z])([A-Z])/g, "$1_$2").toLowerCase();
}

/** Desktop `Mods::add`: hold the modifier `token` names; false when it names none. */
function addModifier(mods: Mods, token: string): boolean {
  switch (token.toLowerCase()) {
    case "ctrl":
    case "control":
      mods.ctrl = true;
      return true;
    case "shift":
      mods.shift = true;
      return true;
    case "alt":
    case "meta":
      mods.alt = true;
      return true;
    default:
      return false;
  }
}

const noMods = (): Mods => ({ ctrl: false, shift: false, alt: false });

const parts = (text: string) =>
  text
    .split("+")
    .map((part) => part.trim())
    .filter((part) => part !== "");

/** Desktop `Mods::parse`: `Alt+Shift`; anything unrecognised is ignored. */
export function parseMods(text: string): Mods {
  const mods = noMods();
  for (const part of parts(text)) addModifier(mods, part);
  return mods;
}

/** Desktop `Chord::parse`: `Ctrl+Shift+Z`; `null` with no key. */
export function parseChord(text: string): Chord | null {
  const mods = noMods();
  let key = "";
  for (const part of parts(text)) if (!addModifier(mods, part)) key = normalize(part);
  return key ? { ...mods, key } : null;
}

type KeyEventLike = Pick<KeyboardEvent, "key" | "ctrlKey" | "shiftKey" | "altKey">;

/** Desktop `Chord::from_event`; `null` for a bare modifier. */
export function chordFromEvent(event: KeyEventLike): Chord | null {
  const key = gdkKey(event.key);
  return key ? { ctrl: event.ctrlKey, shift: event.shiftKey, alt: event.altKey, key } : null;
}

/** Desktop `Mods::from_event`: the modifiers down when a modifier key is pressed; `null` for any other key. */
export function modsFromEvent(event: KeyEventLike): Mods | null {
  const mods = { ctrl: event.ctrlKey, shift: event.shiftKey, alt: event.altKey };
  return addModifier(mods, event.key === "Control" ? "ctrl" : event.key) ? mods : null;
}

/**
 * Shift is only part of a letter or digit chord. On punctuation the shift is
 * how the character is typed at all: Ctrl+? is Ctrl+Shift+slash on most layouts.
 */
function shiftMatters(chord: Chord): boolean {
  return /^[a-z0-9]$/.test(chord.key);
}

export function chordMatches(a: Chord, b: Chord): boolean {
  return (
    a.ctrl === b.ctrl &&
    a.alt === b.alt &&
    a.key === b.key &&
    (!shiftMatters(a) || a.shift === b.shift)
  );
}

/** Desktop `pretty`: `?`, `Space`, `Z`, `Delete`. */
function prettyKey(key: string): string {
  if (key === "question") return "?";
  if (key === "space") return "Space";
  if (/^.$/u.test(key)) return key.toUpperCase();
  return key[0].toUpperCase() + key.slice(1);
}

/** Desktop `Chord::display`: what tooltips, menus and the shortcuts window show. */
export function chordLabel(chord: Chord): string {
  const mods = [
    chord.ctrl && "Ctrl+",
    chord.alt && "Alt+",
    chord.shift && shiftMatters(chord) && "Shift+",
  ];
  return mods.filter(Boolean).join("") + prettyKey(chord.key);
}

/** Every action `chord` fires, in `ACTIONS` order. More than one is only legitimate for tools. */
export function actionsFor(keymap: Keymap, chord: Chord): string[] {
  return ACTIONS.filter(({ id }) => keymap.binds[id]?.some((c) => chordMatches(c, chord))).map(
    ({ id }) => id,
  );
}

/** Desktop `Keymap::conflicts`: actions sharing a chord with another action, tool cycles excepted. */
export function conflicts(keymap: Keymap): Set<string> {
  const clashing = ACTIONS.filter(({ id }) =>
    keymap.binds[id].some((chord) =>
      actionsFor(keymap, chord).some((other) => other !== id && !(TOOLS[id] && TOOLS[other])),
    ),
  );
  return new Set(clashing.map(({ id }) => id));
}

/**
 * Desktop `Keymap::apply`: a line names an action and all its chords; an
 * empty right side unbinds; an unknown action is ignored, so a file from a
 * newer version keeps the bindings it does understand.
 */
export function applyKeymapText(keymap: Keymap, text: string): Keymap {
  const next: Keymap = { binds: { ...keymap.binds }, modals: { ...keymap.modals } };
  for (const [key, value] of entries(text)) {
    const modal = MODALS.find(({ id }) => id === key);
    if (modal) next.modals[modal.id] = parseMods(value);
    else if (key in next.binds)
      next.binds[key] = value.split(",").flatMap((c) => parseChord(c) ?? []);
  }
  return next;
}

/** Desktop `Keymap::to_text`. */
export function keymapText(keymap: Keymap): string {
  const binds = ACTIONS.map(({ id }) => `${id} = ${keymap.binds[id].map(chordLabel).join(", ")}\n`);
  const modals = MODALS.map(({ id }) => `${id} = ${modsLabel(keymap.modals[id])}\n`);
  return (
    "# gifkino keybindings\n" +
    "# One action per line. Separate alternatives with a comma;\n" +
    "# leave the right side empty to unbind.\n\n" +
    binds.join("") +
    "\n# Modifiers held while dragging an overlay on the canvas.\n" +
    modals.join("")
  );
}
