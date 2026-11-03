// Window state only this slice reads. The crop tool being armed is the shared
// `state.activeTool === CROP_TOOL`; its box, the snap toggle, the shape the
// split button adds and the guides of a drag in flight live here.

import type { App } from "../app";
import { Store } from "../state";
import type { Guides, Tool } from "../wasm/gifkino_wasm.js";

export const CROP_TOOL = "crop";

/** Image pixels; `w`/`h` never negative. */
export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Local {
  /** Desktop `snap`, on at start. */
  snap: boolean;
  /** Which of rect/ellipse/arrow the shape split button adds: the one used last. */
  shapeTool: Exclude<Tool, "text">;
  crop: Box | null;
  guides: Guides;
}

export const NO_GUIDES: Guides = { x: null, y: null };

export function createLocal(app: App): Store<Local> {
  const local = new Store<Local>({ snap: true, shapeTool: "rect", crop: null, guides: NO_GUIDES });
  local.subscribe(() => app.redrawCanvas());
  return local;
}

export function cropArmed(app: App): boolean {
  return app.state.activeTool === CROP_TOOL;
}

/** Desktop `usable_crop`: a bare click leaves a zero-size box, which is not a box. */
export function usableCrop(local: Local): Box | null {
  const { crop } = local;
  return crop && crop.w >= 2 && crop.h >= 2 ? crop : null;
}

/** Desktop `leave_crop`: disarm the tool and forget its box. */
export function leaveCrop(app: App, local: Store<Local>): void {
  if (cropArmed(app)) app.update({ activeTool: null });
  local.update({ crop: null });
}
