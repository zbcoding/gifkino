// What a click in the strip or on a band does to the window state: desktop
// `seek`, `extend_selection`, `toggle_selection`, `select_band`,
// `pick_overlay`, `select_all_frames` and `move_picked`'s follow-up. Pure, so
// the rules are tested without a DOM.

import type { AppState } from "../state";

/**
 * The window state after a pick, plus the Shift+click anchor. The anchor is
 * the strip's alone, so it rides beside the state instead of in it.
 */
export interface Picked {
  state: AppState;
  anchor: number | null;
}

/** The anchor the strip set, kept while the selection is still the one it was set for. */
export interface Remembered {
  selection: number[];
  anchor: number | null;
}

/**
 * The Shift+click anchor for `selection`. A selection someone else made (a
 * paste, a nudge) starts its run at its first frame; one the strip made keeps
 * the anchor it was made from, across the copies `edited` makes of it.
 */
export function anchorFor(remembered: Remembered, selection: number[]): number | null {
  const kept =
    remembered.selection.length === selection.length &&
    remembered.selection.every((frame, i) => frame === selection[i]);
  return kept ? remembered.anchor : (selection[0] ?? null);
}

function clampFrame(state: AppState, i: number): number {
  return Math.max(0, Math.min(i, (state.doc?.frameCount ?? 0) - 1));
}

/** Whether frame `i` is one of several frames in `scope`: a press there is aimed at the selection. */
export function insideSelection(scope: number[], i: number): boolean {
  return scope.length > 1 && scope.includes(i);
}

/** Jump to frame `i` and make it the one frame being worked on: the scope follows. */
export function seek(state: AppState, i: number): Picked {
  const playhead = clampFrame(state, i);
  return { state: { ...state, playhead, selection: [], scope: "frame" }, anchor: playhead };
}

/** Right-click inside the selection: move the playhead, keep what is selected. */
export function seekKeepSelection(state: AppState, anchor: number | null, i: number): Picked {
  return { state: { ...state, playhead: clampFrame(state, i) }, anchor };
}

/** Shift+click: the run between the anchor and `i`, in either direction; the anchor stays. */
export function extendSelection(state: AppState, anchor: number | null, i: number): Picked {
  const to = clampFrame(state, i);
  const from = clampFrame(state, anchor ?? state.playhead);
  const selection = Array.from(
    { length: Math.abs(to - from) + 1 },
    (_, k) => Math.min(from, to) + k,
  );
  return { state: { ...state, selection, playhead: to, scope: "range" }, anchor: from };
}

/** Ctrl+click: add or remove frame `i` wherever it sits; the result need not be a run. */
export function toggleSelection(state: AppState, i: number): Picked {
  const frame = clampFrame(state, i);
  const start = state.selection.length === 0 ? [state.playhead] : state.selection;
  const selection = start.includes(frame)
    ? start.filter((f) => f !== frame)
    : [...start, frame].sort((a, b) => a - b);
  const scope = selection.length === 0 ? "frame" : "range";
  return { state: { ...state, selection, playhead: frame, scope }, anchor: frame };
}

/** Ctrl+A: every frame, through the scope rather than a selection. */
export function selectAllFrames(state: AppState): AppState {
  if ((state.doc?.frameCount ?? 0) === 0) return state;
  return { ...state, selection: [], scope: "all" };
}

/**
 * The canvas only shows the playhead frame, so a picked overlay must sit on
 * it: seek to its first frame when the playhead is off it. Selection and
 * scope stay.
 */
function playheadOn(state: AppState, id: number): number {
  const range = state.doc?.overlays.find((overlay) => overlay.id === id)?.range;
  if (!range || (range.start <= state.playhead && state.playhead < range.end))
    return state.playhead;
  return clampFrame(state, range.start);
}

/** Right-click on a band: pick that overlay. */
export function pickOverlay(state: AppState, id: number): AppState {
  return { ...state, selectedOverlay: id, playhead: playheadOn(state, id) };
}

/**
 * A band click: the overlay together with every frame it covers, so an edit
 * reaches it whole. `null` is a click on empty band space.
 */
export function selectBand(state: AppState, anchor: number | null, id: number | null): Picked {
  if (id === null) return { state: { ...state, selectedOverlay: null }, anchor };
  const range = state.doc?.overlays.find((overlay) => overlay.id === id)?.range;
  if (!range) return { state, anchor };
  const count = state.doc?.frameCount ?? 0;
  const selection: number[] = [];
  for (let frame = range.start; frame < Math.min(range.end, count); frame++) selection.push(frame);
  return {
    state: {
      ...state,
      selectedOverlay: id,
      selection,
      scope: "range",
      playhead: playheadOn(state, id),
    },
    anchor: selection[0] ?? null,
  };
}

/**
 * After `picked` moved to start at `at`: the playhead rides the frame it was
 * on, and a moved run of several stays selected at its new home.
 */
export function afterMove(
  state: AppState,
  anchor: number | null,
  picked: number[],
  at: number,
): Picked {
  const sorted = [...new Set(picked)].sort((a, b) => a - b);
  const ridden = sorted.indexOf(state.playhead);
  const playhead = ridden === -1 ? state.playhead : at + ridden;
  if (sorted.length < 2) return { state: { ...state, playhead }, anchor };
  const selection = sorted.map((_, k) => at + k);
  return { state: { ...state, playhead, selection, scope: "range" }, anchor: at };
}
