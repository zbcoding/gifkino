// The crop tool's sidebar group ("Crop / zoom") and what acts on its box:
// desktop `crop_panel`, `apply_crop` / `apply_shrink` / `start_zoom`, and
// Image ▸ "Crop frames…" (`crop_dialog`) and "Zoom and resize all frames".

import type { App } from "../app";
import { fill, t } from "../i18n";
import { framesInScope } from "../state";
import type { AppState, Store } from "../state";
import { alertDialog } from "../ui/dialog";
import { h } from "../ui/dom";
import { actionRow, group, spin } from "../ui/prefs";
import type { Change } from "../wasm/gifkino_wasm.js";
import { allFrames } from "./actions";
import { cropArmed, leaveCrop, usableCrop } from "./local";
import type { Box, Local } from "./local";

const idle = (state: AppState) => state.busy === null && (state.doc?.frameCount ?? 0) > 0;

export function installCrop(app: App, local: Store<Local>): void {
  /** Takes the box (it goes, as on the desktop) and runs `job` on it under the busy bar. */
  const onBox = async (
    label: string,
    frames: number,
    job: (box: Box) => Promise<Change | undefined>,
  ) => {
    const box = usableCrop(local.state);
    if (!box || frames === 0) return;
    leaveCrop(app, local);
    const change = await app.withBusy(t(label), async (report) => {
      report(0, frames);
      return job(box);
    });
    app.toastChange(change, app.state.doc?.frameCount ?? 0);
  };
  const zoom = (frames: number[]) =>
    // Translators: Progress while selected frames are being zoomed; both values are frame counts.
    onBox("Zooming… {done} / {total} frames", frames.length, (b) =>
      app.dispatch("zoom", frames, b.x, b.y, b.w, b.h),
    );
  const actions = {
    // ponytail: the wasm crop/zoom/shrink report no per-frame progress, so the bar shows 0 until done.
    crop: () =>
      // Translators: Progress while every frame is being cropped; both values are frame counts.
      onBox("Cropping… {done} / {total} frames", app.state.doc?.frameCount ?? 0, (b) =>
        app.dispatch("crop", b.x, b.y, b.w, b.h),
      ),
    zoom: () => zoom(framesInScope(app.state)),
    shrink: () => {
      const frames = framesInScope(app.state);
      // Translators: Progress while this frame's crop is being applied.
      return onBox("Cropping this frame…", frames.length, (b) =>
        app.dispatch("shrink", frames, b.x, b.y, b.w, b.h),
      );
    },
  };

  const hasBox = (state: AppState) => idle(state) && usableCrop(local.state) !== null;
  app.registerCommand({
    id: "zoom-to-selection",
    // Translators: Fills the canvas from the crop box, on the frame on screen.
    label: "Zoom this frame",
    run: actions.zoom,
  });
  app.registerCommand({
    id: "optimize-crop",
    // Translators: Crops every frame to a box typed in pixels, shrinking the canvas.
    label: "Crop frames…",
    menu: "image",
    enabled: idle,
    run: () => cropAllDialog(app, local),
  });
  app.registerCommand({
    id: "optimize-zoom-all",
    // Translators: Like the Zoom and resize button, but ignoring the frame
    // scope — needs a box drawn with the crop tool first.
    label: "Zoom and resize all frames",
    menu: "image",
    enabled: hasBox,
    run: () => zoom(allFrames(app.state)),
  });

  app.registerPanel({
    slot: "sidebar",
    id: "crop",
    order: 40,
    element: createCropPanel(app, local, actions, hasBox),
  });
}

function createCropPanel(
  app: App,
  local: Store<Local>,
  actions: Record<"crop" | "zoom" | "shrink", () => Promise<void>>,
  hasBox: (state: AppState) => boolean,
): HTMLElement {
  const label = h("div", { class: "crop-label" });
  const button = (text: string, tooltip: string, run: () => Promise<void>) =>
    h("button", { type: "button", title: tooltip, onclick: () => void run() }, text);
  const buttons = [
    button(
      t("Crop all frames"),
      t("Crops every frame in the document to this box, shrinking the canvas."),
      actions.crop,
    ),
    // Translators: Zoom follows the frame scope — the frame on screen, or the
    // selected frames when there is a selection. "Zoom and resize all frames"
    // in the Optimize menu ignores the scope and takes the whole document.
    button(
      t("Zoom and resize"),
      t("Fills the canvas from this box, on the frame on screen or the selected frames."),
      actions.zoom,
    ),
    // Translators: Crops the frame(s) in scope in place, leaving the rest of
    // each one transparent instead of scaling the kept region back up.
    button(
      t("Crop and keep size"),
      t(
        "Crops this box on every frame in scope, in place, without scaling it up, and blanks the rest of each frame to transparent.",
      ),
      actions.shrink,
    ),
  ];
  const panel = group(t("Crop / zoom"), h("div", { class: "crop-box" }, label, ...buttons));
  const sync = () => {
    panel.hidden = !cropArmed(app);
    const box = usableCrop(local.state);
    label.textContent = box
      ? fill(t("{width} × {height} at {x}, {y}"), {
          width: Math.round(box.w),
          height: Math.round(box.h),
          x: Math.round(box.x),
          y: Math.round(box.y),
        })
      : t("Drag a box on the canvas.");
    for (const each of buttons) each.disabled = !hasBox(app.state);
  };
  sync();
  app.store.subscribe(sync);
  local.subscribe(sync);
  return panel;
}

/** Desktop `crop_dialog`: a box typed in pixels, cropping every frame. */
async function cropAllDialog(app: App, local: Store<Local>): Promise<void> {
  const { width: cw, height: ch } = app.state.doc!;
  const x = spin(0, cw - 1, 1, 0);
  const y = spin(0, ch - 1, 1, 0);
  const width = spin(1, cw, 1, cw);
  const height = spin(1, ch, 1, ch);
  // The size can only reach the canvas's far edge from where the box starts.
  x.input.addEventListener("input", () => (width.input.max = String(Math.max(1, cw - x.value()))));
  y.input.addEventListener("input", () => (height.input.max = String(Math.max(1, ch - y.value()))));
  const content = group(
    null,
    actionRow("X", x.input),
    actionRow("Y", y.input),
    // Translators: Horizontal size of the crop box, in image pixels.
    actionRow(t("Width"), width.input),
    // Translators: Vertical size of the crop box, in image pixels.
    actionRow(t("Height"), height.input),
  );
  const dialog = alertDialog({
    title: t("Crop all frames"),
    // Translators: Explains that this crop changes the shared GIF canvas and moves overlays with it.
    body: t(
      "Keep this box in every frame. The canvas shrinks to fit it, and overlays move with the crop.",
    ),
    content,
    responses: [
      { id: "cancel", label: t("Cancel") },
      { id: "apply", label: t("Apply"), suggested: true },
    ],
    close: "cancel",
    default: "apply",
  });
  if ((await dialog.answer) !== "apply") return;
  const box = { x: x.value(), y: y.value(), w: width.value(), h: height.value() };
  box.w = Math.min(box.w, cw - box.x);
  box.h = Math.min(box.h, ch - box.y);
  // A full-canvas box changes nothing: no undo step, no busy bar.
  if (box.x === 0 && box.y === 0 && box.w === cw && box.h === ch) return;
  leaveCrop(app, local);
  const frames = app.state.doc!.frameCount;
  const change = await app.withBusy(t("Cropping… {done} / {total} frames"), async (report) => {
    report(0, frames);
    return app.dispatch("crop", box.x, box.y, box.w, box.h);
  });
  app.toastChange(change, frames);
}
