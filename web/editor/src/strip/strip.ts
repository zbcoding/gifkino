// The frame strip: one cell per frame with its thumbnail and number, the
// playhead, scope and selection marks, click/Shift/Ctrl selection, drag to
// reorder, and the frame's right-click menu. Desktop strip.rs.

import type { App } from "../app";
import { fill, t } from "../i18n";
import { framesInScope } from "../state";
import type { AppState } from "../state";
import { h, icon } from "../ui/dom";
import type { DocSnapshot } from "../worker/protocol";
import { cellPitch, gapAt, nextStripZoom, STRIP_ZOOM_STEP, thumbSize, zoomed } from "./geometry";
import { createContextMenu } from "./menu";
import type { MenuItem } from "./menu";
import {
  afterMove,
  anchorFor,
  extendSelection,
  insideSelection,
  seek,
  seekKeepSelection,
  toggleSelection,
} from "./selection";
import type { Picked, Remembered } from "./selection";

/** The drag payload's type, so a file dragged in from outside is not taken for a frame. */
const FRAME_DRAG = "application/x-gifkino-frame";
/** Pixels of Ctrl+wheel in pixel mode that make one zoom step; a mouse notch is ~100. */
const ZOOM_WHEEL_PX = 50;
/** The drop divider's width, in CSS pixels; strip.css draws it. */
const DIVIDER_W = 3;
const PENCIL = "M11.3 1.3a1 1 0 0 1 1.4 0l2 2a1 1 0 0 1 0 1.4L5.5 13.9l-4 .6.6-4z";

/** Applies picks and remembers the Shift+click anchor they leave; the bands pick too. */
export class Picker {
  private remembered: Remembered = { selection: [], anchor: null };

  private readonly app: App;

  constructor(app: App) {
    this.app = app;
  }

  get anchor(): number | null {
    return anchorFor(this.remembered, this.app.state.selection);
  }

  apply({ state, anchor }: Picked): void {
    this.remembered = { selection: state.selection, anchor };
    this.app.update(() => state);
  }
}

export interface Strip {
  element: HTMLElement;
  /** The scrolled box the bands follow sideways. */
  scroller: HTMLElement;
  /** Distance from one cell to the next, where the bands put their columns. */
  readonly pitch: number;
  zoomBy(factor: number): void;
  /** Called after the cells change size: a rebuild or a zoom. */
  onLayout(listener: () => void): void;
}

/** Ctrl+wheel steps the zoom; anything else is left to scroll. Shared with the bands. */
export function zoomOnCtrlWheel(element: HTMLElement, strip: Strip): void {
  let pixels = 0;
  element.addEventListener(
    "wheel",
    (event) => {
      if (!(event.ctrlKey || event.metaKey) || event.deltaY === 0) return;
      event.preventDefault();
      if (event.deltaMode === WheelEvent.DOM_DELTA_PIXEL) {
        pixels += event.deltaY;
        if (Math.abs(pixels) < ZOOM_WHEEL_PX) return;
      }
      const up = (event.deltaMode === WheelEvent.DOM_DELTA_PIXEL ? pixels : event.deltaY) < 0;
      pixels = 0;
      strip.zoomBy(up ? STRIP_ZOOM_STEP : 1 / STRIP_ZOOM_STEP);
    },
    { passive: false },
  );
}

/**
 * A frame's two right-click faces: the frame alone, or the selection it sits
 * in. The commands act on the scope either way, since a right-click outside
 * the selection seeks there first; only the wording differs, plus "Move to
 * position…", which names one frame.
 */
function frameMenu(onSelection: boolean): MenuItem[][] {
  const edits = onSelection
    ? [
        t("Delete selected frames"),
        t("Duplicate selected frames"),
        t("Cut selected frames"),
        t("Copy selected frames"),
      ]
    : [
        t("Delete this frame"),
        t("Duplicate this frame"),
        t("Cut this frame"),
        t("Copy this frame"),
      ];
  const clipboard = ["frame-delete", "frame-duplicate", "frame-cut", "frame-copy", "frame-paste"];
  const moves: MenuItem[] = [
    { label: t("Move earlier"), command: "frame-move-earlier" },
    { label: t("Move later"), command: "frame-move-later" },
  ];
  if (!onSelection) moves.push({ label: t("Move to position…"), command: "frame-move-to" });
  return [
    [
      ...[...edits, t("Paste frames")].map((label, i) => ({
        label,
        command: clipboard[i],
        accel: true,
      })),
      { label: t("Set delay…"), command: "frame-delay" },
    ],
    moves,
    [{ label: t("Insert frames from file…"), command: "frame-insert-file" }],
  ];
}

/** One cell for `frame`: picture over its number, the pencil badge for a frame edited outside. */
function thumbCell(index: number, detached: boolean): HTMLElement {
  return h(
    "div",
    { class: "thumb" },
    h("canvas", { class: "thumb-picture" }),
    h("span", { class: "thumb-number" }, String(index + 1)),
    detached &&
      h(
        "span",
        { class: "thumb-badge", title: t("Edited outside; overlays skip this frame") },
        icon(PENCIL),
      ),
  );
}

/** What the strip shows: a change here is worth a rebuild, an overlay edit is not. */
const stripKeys = (doc: DocSnapshot | null) =>
  doc?.frames.map((frame) => `${frame.key}${frame.detached ? "d" : ""}`).join() ?? "";

export function createStrip(app: App, picker: Picker): Strip {
  const row = h("div", { class: "strip" });
  const divider = h("div", { class: "drop-divider", hidden: true });
  row.append(divider);
  const scroller = h("div", { class: "strip-scroll" }, row);
  const menu = createContextMenu(app);
  const element = h("div", { class: "strip-panel" }, scroller, menu.element);

  let cells: HTMLElement[] = [];
  let shownKeys = "";
  let shownDelays = "";
  let zoom = 1;
  let thumb: [number, number] = [1, 1];
  const listeners: (() => void)[] = [];
  const layout = () => listeners.forEach((listener) => listener());

  // ---- Thumbnails, cached by frame key so a rebuild reuses what it showed before ----

  const pictures = new Map<number, ImageData>();
  let filling = false;
  let refill = false;

  const paint = (cell: HTMLElement, picture: ImageData) => {
    const canvas = cell.querySelector("canvas")!;
    canvas.width = picture.width;
    canvas.height = picture.height;
    canvas.getContext("2d")!.putImageData(picture, 0, 0);
  };

  const fillThumbs = async () => {
    if (filling) {
      refill = true;
      return;
    }
    filling = true;
    try {
      do {
        refill = false;
        const frames = app.state.doc?.frames ?? [];
        for (let i = 0; i < cells.length && !refill; i++) {
          const key = frames[i]?.key;
          if (key === undefined) continue;
          let picture = pictures.get(key);
          if (!picture) {
            const fetched = await app.thumbnail(i);
            // An edit landed meanwhile: index `i` may be another frame now.
            if (!fetched || app.state.doc?.frames[i]?.key !== key) continue;
            picture = new ImageData(fetched.data, fetched.width, fetched.height);
            pictures.set(key, picture);
          }
          if (cells[i]) paint(cells[i], picture);
        }
      } while (refill);
    } finally {
      filling = false;
    }
  };

  // ---- Sizes ----

  const sizeCells = () => {
    const [width, height] = [zoomed(thumb[0], zoom), zoomed(thumb[1], zoom)];
    for (const cell of cells) {
      cell.style.width = `${width}px`;
      const canvas = cell.querySelector("canvas")!;
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
    }
    divider.style.height = `${height}px`;
    layout();
  };

  const strip: Strip = {
    element,
    scroller,
    get pitch() {
      return cellPitch(thumb[0], zoom);
    },
    zoomBy(factor) {
      zoom = nextStripZoom(zoom, factor);
      sizeCells();
    },
    onLayout(listener) {
      listeners.push(listener);
    },
  };

  // ---- Cells ----

  const rebuild = (doc: DocSnapshot | null) => {
    for (const cell of cells) cell.remove();
    const frames = doc?.frames ?? [];
    cells = frames.map((frame, i) => thumbCell(i, frame.detached));
    row.append(...cells);
    thumb = doc ? thumbSize(doc.width, doc.height) : [1, 1];
    const shown = new Set(frames.map((frame) => frame.key));
    for (const key of pictures.keys()) if (!shown.has(key)) pictures.delete(key);
    sizeCells();
    void fillThumbs();
  };

  const sync = (state: AppState) => {
    const keys = stripKeys(state.doc);
    if (keys !== shownKeys) {
      shownKeys = keys;
      rebuild(state.doc);
    }
    const frames = state.doc?.frames ?? [];
    const delays = frames.map((frame) => frame.delayCs).join();
    if (delays !== shownDelays) {
      shownDelays = delays;
      cells.forEach((cell, i) => {
        // Translators: "cs" is centiseconds, the unit GIF stores frame delays in.
        cell.title = fill(t("Frame {number} · {delay} cs"), {
          number: i + 1,
          delay: frames[i].delayCs,
        });
      });
    }
    const scoped = new Set(framesInScope(state));
    const selected = new Set(state.selection);
    const draggable = state.busy === null;
    cells.forEach((cell, i) => {
      cell.classList.toggle("playhead", i === state.playhead);
      cell.classList.toggle("in-scope", scoped.has(i) && i !== state.playhead);
      cell.classList.toggle("selected", selected.has(i));
      cell.draggable = draggable;
    });
  };
  sync(app.state);
  app.store.subscribe(sync);

  // ---- Clicks ----

  const cellIndex = (event: Event) => {
    const cell = (event.target as Element).closest(".thumb");
    return cell ? cells.indexOf(cell as HTMLElement) : -1;
  };

  // A press inside the selection may start a drag that moves the whole
  // selection, so it only arms this; a plain click — no drag — then collapses
  // the selection to that one frame.
  let pendingSeek: number | null = null;

  row.addEventListener("pointerdown", (event) => {
    const i = cellIndex(event);
    if (event.button !== 0 || i === -1) return;
    pendingSeek = null;
    if (event.shiftKey) picker.apply(extendSelection(app.state, picker.anchor, i));
    else if (event.ctrlKey || event.metaKey) picker.apply(toggleSelection(app.state, i));
    else if (insideSelection(framesInScope(app.state), i)) pendingSeek = i;
    else picker.apply(seek(app.state, i));
  });
  row.addEventListener("click", (event) => {
    if (pendingSeek !== null && cellIndex(event) === pendingSeek)
      picker.apply(seek(app.state, pendingSeek));
    pendingSeek = null;
  });

  // Right-click acts on the frame under the pointer when it stands alone,
  // and on the selection when it sits inside one, which stays.
  row.addEventListener("contextmenu", (event) => {
    const i = cellIndex(event);
    if (i === -1) return;
    event.preventDefault();
    const onSelection = insideSelection(framesInScope(app.state), i);
    picker.apply(onSelection ? seekKeepSelection(app.state, picker.anchor, i) : seek(app.state, i));
    menu.open(event, frameMenu(onSelection));
  });

  // ---- Drag to reorder: the whole strip is one drop target tracking the nearest gap ----

  let dragFrom: number | null = null;

  const showGap = (gap: number | null) => {
    divider.hidden = gap === null || cells.length === 0;
    if (gap === null) return;
    const cellWidth = zoomed(thumb[0], zoom);
    // The gap past the last frame has no cell after it: its line sits on the last cell's right edge.
    const atEnd = gap === cells.length;
    const x = atEnd ? (gap - 1) * strip.pitch + cellWidth - DIVIDER_W : gap * strip.pitch;
    divider.style.left = `${x}px`;
  };

  const gapUnder = (event: DragEvent) => {
    const x = event.clientX - row.getBoundingClientRect().left;
    return gapAt(x, strip.pitch, zoomed(thumb[0], zoom), cells.length);
  };

  row.addEventListener("dragstart", (event) => {
    const i = cellIndex(event);
    if (i === -1 || !event.dataTransfer) return;
    // The press became a drag: the drop moves whatever the selection names,
    // so it must still be whole here.
    pendingSeek = null;
    dragFrom = i;
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData(FRAME_DRAG, String(i));
  });
  row.addEventListener("dragover", (event) => {
    if (dragFrom === null || !event.dataTransfer?.types.includes(FRAME_DRAG)) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "move";
    showGap(gapUnder(event));
  });
  row.addEventListener("dragleave", (event) => {
    if (!row.contains(event.relatedTarget as Node | null)) showGap(null);
  });
  row.addEventListener("dragend", () => {
    dragFrom = null;
    showGap(null);
  });
  row.addEventListener("drop", (event) => {
    if (dragFrom === null) return;
    event.preventDefault();
    event.stopPropagation();
    const from = dragFrom;
    dragFrom = null;
    showGap(null);
    void moveSelectionTo(from, gapUnder(event));
  });

  /** A drag that started inside the selection moves all of it; anywhere else, one frame. */
  const moveSelectionTo = async (from: number, gap: number) => {
    const { selection, busy, doc } = app.state;
    if (busy !== null || !doc) return;
    const picked = selection.length > 1 && selection.includes(from) ? selection : [from];
    const total = doc.frameCount;
    const moved = await app.dispatch("moveFrames", picked, gap);
    if (!moved) return;
    picker.apply(afterMove(app.state, picker.anchor, picked, moved.at));
    app.toastChange(moved.change, total);
  };

  zoomOnCtrlWheel(element, strip);
  return strip;
}
