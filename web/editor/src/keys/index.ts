// Keyboard shortcuts: one window-level listener reading the live keymap
// (desktop `install_shortcuts`), the chords tooltips and menus show, the canvas
// drag modifiers, and Ctrl+? — the shortcuts window that is also the editor.

import type { App } from "../app";
import { n } from "../i18n";
import { openShortcutsDialog } from "./dialog";
import {
  actionsFor,
  applyKeymapText,
  chordFromEvent,
  chordLabel,
  defaultKeymap,
  keymapText,
} from "./keymap";
import type { Keymap } from "./keymap";

const STORAGE_KEY = "gifkino/keybindings.conf";

/** Desktop `Keymap::load`: the stored file over the defaults; a first run stores the defaults. */
function loadKeymap(): Keymap {
  const text = localStorage.getItem(STORAGE_KEY);
  if (text === null) localStorage.setItem(STORAGE_KEY, keymapText(defaultKeymap()));
  return applyKeymapText(defaultKeymap(), text ?? "");
}

/** The live keymap, and the one way to change it: apply, store, refresh every hint. */
export interface KeymapHandle {
  get(): Keymap;
  set(keymap: Keymap): void;
}

export function install(app: App): void {
  let keymap = loadKeymap();
  const handle: KeymapHandle = {
    get: () => keymap,
    set: (next) => {
      keymap = next;
      localStorage.setItem(STORAGE_KEY, keymapText(next));
      // Tooltips are rebuilt by store subscribers, so a no-op update moves them.
      app.update({});
    },
  };

  app.shortcutFor = (commandId) => {
    const chord = keymap.binds[commandId]?.[0];
    return chord ? chordLabel(chord) : null;
  };
  app.modifiersFor = (modal) => keymap.modals[modal];

  app.registerCommand({
    id: "show-shortcuts",
    label: n("Keyboard shortcuts…"),
    menu: "app",
    run: () => openShortcutsDialog(app, handle),
  });

  // Where the tool cycle is up to, when several tools share a key.
  let cycle = 0;
  addEventListener(
    "keydown",
    (event) => {
      // Cmd chords belong to the browser and the OS; the keymap has no Super.
      if (event.defaultPrevented || event.metaKey || focusOwnsKeys(event.target)) return;
      const chord = chordFromEvent(event);
      const actions = chord ? actionsFor(keymap, chord) : [];
      if (actions.length === 0) return;
      if (actions.length > 1) cycle = (cycle + 1) % actions.length;
      event.preventDefault();
      void app.runCommand(actions.length > 1 ? actions[cycle] : actions[0]);
    },
    // Capture, so a focused button cannot take Space (play/pause) first.
    { capture: true },
  );
}

/** Field types that do not take typing, so they leave keys to the shortcuts. */
const KEYLESS_INPUTS: Record<string, true> = {
  button: true,
  checkbox: true,
  color: true,
  file: true,
  radio: true,
  range: true,
  reset: true,
  submit: true,
};

/**
 * Desktop `focus_owns_keys`: a text field keeps its keystrokes (typing "a" in
 * a caption must not add an arrow), and so does anything in a dialog. A button
 * does not: it would eat Space, which is play/pause.
 */
function focusOwnsKeys(target: EventTarget | null): boolean {
  if (document.querySelector("dialog:modal")) return true;
  if (!(target instanceof HTMLElement)) return false;
  if (target instanceof HTMLInputElement) return !KEYLESS_INPUTS[target.type];
  return target.isContentEditable || target.closest("textarea, select, dialog") !== null;
}
