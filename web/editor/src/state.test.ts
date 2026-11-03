import { describe, expect, test } from "vite-plus/test";
import { edited, elapsedCs, framesInScope, initialState, opened, Store } from "./state";
import type { AppState } from "./state";
import { docWith } from "./test-support";
import type { Overlay } from "./wasm/gifkino_wasm.js";

const overlay = (id: number): Overlay => ({
  id,
  name: "Text",
  kind: { type: "shape", shape: "rect", fill: null, stroke: null },
  range: { start: 0, end: 1 },
  transform: { x: 0, y: 0, w: 1, h: 1, angle: 0 },
  opacity: 1,
  hidden: false,
});

const tenFrames = docWith(Array(10).fill(10));
const atFrame = (playhead: number, changes: Partial<AppState> = {}): AppState => ({
  ...initialState,
  doc: tenFrames,
  playhead,
  ...changes,
});

describe("edited: indices follow the document the way the desktop clamps them", () => {
  test("deleting the frames under the playhead moves it to the new last frame", () => {
    const after = edited(atFrame(9), docWith(Array(4).fill(10), { revision: 2 }));
    expect(after.playhead).toBe(3);
  });

  test("an emptied document parks the playhead at 0, not -1", () => {
    expect(edited(atFrame(5), docWith([], { revision: 2 })).playhead).toBe(0);
  });

  test("selected frames past the end are dropped, the rest kept", () => {
    const after = edited(atFrame(0, { selection: [1, 5, 8] }), docWith(Array(6).fill(10)));
    expect(after.selection).toEqual([1, 5]);
  });

  test("a deleted overlay stops being selected; a surviving one stays", () => {
    const state = atFrame(0, { selectedOverlay: 7 });
    expect(edited(state, docWith([10], { overlays: [overlay(3)] })).selectedOverlay).toBeNull();
    expect(edited(state, docWith([10], { overlays: [overlay(7)] })).selectedOverlay).toBe(7);
  });

  test("playback stops when an edit leaves one frame, which has nothing to play", () => {
    expect(edited(atFrame(0, { playing: true }), docWith([10])).playing).toBe(false);
  });

  test("dirty once the revision moves, and it stays dirty after a later no-op", () => {
    const changed = edited(atFrame(0), { ...tenFrames, revision: 2 });
    expect(changed.dirty).toBe(true);
    expect(edited(changed, { ...tenFrames, revision: 2 }).dirty).toBe(true);
    // A refresh that changed nothing (a failed or no-op edit) is not an edit.
    expect(edited(atFrame(0), tenFrames).dirty).toBe(false);
  });
});

test("opening a file starts clean: nothing selected, unsaved or measured", () => {
  const busy: AppState = atFrame(6, {
    selection: [2, 3],
    scope: "range",
    selectedOverlay: 1,
    dirty: true,
    estimate: { revision: 1, bytes: 5 },
    activeTool: "text",
  });
  const after = opened(busy, docWith([10, 10]), "cat.gif");
  expect(after).toMatchObject({
    playhead: 0,
    selection: [],
    scope: "frame",
    selectedOverlay: null,
    dirty: false,
    estimate: null,
    fileName: "cat.gif",
  });
  expect(after.activeTool).toBe("text");
});

describe("framesInScope matches desktop Scope::resolve", () => {
  test("this frame is the playhead alone", () => {
    expect(framesInScope(atFrame(4))).toEqual([4]);
  });

  test("all frames is every index", () => {
    expect(framesInScope(atFrame(4, { scope: "all" }))).toHaveLength(10);
  });

  test("range is the selection, or the playhead while nothing is selected", () => {
    expect(framesInScope(atFrame(4, { scope: "range", selection: [2, 7] }))).toEqual([2, 7]);
    expect(framesInScope(atFrame(4, { scope: "range" }))).toEqual([4]);
  });

  test("no document, no frames", () => {
    expect(framesInScope(initialState)).toEqual([]);
  });
});

test("the clock counts the delays before the playhead's frame", () => {
  const state = { ...initialState, doc: docWith([10, 20, 30]), playhead: 2 };
  expect(elapsedCs(state)).toBe(30);
});

test("store notifies with the previous state, and not for an unchanged one", () => {
  const store = new Store({ n: 1 });
  const seen: [number, number][] = [];
  store.subscribe((state, previous) => seen.push([previous.n, state.n]));
  store.update({ n: 2 });
  store.update((state) => state);
  expect(seen).toEqual([[1, 2]]);
});
