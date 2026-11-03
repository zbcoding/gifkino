// Answers `Request`s against one wasm `Editor`. Kept apart from the worker
// entry so it runs under test with a stand-in editor.

import type { Editor, Frames, Image } from "../wasm/gifkino_wasm.js";
import type {
  DocSnapshot,
  FreeFunctions,
  FrameSource,
  Op,
  Ops,
  Picture,
  Request,
  Response,
} from "./protocol";

export type Post = (response: Response, transfer?: Transferable[]) => void;
export type Decode = (source: FrameSource, progress: (decoded: number) => void) => Frames;

type Handlers = {
  [K in Op]: (
    request: Extract<Request, { op: K }>,
    progress: (...values: number[]) => void,
  ) => Ops[K]["result"];
};

export function createHandler(
  editor: Editor,
  decode: Decode,
  functions: Partial<FreeFunctions> = {},
) {
  let staged: Frames | null = null;

  const takeStaged = (): Frames => {
    if (!staged) throw new Error("No frames staged");
    const frames = staged;
    staged = null;
    return frames;
  };

  const handlers: Handlers = {
    call: ({ method, args }) =>
      (editor[method] as (...a: unknown[]) => unknown).apply(editor, args),
    compute: ({ name, args }) => (functions[name] as (...a: unknown[]) => unknown)(...args),
    stage: ({ source }, progress) => {
      staged?.free();
      staged = null;
      const frames = decode(source, progress);
      staged = frames;
      return {
        length: frames.length,
        width: frames.width,
        height: frames.height,
        needsFit: editor.needsFit(frames),
      };
    },
    open: () => {
      const frames = takeStaged();
      editor.open(frames);
      frames.free();
      return undefined;
    },
    splice: ({ at, fit }) => {
      const frames = takeStaged();
      const change = editor.splice(frames, at, fit);
      frames.free();
      return change;
    },
    composite: ({ index }) => toPicture(editor.composite(index)),
    thumbnail: ({ index }) => toPicture(editor.thumbnail(index)),
    exportGif: ({ settings }, progress) =>
      editor.exportGif(settings, (done, total) => progress(done, total)) as Uint8Array<ArrayBuffer>,
  };

  return function handle(request: Request, post: Post): void {
    const { id } = request;
    const progress = (...values: number[]) => post({ id, kind: "progress", values });
    try {
      const run = handlers[request.op] as (r: Request, p: typeof progress) => unknown;
      const value = run(request, progress);
      const doc = request.snapshot ? snapshot(editor) : null;
      post({ id, kind: "result", value, doc }, transferables(value));
    } catch (error) {
      post({ id, kind: "error", message: error instanceof Error ? error.message : String(error) });
    }
  };
}

export function snapshot(editor: Editor): DocSnapshot {
  return {
    revision: editor.revision,
    width: editor.width,
    height: editor.height,
    frameCount: editor.frameCount,
    durationCs: editor.durationCs,
    hasAlpha: editor.hasAlpha,
    frames: editor.frames(),
    overlays: editor.overlays(),
    canUndo: editor.canUndo,
    canRedo: editor.canRedo,
    undoLabel: editor.undoLabel ?? null,
    redoLabel: editor.redoLabel ?? null,
    clipboardLength: editor.clipboardLength,
    loopFadeLimit: editor.loopFadeLimit,
  };
}

function toPicture(image: Image | undefined): Picture | undefined {
  if (!image) return undefined;
  const picture = {
    width: image.width,
    height: image.height,
    data: image.data as Uint8ClampedArray<ArrayBuffer>,
  };
  image.free();
  return picture;
}

/** Pixel and file buffers move to the page instead of being copied. */
function transferables(value: unknown): Transferable[] {
  if (value instanceof Uint8Array) return [value.buffer];
  if (isPicture(value)) return [value.data.buffer];
  return [];
}

function isPicture(value: unknown): value is Picture {
  return typeof value === "object" && value !== null && "data" in value && "width" in value;
}
