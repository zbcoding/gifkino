import { describe, expect, test } from "vite-plus/test";
import { initialState } from "../state";
import type { AppState } from "../state";
import { docWith } from "../test-support";
import type { Overlay } from "../wasm/gifkino_wasm.js";
import {
  afterMove,
  anchorFor,
  extendSelection,
  insideSelection,
  pickOverlay,
  seek,
  selectAllFrames,
  selectBand,
  toggleSelection,
} from "./selection";

const overlay = (id: number, start: number, end: number): Overlay => ({
  id,
  name: "Text",
  kind: { type: "shape", shape: "rect", fill: null, stroke: null },
  range: { start, end },
  transform: { x: 0, y: 0, w: 1, h: 1, angle: 0 },
  opacity: 1,
  hidden: false,
});

const tenFrames = (changes: Partial<AppState> = {}): AppState => ({
  ...initialState,
  doc: docWith(Array(10).fill(10), { overlays: [overlay(4, 6, 9)] }),
  ...changes,
});

describe("clicks in the strip", () => {
  test("a plain click is navigation and choosing one frame: All frames does not stick", () => {
    const { state, anchor } = seek(tenFrames({ scope: "all", selection: [1, 2] }), 5);
    expect(state).toMatchObject({ playhead: 5, selection: [], scope: "frame" });
    expect(anchor).toBe(5);
  });

  test("Shift+click picks the run back to the anchor, and a second one re-picks from it", () => {
    const first = extendSelection(tenFrames({ playhead: 5 }), 5, 2);
    expect(first.state).toMatchObject({ selection: [2, 3, 4, 5], playhead: 2, scope: "range" });
    const second = extendSelection(first.state, first.anchor, 7);
    expect(second.state.selection).toEqual([5, 6, 7]);
  });

  test("Ctrl+click adds a frame anywhere, starting from the playhead's frame", () => {
    const { state } = toggleSelection(tenFrames({ playhead: 1 }), 6);
    expect(state).toMatchObject({ selection: [1, 6], playhead: 6, scope: "range" });
  });

  test("Ctrl+clicking the last selected frame away falls back to this frame", () => {
    const { state } = toggleSelection(
      tenFrames({ playhead: 3, selection: [3], scope: "range" }),
      3,
    );
    expect(state).toMatchObject({ selection: [], scope: "frame" });
  });

  test("a press is aimed at the selection only when the frame is one of several in scope", () => {
    expect(insideSelection([2, 3], 3)).toBe(true);
    expect(insideSelection([3], 3)).toBe(false);
    expect(insideSelection([2, 3], 4)).toBe(false);
  });

  test("select all goes through the scope, not a selection", () => {
    expect(selectAllFrames(tenFrames({ selection: [1, 2] }))).toMatchObject({
      selection: [],
      scope: "all",
    });
  });
});

describe("the Shift+click anchor", () => {
  test("survives the copy an edit makes of the same selection", () => {
    expect(anchorFor({ selection: [2, 3, 4], anchor: 4 }, [2, 3, 4])).toBe(4);
  });

  test("a selection made elsewhere (a paste) anchors at its first frame", () => {
    expect(anchorFor({ selection: [2, 3], anchor: 3 }, [7, 8, 9])).toBe(7);
  });
});

describe("band picks", () => {
  test("a band click selects the overlay's whole range and brings it on screen", () => {
    const { state, anchor } = selectBand(tenFrames({ playhead: 1 }), null, 4);
    expect(state).toMatchObject({
      selectedOverlay: 4,
      selection: [6, 7, 8],
      scope: "range",
      playhead: 6,
    });
    expect(anchor).toBe(6);
  });

  test("a band click leaves a playhead already on the overlay where it is", () => {
    expect(selectBand(tenFrames({ playhead: 7 }), null, 4).state.playhead).toBe(7);
  });

  test("a click on empty band space only drops the overlay pick", () => {
    const before = tenFrames({ selectedOverlay: 4, selection: [6, 7] });
    expect(selectBand(before, 6, null).state).toMatchObject({
      selectedOverlay: null,
      selection: [6, 7],
    });
  });

  test("a right-click picks the overlay without touching the frame selection", () => {
    const state = pickOverlay(tenFrames({ playhead: 0, selection: [0, 1] }), 4);
    expect(state).toMatchObject({ selectedOverlay: 4, playhead: 6, selection: [0, 1] });
  });
});

describe("after a drag-reorder", () => {
  test("a moved run stays selected at its new home and the playhead rides its frame", () => {
    const before = tenFrames({ playhead: 3, selection: [2, 3], scope: "range" });
    const { state, anchor } = afterMove(before, 2, [2, 3], 6);
    expect(state).toMatchObject({ playhead: 7, selection: [6, 7], scope: "range" });
    expect(anchor).toBe(6);
  });

  test("a single dragged frame leaves the selection and a playhead elsewhere alone", () => {
    const { state } = afterMove(tenFrames({ playhead: 0 }), 0, [4], 1);
    expect(state).toMatchObject({ playhead: 0, selection: [] });
  });
});
