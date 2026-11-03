// The messages between the page and the worker that owns the wasm `Editor`.
// Only plain data crosses: wasm-owned `Frames` and `Image` stay in the worker.

import type {
  Change,
  contains,
  delaysForFps,
  Editor,
  fitCanvas,
  handleAt,
  orientedCorners,
  ExportSettings,
  FitMode,
  FrameInfo,
  Overlay,
} from "../wasm/gifkino_wasm.js";

/** Pixels the page can put on a canvas: `new ImageData(data, width, height)`. */
export interface Picture {
  width: number;
  height: number;
  data: Uint8ClampedArray<ArrayBuffer>;
}

/** Decoded-elsewhere input for `stage`. `rgba` frames are `width*height*4` bytes each. */
export type FrameSource =
  | { kind: "gif"; bytes: Uint8Array<ArrayBuffer> }
  | { kind: "image"; bytes: Uint8Array<ArrayBuffer> }
  | {
      kind: "rgba";
      width: number;
      height: number;
      frames: { rgba: Uint8Array<ArrayBuffer>; delayCs: number }[];
    };

/** Frames decoded and held in the worker until `open` or `splice` takes them. */
export interface Staged {
  length: number;
  width: number;
  height: number;
  /** `Editor.needsFit`: a splice must ask the fit question. */
  needsFit: boolean;
}

/** Everything the UI reads about the document, refreshed after every edit. */
export interface DocSnapshot {
  revision: number;
  width: number;
  height: number;
  frameCount: number;
  durationCs: number;
  hasAlpha: boolean;
  frames: FrameInfo[];
  overlays: Overlay[];
  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
  clipboardLength: number;
  loopFadeLimit: number;
}

/** Methods that pass plain data both ways, callable by name through `call`. */
export type EditorMethod = {
  [K in keyof Editor]: Editor[K] extends (...args: never[]) => unknown ? K : never;
}[keyof Editor] &
  string &
  Exclude<
    keyof Editor,
    | "free"
    | "open"
    | "splice"
    | "needsFit"
    | "setTextRasterizer"
    | "exportGif"
    | "composite"
    | "thumbnail"
  >;

/** The module's free functions (geometry and timing helpers), callable through `compute`. */
export interface FreeFunctions {
  handleAt: typeof handleAt;
  contains: typeof contains;
  orientedCorners: typeof orientedCorners;
  fitCanvas: typeof fitCanvas;
  delaysForFps: typeof delaysForFps;
}
export type FreeFunction = keyof FreeFunctions;

export type MethodArgs<M extends EditorMethod> = Editor[M] extends (...args: infer A) => unknown
  ? A
  : never;
export type MethodResult<M extends EditorMethod> = Editor[M] extends (...args: never[]) => infer R
  ? R
  : never;

/** Each operation's parameters and result. `progress` lists what it reports. */
export interface Ops {
  call: { params: { method: EditorMethod; args: unknown[] }; result: unknown };
  compute: { params: { name: FreeFunction; args: unknown[] }; result: unknown };
  /** Progress: frames decoded so far. */
  stage: { params: { source: FrameSource }; result: Staged };
  open: { params: object; result: undefined };
  splice: { params: { at: number; fit: FitMode }; result: Change | undefined };
  composite: { params: { index: number }; result: Picture | undefined };
  thumbnail: { params: { index: number }; result: Picture | undefined };
  /** Progress: frames done, frame total. */
  exportGif: {
    params: { settings: ExportSettings | null };
    result: Uint8Array<ArrayBuffer>;
  };
}
export type Op = keyof Ops;

export type Request = {
  [K in Op]: { id: number; op: K; snapshot: boolean } & Ops[K]["params"];
}[Op];

export type Response =
  | { id: number; kind: "result"; value: unknown; doc: DocSnapshot | null }
  | { id: number; kind: "error"; message: string }
  | { id: number; kind: "progress"; values: number[] };
