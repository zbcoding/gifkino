// Overlay edits, as the desktop's `actions.rs` makes them: each one dispatch,
// then the pick and the toast the desktop gives it.

import type { App } from "../app";
import { t } from "../i18n";
import { framesInScope } from "../state";
import type { AppState, Store } from "../state";
import type { Overlay, OverlayKind, Tool } from "../wasm/gifkino_wasm.js";
import { leaveCrop } from "./local";
import type { Local } from "./local";

/** Untranslated: these are also the names stored on new overlays. */
export const TOOL_LABELS: Record<Tool, string> = {
  text: "Text",
  rect: "Rectangle",
  ellipse: "Ellipse",
  arrow: "Arrow",
};

export function selectedOverlay(state: AppState): Overlay | undefined {
  return state.doc?.overlays.find((overlay) => overlay.id === state.selectedOverlay);
}

/** Desktop `overlays_on(playhead)` reversed: the layer list, topmost first. */
export function stackedOverlays(state: AppState): Overlay[] {
  const onFrame = (state.doc?.overlays ?? []).filter(
    ({ range }) => range.start <= state.playhead && state.playhead < range.end,
  );
  return onFrame.reverse();
}

/** Desktop `editing_overlay`: the selection, while it is on the frame on screen. */
export function editingOverlay(state: AppState): Overlay | undefined {
  return stackedOverlays(state).find((overlay) => overlay.id === state.selectedOverlay);
}

/** Every frame index of the document. */
export function allFrames(state: AppState): number[] {
  return Array.from({ length: state.doc?.frameCount ?? 0 }, (_, i) => i);
}

/** Desktop `seek_to_overlay`: an overlay being edited must sit on the frame on screen. */
export function seekToOverlay(app: App, overlay: Overlay): void {
  const { start, end } = overlay.range;
  if (app.state.playhead < start || app.state.playhead >= end) {
    app.update({ playhead: Math.min(start, (app.state.doc?.frameCount ?? 1) - 1) });
  }
}

/** The overlay edits the rail, sidebar, canvas and commands share. */
export interface Actions {
  addOverlay(tool: Tool): Promise<void>;
  pick(id: number | null): void;
  deleteOverlay(id: number): Promise<void>;
  restack(id: number, up: boolean): Promise<void>;
  copySelected(all: boolean): Promise<void>;
  editText(text: string): Promise<void>;
  restyle(change: (kind: OverlayKind) => OverlayKind): Promise<void>;
}

export function createActions(app: App, local: Store<Local>): Actions {
  return {
    /** Desktop `add_overlay`: one piece per run of the scope, the one on screen picked. */
    async addOverlay(tool: Tool) {
      const frames = framesInScope(app.state);
      if (frames.length === 0) return;
      const total = app.state.doc!.frameCount;
      const added = await app.dispatch("addOverlay", tool, frames, app.state.playhead);
      leaveCrop(app, local);
      if (!added) return;
      app.update({ selectedOverlay: added.overlay });
      app.toastChange(added.change, total);
    },

    /** Desktop `pick_overlay`: from the layer list; leaves the crop tool, seeks to it. */
    pick(id: number | null) {
      app.update({ selectedOverlay: id });
      const overlay = selectedOverlay(app.state);
      if (!overlay) return;
      leaveCrop(app, local);
      seekToOverlay(app, overlay);
    },

    async deleteOverlay(id: number) {
      const total = app.state.doc?.frameCount ?? 0;
      app.toastChange(await app.dispatch("deleteOverlay", id), total);
    },

    /** Desktop `restack_overlay`: a step past the layer shown next to it; stays picked. */
    async restack(id: number, up: boolean) {
      const change = await app.dispatch("restackOverlay", id, up, app.state.playhead);
      if (change) app.update({ selectedOverlay: id });
    },

    /** Desktop `copy_overlay_to_frames`: to every frame, or to the scope. */
    async copySelected(all: boolean) {
      const id = app.state.selectedOverlay;
      if (id === null) return;
      const frames = all ? allFrames(app.state) : framesInScope(app.state);
      const total = app.state.doc!.frameCount;
      const change = await app.dispatch("copyOverlayTo", id, frames);
      if (!change) return app.toast(t("The overlay is on those frames already"));
      app.update({ selectedOverlay: id });
      app.toastChange(change, total);
    },

    /** Desktop `edit_text`: lands where the scope reaches into the overlay. */
    async editText(text: string) {
      const id = app.state.selectedOverlay;
      if (id === null) return;
      const edit = await app.dispatch(
        "editText",
        id,
        text,
        framesInScope(app.state),
        app.state.playhead,
      );
      if (edit) app.update({ selectedOverlay: edit.overlay });
    },

    /** Desktop `restyle_overlay`: `change` writes one field onto a copy of the kind. */
    async restyle(change: (kind: OverlayKind) => OverlayKind) {
      const overlay = selectedOverlay(app.state);
      if (!overlay) return;
      seekToOverlay(app, overlay);
      const kind = change(overlay.kind);
      const edit = await app.dispatch(
        "restyleOverlay",
        overlay.id,
        kind,
        framesInScope(app.state),
        app.state.playhead,
      );
      if (edit) app.update({ selectedOverlay: edit.overlay });
    },
  };
}
