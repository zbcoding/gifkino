// The overlay bands under the strip: one band per overlay spanning its frames,
// packed into rows. This is the layer list. A click picks the overlay with its
// whole range, a right-click opens its menu. Desktop `overlay_bands`.

import type { App } from "../app";
import { fill, t, tn } from "../i18n";
import { framesInScope } from "../state";
import type { AppState } from "../state";
import { h } from "../ui/dom";
import { BAND_H, BANDS_COLLAPSED_ROWS, bandAt, packRows, THUMB_SPACING } from "./geometry";
import type { Band } from "./geometry";
import { createContextMenu } from "./menu";
import { pickOverlay, selectBand } from "./selection";
import type { Picker, Strip } from "./strip";
import { zoomOnCtrlWheel } from "./strip";

function bandsOf(state: AppState): Band[] {
  const overlays = state.doc?.overlays ?? [];
  const rows = packRows(overlays.map((overlay) => overlay.range));
  return overlays.map((overlay, i) => ({
    id: overlay.id,
    name: overlay.name,
    range: overlay.range,
    row: rows[i],
    selected: overlay.id === state.selectedOverlay,
  }));
}

function drawBands(canvas: HTMLCanvasElement, bands: Band[], pitch: number, width: number): void {
  const rows = bands.reduce((most, band) => Math.max(most, band.row + 1), 0);
  const height = rows * BAND_H;
  const ratio = devicePixelRatio;
  canvas.width = Math.round(width * ratio);
  canvas.height = Math.round(height * ratio);
  canvas.style.width = `${width}px`;
  canvas.style.height = `${height}px`;
  const ctx = canvas.getContext("2d")!;
  ctx.scale(ratio, ratio);
  ctx.fillStyle = getComputedStyle(canvas).color;
  const h = BAND_H - 4;
  ctx.font = `${h - 6}px sans-serif`;
  for (const band of bands) {
    const x = band.range.start * pitch;
    const w = Math.max(4, (band.range.end - band.range.start) * pitch - THUMB_SPACING);
    const y = band.row * BAND_H + 2;
    ctx.globalAlpha = band.selected ? 0.45 : 0.22;
    ctx.fillRect(x, y, w, h);
    ctx.globalAlpha = 0.9;
    ctx.fillText(band.name, x + 6, y + h - 4);
  }
}

export function createBands(app: App, strip: Strip, picker: Picker): HTMLElement {
  const canvas = h("canvas", { class: "bands" });
  const scroll = h("div", { class: "bands-scroll" }, canvas);
  const expander = h("button", { class: "flat bands-expander" });
  const menu = createContextMenu(app);
  const element = h("div", { class: "bands-panel" }, scroll, expander, menu.element);
  let expanded = false;
  let bands: Band[] = [];

  const sync = () => {
    const { state } = app;
    bands = bandsOf(state);
    const count = state.doc?.frameCount ?? 0;
    drawBands(canvas, bands, strip.pitch, Math.floor(count * strip.pitch));
    scroll.scrollLeft = strip.scroller.scrollLeft;

    // The bands cost the canvas whatever room they take, so they take none
    // until there is an overlay, then a row at a time up to the cap.
    const used = bands.reduce((most, band) => Math.max(most, band.row + 1), 0);
    const hidden = Math.max(0, used - BANDS_COLLAPSED_ROWS);
    const shown = expanded ? used : Math.min(used, BANDS_COLLAPSED_ROWS);
    element.hidden = used === 0;
    scroll.style.maxHeight = `${shown * BAND_H}px`;
    expander.hidden = hidden === 0;
    expander.textContent = expanded
      ? fill(t("▾ {count} overlay rows"), { count: used })
      : // Translators: Expander under the filmstrip when overlay rows are collapsed.
        fill(tn("▸ {count} more overlay row", "▸ {count} more overlay rows", hidden), {
          count: hidden,
        });
  };
  app.store.subscribe((state, previous) => {
    if (state.doc !== previous.doc || state.selectedOverlay !== previous.selectedOverlay) sync();
  });
  strip.onLayout(sync);
  strip.scroller.addEventListener("scroll", () => {
    scroll.scrollLeft = strip.scroller.scrollLeft;
  });
  expander.addEventListener("click", () => {
    expanded = !expanded;
    sync();
  });

  const hit = (event: MouseEvent) => bandAt(bands, event.offsetX, event.offsetY, strip.pitch);
  canvas.addEventListener("pointerdown", (event) => {
    if (event.button === 0) picker.apply(selectBand(app.state, picker.anchor, hit(event)));
  });
  // Right-click a band to act on that overlay; on empty space no menu at all,
  // rather than one acting on whatever was selected elsewhere.
  canvas.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    const id = hit(event);
    if (id === null) return;
    app.update((state) => pickOverlay(state, id));
    // More than one frame in scope gives the copy a selection to aim at.
    const copy =
      framesInScope(app.state).length > 1
        ? { label: t("Copy overlay to selected frames"), command: "overlay-copy-selected" }
        : { label: t("Copy overlay to all frames"), command: "overlay-copy-all" };
    menu.open(event, [[copy], [{ label: t("Delete overlay"), command: "overlay-delete" }]]);
  });

  // The bands scroll sideways with the strip; a sideways wheel here moves the strip.
  scroll.addEventListener("wheel", (event) => {
    const dx = event.deltaX || (event.shiftKey ? event.deltaY : 0);
    if (dx === 0 || event.ctrlKey || event.metaKey) return;
    event.preventDefault();
    strip.scroller.scrollLeft += dx;
  });
  zoomOnCtrlWheel(element, strip);
  sync();
  return element;
}
