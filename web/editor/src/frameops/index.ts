// Frame-list operations, effects, resize, frame removal, "Add frames from
// file…" and the sidebar's frame group: the desktop's `win.frame-*`,
// Effects / Image / Optimize menus and their dialogs (actions.rs, update.rs,
// dialogs.rs, imports.rs). Command ids: ARCHITECTURE.md.

import type { App, Command } from "../app";
import { fill, t } from "../i18n";
import { edited, framesInScope } from "../state";
import type { AppState } from "../state";
import { h } from "../ui/dom";
import type { Change, FitMode, FramesMoved } from "../wasm/gifkino_wasm.js";
import { spliceFits } from "./budget";
import {
  delayAllDialog,
  delayScopeDialog,
  fadeDialog,
  fitDialog,
  loopFadeDialog,
  moveFrameDialog,
  removeFramesDialog,
  resizeDialog,
  smartRemoveDialog,
} from "./dialogs";
import { createFramePanel } from "./panel";
import { plural, smartDropCount } from "./summary";

const count = (state: AppState) => state.doc?.frameCount ?? 0;
/** Desktop `sync_actions`: idle and with frames, else greyed. */
const ready = (state: AppState) => state.busy === null && count(state) > 0;

const EFFECT_BUSY = "Applying effect… {done} / {total} frames";
const OVER_MEMORY = "That resize would use more memory than the configured limit.";

export function install(app: App): void {
  // Commands the desktop leaves sensitive but its busy guard drops
  // (`busy_blocks`): runnable from the menu, a no-op while a job runs.
  const command = (spec: Command) =>
    app.registerCommand({
      ...spec,
      run: () => (app.state.busy === null && app.state.doc ? spec.run() : undefined),
    });

  /** Run a frame-heavy edit under the busy bar, then toast it if it was document-wide. */
  const work = async (label: string, frames: number, job: () => Promise<Change | undefined>) => {
    const change = await app.withBusy(t(label), async (report) => {
      report(0, frames);
      return job();
    });
    app.toastChange(change, count(app.state));
  };

  /** Desktop `run_frame_op`: delete, cut, duplicate or reverse the scope. */
  const frameOp = async (
    method: "deleteFrames" | "cutFrames" | "duplicateFrames" | "reverseFrames",
  ) => {
    const frames = framesInScope(app.state);
    if (frames.length === 0) return;
    const total = count(app.state);
    const change = await app.dispatch(method, frames);
    const removes = method === "deleteFrames" || method === "cutFrames";
    app.update((state) => ({
      ...state,
      playhead: removes ? Math.min(frames[0], Math.max(0, count(state) - 1)) : state.playhead,
      selection: [],
      scope: state.scope === "range" ? "frame" : state.scope,
    }));
    app.toastChange(change, total);
  };

  /** Desktop `move_picked`: the playhead rides its frame, a moved run stays selected. */
  const landMove = (picked: number[], moved: FramesMoved | undefined, total: number) => {
    if (!moved) return;
    const { at } = moved;
    app.update((state) => {
      const k = picked.indexOf(state.playhead);
      const run = picked.length > 1;
      return {
        ...state,
        playhead: k >= 0 ? at + k : state.playhead,
        selection: run ? picked.map((_, i) => at + i) : state.selection,
        scope: run ? "range" : state.scope,
      };
    });
    app.toastChange(moved.change, total);
  };

  /** Desktop `ImportInto(at)`: pick a file, decode it, ask the fit question, splice at `at`. */
  const picker = h("input", { type: "file", hidden: true });
  let importAt = 0;
  picker.addEventListener("change", () => {
    const file = picker.files?.[0];
    picker.value = "";
    if (!file) return;
    addFrames(app, file, importAt).catch((error: unknown) => {
      const reason = error instanceof Error ? error.message : String(error);
      app.toast(fill(t("Could not open {name}: {reason}"), { name: file.name, reason }));
    });
  });
  document.body.append(picker);
  const pickImport = (at: number) => {
    importAt = at;
    picker.accept = app.accept;
    picker.click();
  };

  // ---- Frames menu ----
  const clipboard = "clipboard";
  command({
    id: "frame-cut",
    label: "Cut",
    menu: "frames",
    section: clipboard,
    run: () => frameOp("cutFrames"),
  });
  command({
    id: "frame-copy",
    label: "Copy",
    menu: "frames",
    section: clipboard,
    run: async () => {
      const frames = framesInScope(app.state);
      if (frames.length === 0) return;
      // Through dispatch so `doc.clipboardLength` refreshes; no undo step.
      const copied = await app.dispatch("copyFrames", frames);
      app.toast(plural("{count} frame copied", "{count} frames copied", copied));
    },
  });
  command({
    id: "frame-paste",
    label: "Paste",
    menu: "frames",
    section: clipboard,
    run: async () => {
      const pasted = app.state.doc?.clipboardLength ?? 0;
      if (pasted === 0) return;
      const total = count(app.state);
      const at = total === 0 ? 0 : app.state.playhead + 1;
      const change = await app.dispatch("pasteFrames", at);
      // The playhead rides the last pasted frame, so pasting twice stacks
      // the runs; a run of more than one stays selected.
      app.update((state) => ({
        ...state,
        playhead: at + pasted - 1,
        selection: pasted > 1 ? Array.from({ length: pasted }, (_, i) => at + i) : [],
        scope: pasted > 1 ? "range" : state.scope === "range" ? "frame" : state.scope,
      }));
      app.toastChange(change, total);
    },
  });
  const list = "list";
  for (const [id, label, method] of [
    ["frame-delete", "Delete", "deleteFrames"],
    ["frame-duplicate", "Duplicate", "duplicateFrames"],
    ["frame-reverse", "Reverse", "reverseFrames"],
  ] as const) {
    app.registerCommand({
      id,
      label,
      menu: "frames",
      section: list,
      enabled: ready,
      run: () => frameOp(method),
    });
  }
  command({
    id: "frame-delay-all",
    label: "Set delay for all frames…",
    menu: "frames",
    section: "timing",
    run: async () => {
      const doc = app.state.doc!;
      if (doc.frameCount === 0) return;
      const cs = await delayAllDialog(doc.frameCount, doc.frames[0].delayCs);
      if (cs === null) return;
      const total = count(app.state);
      const all = Array.from({ length: total }, (_, i) => i);
      app.toastChange(await app.dispatch("setDelay", all, Math.max(1, cs)), total);
    },
  });

  // ---- The strip's frame menu (no header entry) ----
  command({
    id: "frame-delay",
    label: "Set delay…",
    run: async () => {
      const scope = framesInScope(app.state);
      const frame = app.state.doc!.frames[app.state.playhead];
      if (!frame) return;
      const cs = await delayScopeDialog(scope, frame.delayCs);
      if (cs === null) return;
      app.toastChange(await app.dispatch("setDelay", scope, Math.max(1, cs)), count(app.state));
    },
  });
  for (const [id, label, earlier] of [
    ["frame-move-earlier", "Move earlier", true],
    ["frame-move-later", "Move later", false],
  ] as const) {
    command({
      id,
      label,
      run: async () => {
        const picked = framesInScope(app.state);
        if (picked.length === 0) return;
        landMove(picked, await app.dispatch("nudgeFrames", picked, earlier), count(app.state));
      },
    });
  }
  command({
    id: "frame-move-to",
    label: "Move to position…",
    run: async () => {
      const from = app.state.playhead;
      const total = count(app.state);
      if (from >= total) return;
      const to = await moveFrameDialog(from, total);
      if (to === null || to === from) return;
      const change = await app.dispatch("moveFrame", from, to);
      app.update({ playhead: to });
      app.toastChange(change, total);
    },
  });
  command({
    id: "frame-insert-file",
    label: "Insert frames from file…",
    // Directly after the frame the menu was opened on.
    run: () => pickImport(app.state.playhead + 1),
  });
  app.registerCommand({
    id: "delete",
    label: "Delete",
    enabled: (state) => state.busy === null,
    run: async () => {
      const { selectedOverlay, selection } = app.state;
      if (selectedOverlay !== null) {
        const change = await app.dispatch("deleteOverlay", selectedOverlay);
        app.update({ selectedOverlay: null });
        app.toastChange(change, count(app.state));
        return;
      }
      if (selection.length === 0) return;
      const total = count(app.state);
      const change = await app.dispatch("deleteFrames", selection);
      app.update({ selection: [], scope: "frame" });
      app.toastChange(change, total);
    },
  });

  // ---- File menu ----
  app.registerCommand({
    id: "import-more",
    label: "Add frames from file…",
    menu: "file",
    enabled: (state) => state.busy === null,
    run: () => pickImport(count(app.state)),
  });

  // ---- Effects ----
  for (const [id, label, axis] of [
    ["mirror-horizontal", "Mirror horizontally", "horizontal"],
    ["mirror-vertical", "Mirror vertically", "vertical"],
  ] as const) {
    app.registerCommand({
      id,
      label,
      menu: "effects",
      section: "mirror",
      enabled: ready,
      run: async () => {
        const frames = framesInScope(app.state);
        if (frames.length === 0) return;
        await work(EFFECT_BUSY, frames.length, () => app.dispatch("mirror", frames, axis));
      },
    });
  }
  for (const [id, label, fade] of [
    ["fade-in", "Fade in…", "in"],
    ["fade-out", "Fade out…", "out"],
  ] as const) {
    app.registerCommand({
      id,
      label,
      menu: "effects",
      section: "fades",
      enabled: ready,
      run: async () => {
        const frames = framesInScope(app.state);
        const colour = await fadeDialog(fade, frames.length);
        if (colour === null) return;
        await work(EFFECT_BUSY, frames.length, () => app.dispatch("fade", frames, fade, colour));
      },
    });
  }
  app.registerCommand({
    id: "loop-fade",
    label: "Cross-fade the loop…",
    menu: "effects",
    section: "fades",
    enabled: ready,
    run: async () => {
      const limit = app.state.doc!.loopFadeLimit;
      const blend = await loopFadeDialog(limit);
      if (blend === null) return;
      await work(EFFECT_BUSY, Math.min(blend, limit), () => app.dispatch("loopFade", blend));
    },
  });

  // ---- Image ----
  app.registerCommand({
    id: "optimize-resize",
    label: "Resize frames…",
    menu: "image",
    enabled: ready,
    run: async () => {
      const { width, height, frameCount } = app.state.doc!;
      const size = await resizeDialog(width, height, frameCount);
      if (!size) return;
      const [w, h] = size;
      if (w === width && h === height) return;
      // Translators: Progress while every frame is being resized; both values are frame counts.
      await work("Resizing… {done} / {total} frames", frameCount, () =>
        app.dispatch("resize", w, h),
      );
    },
  });

  // ---- Optimize ----
  app.registerCommand({
    id: "optimize-remove",
    label: "Remove frames…",
    menu: "optimize",
    enabled: ready,
    run: async () => {
      const every = await removeFramesDialog();
      const total = count(app.state);
      if (every === null || every < 2 || total === 0) return;
      const change = await app.dispatch("dropEveryNth", every);
      app.update({ playhead: 0 });
      app.toastChange(change, total);
    },
  });
  app.registerCommand({
    id: "optimize-smart",
    label: "Smart remove frames…",
    menu: "optimize",
    enabled: ready,
    run: async () => {
      const percent = await smartRemoveDialog();
      const total = count(app.state);
      if (percent === null || smartDropCount(total, percent) === 0) return;
      const change = await app.dispatch("smartDrop", percent);
      app.update({ playhead: 0 });
      app.toastChange(change, total);
    },
  });

  app.registerPanel({ slot: "sidebar", id: "frame", order: 0, element: createFramePanel(app) });
}

/**
 * Desktop `load_into` → `ask_fit` → `start_splice` → `land_splice`: frames
 * from `file` into the open document before frame `at`. An empty document
 * just opens it.
 */
async function addFrames(app: App, file: File, at: number): Promise<void> {
  if (count(app.state) === 0) return app.openFile(file);
  const staged = await app.stageFile(file);
  if (!staged) return;
  const doc = app.state.doc!;
  at = Math.min(at, doc.frameCount);
  let fit: FitMode | null = "stretch";
  if (staged.needsFit) {
    fit = await fitDialog(file.name, [doc.width, doc.height], [staged.width, staged.height]);
    if (fit === null) return;
  }
  const [w, h] = await app.compute(
    "fitCanvas",
    fit,
    doc.width,
    doc.height,
    staged.width,
    staged.height,
  );
  if (!spliceFits(w, h, doc.frameCount + staged.length)) {
    app.toast(t(OVER_MEMORY));
    return;
  }
  // Translators: Progress while frames being spliced in are resized to one canvas; both values are frame counts.
  const change = await app.withBusy(t("Fitting… {done} / {total} frames"), async (report) => {
    report(0, staged.length);
    const reply = await app.request("splice", { at, fit }, { snapshot: true });
    app.update((state) => ({ ...edited(state, reply.doc!), playhead: at }));
    return reply.value;
  });
  // Always toasted, unlike a scoped edit: the frames land off the playhead's
  // frame, and a grown canvas redrew every one of them.
  app.toastChange(change);
}
