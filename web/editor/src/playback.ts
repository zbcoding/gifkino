// Play/pause. Like the desktop, each frame waits its own delay before the
// playhead moves on, wrapping at the end; one frame has nothing to play.

import type { AppState, Store } from "./state";

/** GIF players treat a 0 cs delay as the shortest they allow; so does the desktop. */
const MIN_DELAY_CS = 1;

export function togglePlay(store: Store<AppState>): void {
  const { playing, doc } = store.state;
  store.update({ playing: !playing && (doc?.frameCount ?? 0) > 1 });
}

/**
 * Advance the playhead while `playing` is set, rescheduling per frame. Seeking
 * while playing keeps the running wait; the next frame waits its own delay.
 */
export function startPlayback(store: Store<AppState>): () => void {
  let timer: number | undefined;

  const tick = () => {
    timer = undefined;
    const { playing, doc, playhead } = store.state;
    if (!playing || !doc) return;
    store.update({ playhead: (playhead + 1) % doc.frameCount });
  };

  const schedule = (state: AppState) => {
    if (!state.playing) {
      clearTimeout(timer);
      timer = undefined;
      return;
    }
    if (timer !== undefined || !state.doc) return;
    const delayCs = state.doc.frames[state.playhead]?.delayCs ?? MIN_DELAY_CS;
    timer = setTimeout(tick, Math.max(delayCs, MIN_DELAY_CS) * 10);
  };

  return store.subscribe(schedule);
}
