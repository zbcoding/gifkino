import { describe, expect, test } from "vite-plus/test";
import {
  ACTIONS,
  actionsFor,
  applyKeymapText,
  chordFromEvent,
  chordLabel,
  conflicts,
  defaultKeymap,
  keymapText,
  modsFromEvent,
  parseChord,
} from "./keymap";

const press = (key: string, mods: { ctrl?: boolean; shift?: boolean; alt?: boolean } = {}) => ({
  key,
  ctrlKey: !!mods.ctrl,
  shiftKey: !!mods.shift,
  altKey: !!mods.alt,
});

describe("keymap", () => {
  // The file is shared with the desktop, so a desktop keybindings.conf has to
  // read back as the same bindings here, and ours as the same text there.
  test("the default file round-trips to the default keymap", () => {
    const text = keymapText(defaultKeymap());
    expect(text).toContain("undo = Ctrl+Z\n");
    expect(text).toContain("redo = Ctrl+Shift+Z, Ctrl+Y\n");
    expect(text).toContain("show-shortcuts = Ctrl+?\n");
    expect(text).toContain("frame-delete = \n");
    expect(text).toContain("canvas-rotate = Alt\n");
    expect(applyKeymapText(defaultKeymap(), text)).toEqual(defaultKeymap());
  });

  test("a line rebinds or unbinds; unknown actions and junk are ignored", () => {
    const map = applyKeymapText(
      defaultKeymap(),
      "undo = Ctrl+U\nredo =\nfrom-the-future = Ctrl+Q\nnonsense\ncanvas-rotate = Ctrl+Shift\n",
    );
    expect(map.binds.undo.map(chordLabel)).toEqual(["Ctrl+U"]);
    expect(map.binds.redo).toEqual([]);
    expect(map.binds["play-pause"].map(chordLabel)).toEqual(["Space"]);
    expect(map.modals["canvas-rotate"]).toEqual({ ctrl: true, shift: true, alt: false });
  });

  // A browser names keys differently from GDK; the keymap has to see the
  // desktop's names or the shared file would never match a keypress.
  test("browser key events become the desktop's chords", () => {
    const map = defaultKeymap();
    expect(actionsFor(map, chordFromEvent(press("z", { ctrl: true }))!)).toEqual(["undo"]);
    expect(actionsFor(map, chordFromEvent(press("Z", { ctrl: true, shift: true }))!)).toEqual([
      "redo",
    ]);
    expect(actionsFor(map, chordFromEvent(press(" "))!)).toEqual(["play-pause"]);
    expect(actionsFor(map, chordFromEvent(press("Delete"))!)).toEqual(["delete"]);
    expect(actionsFor(map, chordFromEvent(press("ArrowUp", { ctrl: true }))!)).toEqual([
      "strip-zoom-in",
    ]);
    expect(actionsFor(map, chordFromEvent(press("=", { ctrl: true }))!)).toEqual(["strip-zoom-in"]);
    expect(chordFromEvent(press("Control", { ctrl: true }))).toBeNull();
  });

  // Ctrl+? is Ctrl+Shift+slash on most layouts: demanding no Shift would make it unreachable.
  test("shift only matters on letters and digits", () => {
    const map = defaultKeymap();
    expect(actionsFor(map, chordFromEvent(press("?", { ctrl: true, shift: true }))!)).toEqual([
      "show-shortcuts",
    ]);
    expect(actionsFor(map, chordFromEvent(press("+", { ctrl: true, shift: true }))!)).toEqual([
      "strip-zoom-in",
    ]);
    expect(actionsFor(map, chordFromEvent(press("Z", { shift: true, ctrl: true }))!)).not.toContain(
      "undo",
    );
    expect(chordLabel(parseChord("Ctrl+Shift+?")!)).toBe("Ctrl+?");
  });

  test("a bare modifier press is a canvas modifier; any other key is not", () => {
    expect(modsFromEvent(press("Alt", { alt: true }))).toEqual({
      ctrl: false,
      shift: false,
      alt: true,
    });
    expect(modsFromEvent(press("Control", { ctrl: true }))?.ctrl).toBe(true);
    expect(modsFromEvent(press("a"))).toBeNull();
  });

  // The editor paints these red; tools sharing a key cycle instead (AGENTS.md).
  test("conflicts flag a shared chord except between tools", () => {
    const base = defaultKeymap();
    expect(conflicts(base).size).toBe(0);
    const clash = applyKeymapText(base, "undo = Ctrl+D\n");
    expect([...conflicts(clash)].sort()).toEqual(["frame-duplicate", "undo"]);
    const cycle = applyKeymapText(base, "tool-rectangle = T\n");
    expect(conflicts(cycle).size).toBe(0);
    expect(actionsFor(cycle, parseChord("T")!)).toEqual(["tool-text", "tool-rectangle"]);
  });

  test("every desktop action is listed once", () => {
    expect(new Set(ACTIONS.map(({ id }) => id)).size).toBe(29);
  });
});
