// ffmpeg.wasm's single-thread core in its own worker: the desktop's ffprobe
// and `ffmpeg … -f rawvideo -pix_fmt rgba -` pipe. Frames leave as they are
// written, through a device file, so the decoded clip never sits in ffmpeg's
// own heap (which tops out at 2 GiB).

import createCore from "@ffmpeg/core";
import type { Core } from "@ffmpeg/core";
import type { FfmpegReply, FfmpegRequest } from "./ffmpeg-protocol";
import { FRAMES_DEVICE, INPUT_DIR } from "./ffmpeg-protocol";

/** Where the bytes ffmpeg writes to `FRAMES_DEVICE` go during one run. */
let sink: (bytes: Uint8Array) => void = () => {};
/** Stdout lines of the run in progress: ffprobe's answer. */
let stdout: string[] = [];

const post = (reply: FfmpegReply, transfer: Transferable[] = []) =>
  self.postMessage(reply, { transfer });

async function load(module: WebAssembly.Module): Promise<Core> {
  const core = await createCore({
    // Compiled once on the page, so a worker restarted after a cancel does
    // not download or compile the 30 MB again.
    instantiateWasm(imports: WebAssembly.Imports, done: (instance: WebAssembly.Instance) => void) {
      void WebAssembly.instantiate(module, imports).then(done);
      return {};
    },
  });
  core.setLogger(({ type, message }) => {
    if (type === "stdout") stdout.push(message);
  });
  const { FS } = core;
  FS.mkdir(INPUT_DIR);
  const dev = FS.makedev(64, 0);
  FS.registerDevice(dev, {
    open() {},
    close() {},
    read: () => 0,
    llseek: () => 0,
    // `buffer` is the core's whole heap; copy out just this write.
    write(_stream: unknown, buffer: Int8Array, offset: number, length: number) {
      sink(new Uint8Array(buffer.buffer, buffer.byteOffset + offset, length));
      return length;
    },
  });
  FS.mkdev(FRAMES_DEVICE, dev);
  return core;
}

/** Run `job` with `file` readable under `INPUT_DIR`, without copying it in. */
function withInput<T>(core: Core, file: File, job: () => T): T {
  core.FS.mount(core.FS.filesystems.WORKERFS, { files: [file] }, INPUT_DIR);
  try {
    return job();
  } finally {
    core.FS.unmount(INPUT_DIR);
    core.reset();
  }
}

/**
 * Desktop `import_planned`'s read loop: whole frames off the pipe, every
 * `dropNth`th dropped before it costs memory, stopping at `cap` kept.
 */
function decode(core: Core, request: Extract<FfmpegRequest, { type: "decode" }>) {
  const { id, frameBytes, dropNth, cap } = request;
  let frame = new Uint8Array(frameBytes);
  let filled = 0;
  let decoded = 0;
  let kept = 0;
  sink = (bytes) => {
    for (let at = 0; at < bytes.length && kept < cap;) {
      const take = Math.min(frameBytes - filled, bytes.length - at);
      frame.set(bytes.subarray(at, at + take), filled);
      filled += take;
      at += take;
      if (filled < frameBytes) continue;
      filled = 0;
      decoded += 1;
      if (dropNth !== null && decoded % dropNth === 0) continue;
      kept += 1;
      post({ id, kind: "frame", rgba: frame }, [frame.buffer]);
      frame = new Uint8Array(frameBytes);
    }
  };
  let failed = false;
  withInput(core, request.file, () => {
    for (const args of request.runs) {
      core.exec(...args);
      failed ||= core.ret !== 0;
      core.reset();
      filled = 0;
    }
  });
  sink = () => {};
  post({ id, kind: "done", decoded, kept, failed });
}

function probe(core: Core, id: number, file: File, args: string[]) {
  stdout = [];
  withInput(core, file, () => core.ffprobe(...args));
  post({ id, kind: "lines", lines: stdout });
}

let core: Promise<Core> | null = null;

// One job at a time, in order: each waits on the same loaded core, and the
// jobs themselves are synchronous.
self.addEventListener("message", (event: MessageEvent<FfmpegRequest>) => {
  const request = event.data;
  if (request.type === "init") {
    core = load(request.module);
    return;
  }
  void core!.then(
    (loaded) => {
      try {
        if (request.type === "probe") probe(loaded, request.id, request.file, request.args);
        else decode(loaded, request);
      } catch (error) {
        post({ id: request.id, kind: "error", message: String(error) });
      }
    },
    (error) => post({ id: request.id, kind: "error", message: `ffmpeg failed to load: ${error}` }),
  );
});
