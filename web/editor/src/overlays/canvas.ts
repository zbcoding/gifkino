// The canvas layer: click to select, drag to move, a corner to resize, the
// rotate modifier to turn — and with the crop tool armed, drag a box. Desktop
// `canvas.rs` (drawing, hover) and `canvas_press` / `canvas_drag` /
// `canvas_release` in `actions.rs`. Hit tests and drag maths run in the
// worker, so pointer events are handled one after another on a promise chain,
// with motion coalesced to the latest position.

import type { App, View } from "../app";
import { fill, t } from "../i18n";
import { modsHeld, modsLabel } from "../modals";
import { framesInScope } from "../state";
import type { AppState, Store } from "../state";
import type { Grip, Guide, Overlay, Transform } from "../wasm/gifkino_wasm.js";
import rotateCursorUrl from "../../../../resources/rotate-handle.png";
import { editingOverlay } from "./actions";
import { cropArmed, leaveCrop, NO_GUIDES, usableCrop } from "./local";
import type { Local } from "./local";

/** Grab distance of a grip, in screen pixels: constant whatever the zoom. */
const HANDLE_PX = 9.5;
const HANDLE_R = 4.5;
const HANDLE_FILL = "rgb(26 77 255)";
/** How close, in screen pixels, a dragged edge or centre comes to a guide to snap onto it. */
const SNAP_PX = 8;
const ROTATE_CURSOR = `url("${rotateCursorUrl}") 16 16, grab`;

interface Drag {
  grip: Grip | "crop";
  from: [number, number];
  origin: Transform;
  snapReach: number;
  moved: boolean;
  current: Transform;
  id: number;
}

interface Modifiers {
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

/** The overlay whose grips show: picked, painting on this frame, crop tool off. */
function boxedOverlay(app: App, state: AppState): Overlay | undefined {
  const overlay = editingOverlay(state);
  return overlay && !overlay.hidden && !cropArmed(app) ? overlay : undefined;
}

export function installCanvas(app: App, local: Store<Local>): void {
  let drag: Drag | null = null;
  let pressed = false;
  let chain = Promise.resolve();
  const enqueue = (job: () => Promise<void> | void) => {
    chain = chain.then(job).catch((error: unknown) => app.toast(String(error)));
  };

  const startDrag = (grip: Grip, overlay: Overlay, from: [number, number], snapReach: number) => {
    drag = {
      grip,
      from,
      origin: overlay.transform,
      snapReach,
      moved: false,
      current: overlay.transform,
      id: overlay.id,
    };
  };

  /** Desktop `canvas_press`. */
  const press = async (x: number, y: number, scale: number, mods: Modifiers) => {
    const { playhead } = app.state;
    if (
      cropArmed(app) &&
      !usableCrop(local.state) &&
      (await app.query("overlayAt", playhead, x, y)) !== undefined
    ) {
      // Before any box is drawn, a press on an overlay is a pick.
      leaveCrop(app, local);
    }
    if (cropArmed(app)) {
      local.update({ crop: { x, y, w: 0, h: 0 } });
      const origin = { x, y, w: 0, h: 0, angle: 0 };
      drag = {
        grip: "crop",
        from: [x, y],
        origin,
        snapReach: 0,
        moved: false,
        current: origin,
        id: -1,
      };
      return;
    }
    const grab = HANDLE_PX / Math.max(scale, 0.01);
    const snapReach = SNAP_PX / Math.max(scale, 0.01);
    const rotating = modsHeld(app.modifiersFor("canvas-rotate"), mods);
    // A grip of the selection wins over what is under the pointer, or a small
    // overlay could never be resized; the rotate modifier outranks both.
    const selected = boxedOverlay(app, app.state);
    if (selected) {
      const corner = await app.compute("handleAt", selected.transform, x, y, grab);
      const onOverlay =
        corner !== undefined || (await app.compute("contains", selected.transform, x, y));
      if (rotating && onOverlay) return startDrag("rotate", selected, [x, y], snapReach);
      if (corner !== undefined) {
        return startDrag({ resize: corner as 0 | 1 | 2 | 3 }, selected, [x, y], snapReach);
      }
    }
    const hit = await app.query("overlayAt", playhead, x, y);
    const overlay = app.state.doc?.overlays.find(({ id }) => id === hit);
    app.update({ selectedOverlay: overlay?.id ?? null });
    if (overlay) startDrag(rotating ? "rotate" : "move", overlay, [x, y], snapReach);
  };

  /** Desktop `canvas_drag`: live, outside the history until release. */
  const motion = async (x: number, y: number, mods: Modifiers) => {
    if (!drag) return;
    const [fromX, fromY] = drag.from;
    if (!drag.moved && Math.abs(x - fromX) < 0.5 && Math.abs(y - fromY) < 0.5) return;
    drag.moved = true;
    if (drag.grip === "crop") {
      const box = {
        x: Math.min(fromX, x),
        y: Math.min(fromY, y),
        w: Math.abs(x - fromX),
        h: Math.abs(y - fromY),
      };
      return local.update({ crop: box });
    }
    const dragged = await app.query(
      "dragOverlay",
      drag.grip,
      drag.origin,
      fromX,
      fromY,
      x,
      y,
      local.state.snap ? drag.snapReach : null,
      modsHeld(app.modifiersFor("canvas-keep-aspect"), mods),
      modsHeld(app.modifiersFor("canvas-from-center"), mods),
    );
    drag.current = dragged.transform;
    await app.dispatch("setOverlayTransformLive", drag.id, dragged.transform);
    local.update({ guides: dragged.guides });
  };

  /** Desktop `canvas_release`: the drag becomes one undo step on the scope. */
  const release = async () => {
    local.update({ guides: NO_GUIDES });
    const done = drag;
    drag = null;
    if (!done || !done.moved || done.grip === "crop") return;
    const { playhead, doc } = app.state;
    const total = doc?.frameCount ?? 0;
    const edit = await app.dispatch(
      "commitOverlayDrag",
      done.id,
      done.grip,
      done.origin,
      done.current,
      framesInScope(app.state),
      playhead,
    );
    if (!edit) return;
    app.update({ selectedOverlay: edit.overlay });
    app.toastChange(edit.change, total);
  };

  // Motion while pressed: only the latest position waits in the queue.
  let queued: { x: number; y: number; mods: Modifiers } | null = null;
  const queueMotion = (x: number, y: number, mods: Modifiers) => {
    const waiting = queued !== null;
    queued = { x, y, mods };
    if (waiting) return;
    enqueue(() => {
      const next = queued!;
      queued = null;
      return motion(next.x, next.y, next.mods);
    });
  };

  const hover = createHover(app);

  app.registerCanvasLayer({
    id: "overlays",
    order: 10,
    draw: (context, view, state) => draw(context, view, state, app, local.state),
    pointer(event, phase, view) {
      if (!app.state.doc) return false;
      const [x, y] = view.toImage(event.offsetX, event.offsetY);
      const mods = { ctrlKey: event.ctrlKey, shiftKey: event.shiftKey, altKey: event.altKey };
      if (phase === "down") {
        if (event.button !== 0) return false;
        pressed = true;
        enqueue(() => press(x, y, view.scale, mods));
      } else if (phase === "move" && pressed) {
        queueMotion(x, y, mods);
      } else if (phase === "move") {
        hover(event, x, y, view.scale);
      } else if (pressed) {
        pressed = false;
        enqueue(release);
      }
      return true;
    },
  });

  // Picking an overlay anywhere (a band, the layer list) leaves the crop tool;
  // a tool switched off elsewhere takes its box along.
  app.store.subscribe((state, previous) => {
    if (
      state.selectedOverlay !== null &&
      state.selectedOverlay !== previous.selectedOverlay &&
      cropArmed(app)
    ) {
      leaveCrop(app, local);
    } else if (!cropArmed(app) && local.state.crop) {
      local.update({ crop: null });
    }
  });

  // Escape is not bindable: it backs out of the crop tool, else drops the pick.
  document.addEventListener("keydown", (event) => {
    // A dialog or popover open takes Escape for itself.
    const ownedElsewhere = document.querySelector("dialog[open], :popover-open");
    if (event.key !== "Escape" || event.defaultPrevented || ownedElsewhere) return;
    if (cropArmed(app)) leaveCrop(app, local);
    else app.update({ selectedOverlay: null });
  });
}

/**
 * Desktop `install_canvas_hover`: the resize glyph for the grip under the
 * pointer, the rotate cursor while the rotate modifier is down, a move glyph
 * over any overlay, and the matching hint as the tooltip.
 */
function createHover(app: App) {
  let busy = false;
  let latest: { event: PointerEvent; x: number; y: number; scale: number } | null = null;
  const settle = async () => {
    while (latest) {
      const { event, x, y, scale } = latest;
      latest = null;
      const cursor = await hoverCursor(app, event, x, y, scale);
      const target = event.target as HTMLElement;
      target.style.cursor = cursor === "rotate" ? ROTATE_CURSOR : cursor;
      target.title = cursor === "default" ? "" : cursor === "move" ? moveHint(app) : gripHint(app);
    }
    busy = false;
  };
  return (event: PointerEvent, x: number, y: number, scale: number) => {
    latest = { event, x, y, scale };
    if (busy) return;
    busy = true;
    void settle();
  };
}

async function hoverCursor(
  app: App,
  mods: Modifiers,
  x: number,
  y: number,
  scale: number,
): Promise<string> {
  if (cropArmed(app)) return "default";
  const selected = boxedOverlay(app, app.state);
  if (selected && modsHeld(app.modifiersFor("canvas-rotate"), mods)) return "rotate";
  if (selected) {
    const corner = await app.compute(
      "handleAt",
      selected.transform,
      x,
      y,
      HANDLE_PX / Math.max(scale, 0.01),
    );
    if (corner !== undefined) return cornerCursor(corner, selected.transform.angle);
  }
  const over = await app.query("overlayAt", app.state.playhead, x, y);
  return over === undefined ? "default" : "move";
}

/**
 * Desktop `corner_cursor`: the glyph for a corner of a box turned by `angle`,
 * snapped within the corner's own diagonal family so the arrow keeps pointing
 * along the rotated edge.
 */
export function cornerCursor(corner: number, angle: number): string {
  const CURSORS = [
    "e-resize",
    "se-resize",
    "s-resize",
    "sw-resize",
    "w-resize",
    "nw-resize",
    "n-resize",
    "ne-resize",
  ];
  // Axis-aligned octant of each corner, in TL, TR, BL, BR order.
  const base = [5, 7, 3, 1][corner % 4];
  const parity = base & 1;
  const rotated = base * 45 + (angle * 180) / Math.PI;
  const steps = Math.round((rotated - parity * 45) / 90);
  return CURSORS[(((parity + 2 * steps) % 8) + 8) % 8];
}

function gripHint(app: App): string {
  return fill(
    // Translators: Canvas tooltip over an overlay's resize grip. Keep the line breaks.
    t(
      "Drag to resize\n{aspect}: keep the aspect ratio\n{center}+drag: resize from the center\n{rotate}+drag: rotate",
    ),
    {
      aspect: modsLabel(app.modifiersFor("canvas-keep-aspect")),
      center: modsLabel(app.modifiersFor("canvas-from-center")),
      rotate: modsLabel(app.modifiersFor("canvas-rotate")),
    },
  );
}

function moveHint(app: App): string {
  // Translators: Canvas tooltip over the body of an overlay. Keep the line break.
  return fill(t("Drag to move\n{rotate}+drag: rotate"), {
    rotate: modsLabel(app.modifiersFor("canvas-rotate")),
  });
}

let accent = "";

/** Desktop `draw_canvas_overlay`: drawn over the picture, never into the GIF. */
function draw(
  context: CanvasRenderingContext2D,
  view: View,
  state: AppState,
  app: App,
  local: Local,
) {
  const { doc } = state;
  if (!doc || doc.width === 0) return;
  accent ||= getComputedStyle(document.documentElement).getPropertyValue("--accent").trim();
  const { width, height } = context.canvas;
  const [imageW, imageH] = [doc.width * view.scale, doc.height * view.scale];
  context.lineWidth = 1;
  const crop = cropArmed(app) ? local.crop : null;

  const dimOutside = (alpha: number, x: number, y: number, w: number, h: number) => {
    context.fillStyle = `rgb(0 0 0 / ${alpha})`;
    context.beginPath();
    context.rect(0, 0, width, height);
    context.rect(x, y, w, h);
    context.fill("evenodd");
  };

  if (crop) {
    // Dim what the crop would throw away, so the box reads as a keep-area.
    const [px, py] = view.toScreen(crop.x, crop.y);
    const [pw, ph] = [crop.w * view.scale, crop.h * view.scale];
    dimOutside(0.45, px, py, pw, ph);
    context.strokeStyle = "#fff";
    context.strokeRect(px, py, pw, ph);
    return;
  }
  // An overlay dragged off the picture is clipped from every frame: say where
  // the picture ends rather than letting it vanish.
  dimOutside(0.3, view.left, view.top, imageW, imageH);
  context.globalAlpha = 0.55;
  context.strokeStyle = accent;
  context.strokeRect(view.left + 0.5, view.top + 0.5, imageW - 1, imageH - 1);
  context.globalAlpha = 1;

  drawGuides(context, view, doc.width, doc.height, local.guides);
  const overlay = boxedOverlay(app, state);
  if (overlay) drawGrips(context, view, overlay.transform);
}

/** Desktop `draw_guides`: the edge or centre line a drag is snapped to, dashed. */
function drawGuides(
  context: CanvasRenderingContext2D,
  view: View,
  width: number,
  height: number,
  guides: Local["guides"],
) {
  const at = (guide: Guide, extent: number) =>
    guide === "start" ? 0 : guide === "center" ? extent / 2 : extent;
  context.beginPath();
  if (guides.x) {
    const x = Math.round(view.left + at(guides.x, width) * view.scale) + 0.5;
    context.moveTo(x, view.top);
    context.lineTo(x, view.top + height * view.scale);
  }
  if (guides.y) {
    const y = Math.round(view.top + at(guides.y, height) * view.scale) + 0.5;
    context.moveTo(view.left, y);
    context.lineTo(view.left + width * view.scale, y);
  }
  context.strokeStyle = "rgb(51 128 255 / 0.9)";
  context.setLineDash([4, 4]);
  context.stroke();
  context.setLineDash([]);
}

/** The dashed outline of the picked overlay and its four grips, turned with it. */
function drawGrips(context: CanvasRenderingContext2D, view: View, transform: Transform) {
  const [cx, cy] = view.toScreen(transform.x + transform.w / 2, transform.y + transform.h / 2);
  const [halfW, halfH] = [(transform.w * view.scale) / 2, (transform.h * view.scale) / 2];
  context.save();
  context.translate(cx, cy);
  context.rotate(transform.angle);
  context.globalAlpha = 0.9;
  context.strokeStyle = accent;
  context.setLineDash([4, 3]);
  context.strokeRect(-halfW, -halfH, halfW * 2, halfH * 2);
  context.setLineDash([]);
  context.globalAlpha = 1;
  for (const [gx, gy] of [
    [-halfW, -halfH],
    [halfW, -halfH],
    [-halfW, halfH],
    [halfW, halfH],
  ]) {
    context.beginPath();
    context.arc(gx, gy, HANDLE_R, 0, 2 * Math.PI);
    context.fillStyle = HANDLE_FILL;
    context.fill();
    context.strokeStyle = "rgb(255 255 255 / 0.7)";
    context.stroke();
  }
  context.restore();
}
