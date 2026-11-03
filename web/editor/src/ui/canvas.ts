// The canvas: the playhead's composited frame, fitted into the area, with an
// overlay canvas on top where registered layers draw and take pointer input.

import type { App, PointerPhase, View } from "../app";
import type { AppState } from "../state";
import { h } from "./dom";

/** Breathing room between the picture and the area's edge, in CSS pixels. */
const MARGIN = 16;

/** Desktop `ContentFit::ScaleDown`: centred, shrunk to fit, never enlarged. */
export function fitView(
  areaWidth: number,
  areaHeight: number,
  width: number,
  height: number,
): View {
  const room = (size: number) => Math.max(1, size - 2 * MARGIN);
  const scale = Math.min(1, room(areaWidth) / width, room(areaHeight) / height);
  const left = (areaWidth - width * scale) / 2;
  const top = (areaHeight - height * scale) / 2;
  return {
    scale,
    left,
    top,
    toImage: (x, y) => [(x - left) / scale, (y - top) / scale],
    toScreen: (x, y) => [left + x * scale, top + y * scale],
  };
}

export function createCanvasView(app: App): HTMLElement {
  const picture = h("canvas", { class: "frame" });
  const overlay = h("canvas", { class: "overlay-layer" });
  const area = h("div", { class: "canvas-area" }, picture, overlay);
  const frameContext = picture.getContext("2d")!;
  const overlayContext = overlay.getContext("2d")!;
  let view: View | null = null;

  // Composites are coalesced: one in flight, and the latest wish runs next.
  let shown = "";
  let inFlight = false;
  const wanted = (state: AppState) => (state.doc ? `${state.doc.revision}:${state.playhead}` : "");
  const refreshPicture = async () => {
    const key = wanted(app.state);
    if (inFlight || key === shown || !key) return;
    inFlight = true;
    try {
      const frame = await app.composite(app.state.playhead);
      if (frame) {
        if (picture.width !== frame.width) picture.width = frame.width;
        if (picture.height !== frame.height) picture.height = frame.height;
        frameContext.putImageData(new ImageData(frame.data, frame.width, frame.height), 0, 0);
      }
      shown = key;
    } finally {
      inFlight = false;
    }
    void refreshPicture();
  };

  const layout = () => {
    const doc = app.state.doc;
    if (!doc || doc.width === 0) return;
    view = fitView(area.clientWidth, area.clientHeight, doc.width, doc.height);
    Object.assign(picture.style, {
      left: `${view.left}px`,
      top: `${view.top}px`,
      width: `${doc.width * view.scale}px`,
      height: `${doc.height * view.scale}px`,
    });
    const ratio = devicePixelRatio;
    overlay.width = Math.round(area.clientWidth * ratio);
    overlay.height = Math.round(area.clientHeight * ratio);
    overlayContext.setTransform(ratio, 0, 0, ratio, 0, 0);
    app.redrawCanvas();
  };

  app.redrawCanvas = () => {
    overlayContext.clearRect(0, 0, overlay.width, overlay.height);
    if (!view) return;
    for (const layer of [...app.layers].sort((a, b) => a.order - b.order)) {
      layer.draw?.(overlayContext, view, app.state);
    }
  };

  const route = (phase: PointerPhase) => (event: PointerEvent) => {
    if (!view) return;
    const topFirst = [...app.layers].sort((a, b) => b.order - a.order);
    const taken = topFirst.some((layer) => layer.pointer?.(event, phase, view!));
    if (taken && phase === "down") overlay.setPointerCapture(event.pointerId);
  };
  overlay.addEventListener("pointerdown", route("down"));
  overlay.addEventListener("pointermove", route("move"));
  overlay.addEventListener("pointerup", route("up"));

  new ResizeObserver(layout).observe(area);
  app.store.subscribe((state, previous) => {
    const resized =
      state.doc?.width !== previous.doc?.width || state.doc?.height !== previous.doc?.height;
    if (resized) layout();
    else app.redrawCanvas();
    void refreshPicture();
  });
  return area;
}
