// Document snapshots for tests: the fields the page reads, nothing decoded.

import type { DocSnapshot } from "./worker/protocol";

export function docWith(delaysCs: number[], changes: Partial<DocSnapshot> = {}): DocSnapshot {
  return {
    revision: 1,
    width: 40,
    height: 30,
    frameCount: delaysCs.length,
    durationCs: delaysCs.reduce((sum, delay) => sum + delay, 0),
    hasAlpha: false,
    frames: delaysCs.map((delayCs, key) => ({ delayCs, detached: false, opaque: true, key })),
    overlays: [],
    canUndo: false,
    canRedo: false,
    undoLabel: null,
    redoLabel: null,
    clipboardLength: 0,
    loopFadeLimit: Math.floor(delaysCs.length / 2),
    ...changes,
  };
}
