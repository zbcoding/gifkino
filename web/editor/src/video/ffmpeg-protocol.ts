// Messages between the page and `ffmpeg.worker.ts`.

/** Where the worker mounts the file being read; it appears under its own name. */
export const INPUT_DIR = "/input";
/** The device file ffmpeg writes raw RGBA to, read out as it is written. */
export const FRAMES_DEVICE = "/frames";

export type FfmpegRequest =
  | { type: "init"; module: WebAssembly.Module }
  | { type: "probe"; id: number; file: File; args: string[] }
  | {
      type: "decode";
      id: number;
      file: File;
      /** ffmpeg runs, one after the other, all writing to `FRAMES_DEVICE`. */
      runs: string[][];
      frameBytes: number;
      /** Drop every Nth frame as it arrives (desktop `import_planned`). */
      dropNth: number | null;
      /** Stop keeping frames after this many. */
      cap: number;
    };

export type FfmpegReply =
  | { id: number; kind: "lines"; lines: string[] }
  | { id: number; kind: "frame"; rgba: Uint8Array<ArrayBuffer> }
  | { id: number; kind: "done"; decoded: number; kept: number; failed: boolean }
  | { id: number; kind: "error"; message: string };
