// Where things sit in the strip and the bands under it: desktop strip.rs's
// cell pitch, drop gap, zoom steps and band packing.

/** The box a thumbnail is fitted into, on its long side: core `THUMB_BOX`. */
const THUMB_BOX = 72;
/** Pixels between two strip cells; not zoomed. */
export const THUMB_SPACING = 4;
/** One band row's height. */
export const BAND_H = 18;
/** How many overlay rows show before the band area scrolls behind an expander. */
export const BANDS_COLLAPSED_ROWS = 5;
const STRIP_ZOOM_MIN = 0.2;
const STRIP_ZOOM_MAX = 3;
export const STRIP_ZOOM_STEP = 1.2;

/** Core `thumb_size`: a `w`×`h` frame fitted to the thumbnail box, at least a pixel each way. */
export function thumbSize(width: number, height: number): [number, number] {
  const scale = Math.fround(THUMB_BOX / Math.max(width, height, 1));
  const side = (px: number) => Math.max(1, Math.round(Math.fround(px * scale)));
  return [side(width), side(height)];
}

/** One thumbnail dimension at `zoom`, in whole pixels, never off to nothing. */
export function zoomed(px: number, zoom: number): number {
  return Math.max(1, Math.round(px * zoom));
}

/**
 * Distance from one strip cell to the next: a zoomed thumbnail plus the
 * spacing, which is not zoomed. The bands put their per-frame columns here.
 */
export function cellPitch(thumbWidth: number, zoom: number): number {
  return zoomed(Math.max(thumbWidth, 1), zoom) + THUMB_SPACING;
}

/**
 * Which gap between thumbnails strip-local `x` is nearest: `0` is before the
 * first frame, `count` after the last. Each cell splits at its own midpoint,
 * so the divider promises exactly where a drop lands.
 */
export function gapAt(x: number, pitch: number, cellWidth: number, count: number): number {
  const past = Math.floor((x - cellWidth / 2) / pitch) + 1;
  return Math.max(0, Math.min(past, count));
}

/** The strip zoom after `factor` (`0` resets to 1×), clamped to its bounds. */
export function nextStripZoom(current: number, factor: number): number {
  if (factor === 0) return 1;
  return Math.min(STRIP_ZOOM_MAX, Math.max(STRIP_ZOOM_MIN, current * factor));
}

export interface FrameRange {
  start: number;
  end: number;
}

/**
 * Band rows: each overlay takes the lowest row where nothing already sits
 * over its frames, so an overlay far from the others does not draw ten rows down.
 */
export function packRows(ranges: FrameRange[]): number[] {
  const occupied: FrameRange[][] = [];
  return ranges.map((range) => {
    const free = (taken: FrameRange[]) =>
      taken.every((t) => t.end <= range.start || range.end <= t.start);
    let row = occupied.findIndex(free);
    if (row === -1) row = occupied.push([]) - 1;
    occupied[row].push(range);
    return row;
  });
}

export interface Band {
  id: number;
  name: string;
  range: FrameRange;
  row: number;
  selected: boolean;
}

/** The overlay whose packed row and frame span hold the point; rows share overlays, so search. */
export function bandAt(bands: Band[], x: number, y: number, pitch: number): number | null {
  const row = Math.floor(y / BAND_H);
  const frame = Math.floor(x / pitch);
  const hit = bands.find(
    (band) => band.row === row && band.range.start <= frame && frame < band.range.end,
  );
  return hit?.id ?? null;
}
