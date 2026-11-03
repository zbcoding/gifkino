import { afterEach, beforeEach, expect, test, vi } from "vite-plus/test";
import { startPlayback, togglePlay } from "./playback";
import { initialState, Store } from "./state";
import type { AppState } from "./state";
import { docWith } from "./test-support";

let store: Store<AppState>;
let stop: () => void;

beforeEach(() => {
  vi.useFakeTimers();
  store = new Store<AppState>({ ...initialState, doc: docWith([10, 50, 0]) });
  stop = startPlayback(store);
});

afterEach(() => {
  stop();
  vi.useRealTimers();
});

test("each frame shows for its own delay before the playhead moves on", () => {
  togglePlay(store);
  vi.advanceTimersByTime(99);
  expect(store.state.playhead).toBe(0);
  vi.advanceTimersByTime(1);
  expect(store.state.playhead).toBe(1);
  // Frame 1 waits 50 cs, not frame 0's 10.
  vi.advanceTimersByTime(499);
  expect(store.state.playhead).toBe(1);
  vi.advanceTimersByTime(1);
  expect(store.state.playhead).toBe(2);
});

test("a 0 cs frame still waits the 1 cs minimum, then playback wraps to the start", () => {
  store.update({ playhead: 2 });
  togglePlay(store);
  vi.advanceTimersByTime(9);
  expect(store.state.playhead).toBe(2);
  vi.advanceTimersByTime(1);
  expect(store.state.playhead).toBe(0);
});

test("pausing cancels the pending advance", () => {
  togglePlay(store);
  vi.advanceTimersByTime(50);
  togglePlay(store);
  vi.advanceTimersByTime(1000);
  expect(store.state).toMatchObject({ playing: false, playhead: 0 });
});

test("a single frame has nothing to play", () => {
  store.update({ doc: docWith([10]) });
  togglePlay(store);
  expect(store.state.playing).toBe(false);
});
