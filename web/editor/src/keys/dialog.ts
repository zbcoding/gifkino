// Desktop `shortcuts_dialog`: the Ctrl+? shortcuts window, which is also the
// keybinding editor — one list that shows what a key does and changes it.

import { download } from "../app";
import type { App } from "../app";
import { fill, t } from "../i18n";
import { MODALS, modsLabel } from "../modals";
import type { Modal } from "../modals";
import { alertDialog } from "../ui/dialog";
import { h, icon } from "../ui/dom";
import { CLEAR } from "../ui/icons";
import { actionRow, group, setSubtitle, suffix } from "../ui/prefs";
import type { KeymapHandle } from ".";
import {
  ACTIONS,
  applyKeymapText,
  chordFromEvent,
  chordLabel,
  conflicts,
  defaultKeymap,
  keymapText,
  modsFromEvent,
} from "./keymap";
import type { Keymap } from "./keymap";

const FILE_NAME = "keybindings.conf";

/** Which row waits for a keypress: an action's chord or a canvas modifier. */
type Capture = { action: string } | { modal: Modal } | null;

interface BindingRow {
  row: HTMLElement;
  button: HTMLButtonElement;
}

/** A row titled `title`: the button that shows the binding and starts a capture, then Unbind. */
function bindingRow(title: string, capture: () => void, unbind: () => void): BindingRow {
  const button = h("button", { class: "bind-button", onclick: capture });
  const clear = h(
    "button",
    { class: "flat icon-button", title: t("Unbind"), onclick: unbind },
    icon(CLEAR),
  );
  return { row: actionRow(title, suffix(button, clear)), button };
}

/** A `group` with an `adw::PreferencesGroup` description under its title. */
function describedGroup(title: string, description: string, ...rows: Node[]): HTMLElement {
  const section = group(title, ...rows);
  const heading = section.querySelector(".pref-heading")!;
  heading.after(h("p", { class: "group-description" }, description));
  return section;
}

export function openShortcutsDialog(app: App, keymap: KeymapHandle): void {
  let capturing: Capture = null;
  const actionRows = new Map<string, BindingRow>();
  const modalRows = new Map<Modal, BindingRow>();

  /** Desktop `BindingEditor::refresh`: every binding, the capture prompt, the conflict marks. */
  const refresh = () => {
    const map = keymap.get();
    const clashes = conflicts(map);
    for (const [id, { row, button }] of actionRows) {
      const chords = map.binds[id].map(chordLabel).join(", ");
      const waiting = capturing && "action" in capturing && capturing.action === id;
      button.textContent = waiting ? t("Press a key…") : chords || t("Unassigned");
      button.classList.toggle("bind-conflict", clashes.has(id));
      setSubtitle(row, clashes.has(id) ? t("Already used by another action") : "");
    }
    for (const [id, { button }] of modalRows) {
      const mods = modsLabel(map.modals[id]);
      const waiting = capturing && "modal" in capturing && capturing.modal === id;
      // Translators: A canvas modifier row waiting for the user to hold a key.
      button.textContent = waiting ? t("Hold a modifier…") : mods || t("Unassigned");
    }
  };
  /** Desktop `BindingEditor::change`: every edit applies, stores and redraws in one path. */
  const change = (next: Keymap) => {
    keymap.set(next);
    refresh();
  };
  const startCapture = (capture: Capture) => {
    capturing = capture;
    refresh();
  };

  const groups = new Map<string, HTMLElement[]>();
  for (const { id, label, group: name } of ACTIONS) {
    const binding = bindingRow(
      t(label),
      () => startCapture({ action: id }),
      () => change({ ...keymap.get(), binds: { ...keymap.get().binds, [id]: [] } }),
    );
    actionRows.set(id, binding);
    groups.set(name, [...(groups.get(name) ?? []), binding.row]);
  }
  for (const { id, label } of MODALS) {
    const unbound = { ctrl: false, shift: false, alt: false };
    const binding = bindingRow(
      t(label),
      () => startCapture({ modal: id }),
      () => change({ ...keymap.get(), modals: { ...keymap.get().modals, [id]: unbound } }),
    );
    modalRows.set(id, binding);
  }

  const picker = h("input", { type: "file", hidden: true });
  picker.addEventListener("change", async () => {
    const file = picker.files?.[0];
    picker.value = "";
    if (!file) return;
    try {
      change(applyKeymapText(keymap.get(), await file.text()));
    } catch (error) {
      app.toast(
        fill(t("Could not read {path}: {error}"), { path: file.name, error: String(error) }),
      );
    }
  });
  const exportFile = () => {
    download(new Blob([keymapText(keymap.get())], { type: "text/plain" }), FILE_NAME);
    app.toast(fill(t("Saved {path}"), { path: FILE_NAME }));
  };
  const fileRow = actionRow(
    t("Import or export"),
    suffix(
      h("button", { title: t("Import keybindings"), onclick: () => picker.click() }, t("Import…")),
      h("button", { title: t("Export keybindings"), onclick: exportFile }, t("Export…")),
      h("button", { onclick: () => change(defaultKeymap()) }, t("Reset")),
    ),
    FILE_NAME,
  );

  const content = h(
    "div",
    { class: "shortcuts-editor" },
    ...[...groups].map(([name, rows]) => group(t(name), ...rows)),
    describedGroup(
      t("Canvas"),
      t("Held while dragging an overlay's grip."),
      ...[...modalRows.values()].map(({ row }) => row),
    ),
    describedGroup(t("Keybindings file"), t("Bindings are saved as you change them."), fileRow),
    picker,
  );
  refresh();
  void alertDialog({
    title: t("Keyboard shortcuts"),
    content,
    // Translators: Dismisses a dialog that only explains something.
    responses: [{ id: "close", label: t("Close") }],
    close: "close",
  }).answer;

  // Desktop `BindingEditor::capture`, in the capture phase so the dialog's own
  // buttons never see the keystroke being bound.
  content.closest("dialog")!.addEventListener(
    "keydown",
    (event) => {
      if (!capturing) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") return startCapture(null);
      if ("modal" in capturing) {
        // Anything but a modifier is not an answer to this question.
        const mods = modsFromEvent(event);
        if (!mods) return;
        const modal = capturing.modal;
        capturing = null;
        return change({ ...keymap.get(), modals: { ...keymap.get().modals, [modal]: mods } });
      }
      const chord = event.metaKey ? null : chordFromEvent(event);
      if (!chord) return;
      const action = capturing.action;
      capturing = null;
      change({ ...keymap.get(), binds: { ...keymap.get().binds, [action]: [chord] } });
    },
    { capture: true },
  );
}
