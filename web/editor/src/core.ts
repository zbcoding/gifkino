// The shell's own commands and panels: open, export, history, playback and
// the scope toggle. Registered the same way a slice registers its own.

import type { App } from "./app";
import { fill, n, t } from "./i18n";
import { togglePlay } from "./playback";
import type { AppState } from "./state";
import { h } from "./ui/dom";
import { createScopeButtons } from "./ui/transport";

const idle = (state: AppState) => state.busy === null;
const loaded = (state: AppState) => idle(state) && (state.doc?.frameCount ?? 0) > 0;

/** Desktop `sync_tooltips`: "Undo Frames deleted", or plain "Undo" with nothing to undo. */
function historyTip(template: string, bare: string, label: string | null): string {
  return label ? fill(t(template), { change: t(label) }) : t(bare);
}

export function installCore(app: App): void {
  const picker = h("input", { type: "file", hidden: true });
  picker.addEventListener("change", () => {
    const file = picker.files?.[0];
    picker.value = "";
    if (file) void openFile(app, file);
  });
  document.body.append(picker);

  app.registerCommand({
    id: "open",
    label: "Open…",
    menu: "file",
    enabled: idle,
    run: () => {
      picker.accept = app.accept;
      picker.click();
    },
  });
  app.registerCommand({
    id: "export",
    label: "Export GIF…",
    menu: "file",
    enabled: loaded,
    tooltip: () => t("Export GIF"),
    run: () => app.exportGif(),
  });
  app.registerCommand({
    id: "undo",
    label: "Undo",
    enabled: (state) => idle(state) && !!state.doc?.canUndo,
    // Translators: Undo button tooltip. {change} is the name of the edit, e.g. "Frames deleted".
    tooltip: (state) => historyTip(n("Undo {change}"), "Undo", state.doc?.undoLabel ?? null),
    run: () => app.dispatch("undo"),
  });
  app.registerCommand({
    id: "redo",
    label: "Redo",
    enabled: (state) => idle(state) && !!state.doc?.canRedo,
    // Translators: Redo button tooltip. {change} is the name of the edit, e.g. "Frames deleted".
    tooltip: (state) => historyTip(n("Redo {change}"), "Redo", state.doc?.redoLabel ?? null),
    run: () => app.dispatch("redo"),
  });
  app.registerCommand({
    id: "play-pause",
    label: "Play/pause",
    enabled: (state) => (state.doc?.frameCount ?? 0) > 1,
    run: () => togglePlay(app.store),
  });

  app.registerPanel({ slot: "transport", id: "scope", order: 0, element: createScopeButtons(app) });

  // Drop a file anywhere on the window to open it.
  addEventListener("dragover", (event) => event.preventDefault());
  addEventListener("drop", (event) => {
    event.preventDefault();
    const file = event.dataTransfer?.files[0];
    if (file && idle(app.state)) void openFile(app, file);
  });

  addEventListener("beforeunload", (event) => {
    if (app.state.dirty) event.preventDefault();
  });
}

/** Open through the importers, reporting a file that will not decode. */
async function openFile(app: App, file: File): Promise<void> {
  try {
    await app.openFile(file);
  } catch (error) {
    const reason = error instanceof Error ? error.message : String(error);
    // Translators: Toast when a file will not open. {name} is the file name, {reason} the error.
    app.toast(fill(t("Could not open {name}: {reason}"), { name: file.name, reason }));
  }
}
