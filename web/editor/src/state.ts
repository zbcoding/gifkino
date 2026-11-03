// Window state. The document lives in the worker; `doc` is its latest
// snapshot. Everything else (playhead, selection, scope, tool) is the page's,
// as API.md puts it: window state stays in JavaScript.

import type { DocSnapshot } from "./worker/protocol";

/** The transport's scope buttons: desktop `ScopeChoice`. */
export type Scope = "frame" | "all" | "range";

export interface Busy {
  label: string;
  done: number;
  /** Unknown totals show a pulsing bar. */
  total: number | null;
  /** Shows the busy bar's Cancel button (desktop `import_cancel`). */
  cancel?: () => void;
}

export interface AppState {
  /** `null` until a file is open: the welcome page. */
  doc: DocSnapshot | null;
  fileName: string | null;
  playhead: number;
  /** Strip selection, sorted frame indices. */
  selection: number[];
  scope: Scope;
  /** The canvas tool a slice registered, or `null` for plain picking. */
  activeTool: string | null;
  selectedOverlay: number | null;
  playing: boolean;
  /** Edited since it was opened or last exported. */
  dirty: boolean;
  busy: Busy | null;
  /** Export size measured at a document revision. */
  estimate: { revision: number; bytes: number } | null;
}

export const initialState: AppState = {
  doc: null,
  fileName: null,
  playhead: 0,
  selection: [],
  scope: "frame",
  activeTool: null,
  selectedOverlay: null,
  playing: false,
  dirty: false,
  busy: null,
  estimate: null,
};

export type Listener<T> = (state: T, previous: T) => void;

/** Holds one immutable state value and tells subscribers when it is replaced. */
export class Store<T> {
  #state: T;
  #listeners = new Set<Listener<T>>();

  constructor(state: T) {
    this.#state = state;
  }

  get state(): T {
    return this.#state;
  }

  update(change: Partial<T> | ((state: T) => T)): void {
    const previous = this.#state;
    this.#state = typeof change === "function" ? change(previous) : { ...previous, ...change };
    if (this.#state === previous) return;
    for (const listener of this.#listeners) listener(this.#state, previous);
  }

  /** Returns the unsubscribe function. */
  subscribe(listener: Listener<T>): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }
}

/**
 * A freshly opened document: everything that pointed into the old one resets,
 * and nothing is unsaved yet.
 */
export function opened(state: AppState, doc: DocSnapshot, fileName: string): AppState {
  return {
    ...initialState,
    activeTool: state.activeTool,
    doc,
    fileName,
  };
}

/**
 * The document after an edit, undo or redo. Indices into it are clamped the
 * way the desktop does after any edit: playhead to the last frame, selection
 * and overlay dropped where they point past the end.
 */
export function edited(state: AppState, doc: DocSnapshot): AppState {
  const count = doc.frameCount;
  const overlayExists = doc.overlays.some((overlay) => overlay.id === state.selectedOverlay);
  return {
    ...state,
    doc,
    playhead: Math.min(state.playhead, Math.max(0, count - 1)),
    selection: state.selection.filter((index) => index < count),
    selectedOverlay: overlayExists ? state.selectedOverlay : null,
    playing: state.playing && count > 1,
    dirty: state.dirty || doc.revision !== state.doc?.revision,
  };
}

/** Desktop `Scope::resolve`: the frame indices an edit in this scope touches. */
export function framesInScope(state: AppState): number[] {
  const count = state.doc?.frameCount ?? 0;
  const thisFrame = state.playhead < count ? [state.playhead] : [];
  switch (state.scope) {
    case "frame":
      return thisFrame;
    case "all":
      return Array.from({ length: count }, (_, i) => i);
    case "range":
      return state.selection.length > 0 ? state.selection.filter((i) => i < count) : thisFrame;
  }
}

/** Time before the playhead's frame, in centiseconds: the transport clock. */
export function elapsedCs(state: AppState): number {
  const frames = state.doc?.frames ?? [];
  return frames.slice(0, state.playhead).reduce((sum, frame) => sum + frame.delayCs, 0);
}
