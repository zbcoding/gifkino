// ffmpeg on the page's side: asked for once, downloaded once per session
// under the busy bar, then driven in `ffmpeg.worker.ts`. The desktop finds
// ffmpeg at startup (`pipeline/caps.rs`); here it is fetched on first use.
//
// The 30 MB core comes from a public npm CDN rather than this site's own
// canister, which would pay to store it. It is the same file as the
// `@ffmpeg/core` installed for the build: `vite.config.ts` takes the version
// and SHA-256 from that copy, and bytes that hash differently are never compiled.

import type { App } from "../app";
import { fill, t } from "../i18n";
import { alertDialog } from "../ui/dialog";
import type { FfmpegReply, FfmpegRequest } from "./ffmpeg-protocol";
import { INPUT_DIR } from "./ffmpeg-protocol";

const MB = 1024 * 1024;

/** Where the core is fetched from, in order; each must be in the CSP's `connect-src` (public/_headers). */
const CORE_URLS = [
  `https://cdn.jsdelivr.net/npm/@ffmpeg/core@${__FFMPEG_CORE_VERSION__}/dist/esm/ffmpeg-core.wasm`,
  `https://unpkg.com/@ffmpeg/core@${__FFMPEG_CORE_VERSION__}/dist/esm/ffmpeg-core.wasm`,
];

/** Thrown out of a decode the user cancelled. */
export class Cancelled extends Error {}

/** The compiled core: set once the download lands, so later imports skip both questions. */
let compiled: WebAssembly.Module | null = null;

/**
 * The core, compiled; `false` when the user declined the download or
 * cancelled it. Asks first, the first time only.
 */
export async function loadFfmpeg(app: App): Promise<boolean> {
  if (compiled) return true;
  const answer = await alertDialog({
    // Translators: Title of the question asked before the first video import downloads ffmpeg.
    title: t("Download ffmpeg?"),
    body: fill(
      // Translators: Body of the question before the first ffmpeg download. jsDelivr is a CDN's name; {version} is a version number such as 0.12.10.
      t(
        "Importing video needs ffmpeg, about 30 MB. It downloads once from the jsDelivr CDN, then this browser keeps it. The download is checked against the official ffmpeg.wasm {version} package and is not used unless it matches.",
      ),
      { version: __FFMPEG_CORE_VERSION__ },
    ),
    responses: [
      { id: "cancel", label: t("Cancel") },
      // Translators: Button that starts the ffmpeg download.
      { id: "download", label: t("Download"), suggested: true },
    ],
    close: "cancel",
    default: "download",
  }).answer;
  if (answer !== "download") return false;
  const abort = new AbortController();
  try {
    const bytes = await download(app, abort);
    compiled = await WebAssembly.compile(bytes);
    return true;
  } catch (error) {
    if (abort.signal.aborted) return false;
    throw error;
  }
}

/**
 * The core's bytes from the first mirror that serves the genuine file. A
 * mirror that fails or serves anything else is skipped; the error names the
 * mismatch if any mirror had one, since that is the one worth reporting.
 */
async function download(app: App, abort: AbortController): Promise<Uint8Array<ArrayBuffer>> {
  let reason = "";
  let mismatch = false;
  for (const url of CORE_URLS) {
    try {
      const bytes = await downloadFrom(app, url, abort);
      if ((await sha256(bytes)) === __FFMPEG_CORE_SHA256__) return bytes;
      mismatch = true;
    } catch (error) {
      if (abort.signal.aborted) throw error;
      reason = error instanceof Error ? error.message : String(error);
    }
  }
  throw new Error(
    mismatch
      ? // Translators: Error when the ffmpeg file a CDN served failed its checksum check.
        t("The downloaded ffmpeg did not match the official package, so it was not used.")
      : fill(t("Could not download ffmpeg: {reason}"), { reason }),
  );
}

/** `url`'s bytes, with the busy bar counting megabytes. */
async function downloadFrom(
  app: App,
  url: string,
  abort: AbortController,
): Promise<Uint8Array<ArrayBuffer>> {
  const response = await fetch(url, { signal: abort.signal });
  if (!response.ok || !response.body) {
    throw new Error(`${new URL(url).host}: ${response.status} ${response.statusText}`);
  }
  const length = Number(response.headers.get("Content-Length")) || null;
  const label = length
    ? // Translators: Progress of the ffmpeg download; both values are megabytes.
      t("Downloading ffmpeg… {done} / {total} MB")
    : // Translators: Progress of the ffmpeg download when its size is unknown, in megabytes.
      t("Downloading ffmpeg… {done} MB");
  const total = length === null ? null : Math.round(length / MB);
  const chunks: Uint8Array[] = [];
  let received = 0;
  await app.withBusy(
    label,
    async (report) => {
      const reader = response.body!.getReader();
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        chunks.push(value);
        received += value.length;
        // A compressed response's length is shorter than the bytes it decodes to.
        report(Math.min(Math.round(received / MB), total ?? Infinity), total);
      }
    },
    () => abort.abort(),
  );
  const bytes = new Uint8Array(received);
  let at = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, at);
    at += chunk.length;
  }
  return bytes;
}

/** `bytes`' SHA-256, lowercase hex. */
async function sha256(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** Where `file` reads inside the worker. */
export function inputPath(file: File): string {
  return `${INPUT_DIR}/${file.name}`;
}

type Pending = {
  resolve(reply: FfmpegReply): void;
  reject(error: Error): void;
  onFrame?(rgba: Uint8Array<ArrayBuffer>): void;
};

let worker: Worker | null = null;
let nextId = 1;
const pending = new Map<number, Pending>();

function ffmpegWorker(): Worker {
  if (worker) return worker;
  worker = new Worker(new URL("./ffmpeg.worker.ts", import.meta.url), { type: "module" });
  worker.addEventListener("message", (event: MessageEvent<FfmpegReply>) => {
    const reply = event.data;
    const job = pending.get(reply.id);
    if (!job) return;
    if (reply.kind === "frame") return job.onFrame?.(reply.rgba);
    pending.delete(reply.id);
    if (reply.kind === "error") job.reject(new Error(reply.message));
    else job.resolve(reply);
  });
  worker.addEventListener("error", (event) => {
    for (const job of pending.values()) job.reject(new Error(event.message || "ffmpeg crashed"));
    pending.clear();
    stopWorker();
  });
  worker.postMessage({ type: "init", module: compiled! } satisfies FfmpegRequest);
  return worker;
}

function stopWorker(): void {
  worker?.terminate();
  worker = null;
}

function send<R extends FfmpegReply["kind"]>(
  request: FfmpegRequest & { id: number },
  onFrame?: Pending["onFrame"],
): Promise<Extract<FfmpegReply, { kind: R }>> {
  const { promise, resolve, reject } = Promise.withResolvers<Extract<FfmpegReply, { kind: R }>>();
  pending.set(request.id, { resolve: resolve as Pending["resolve"], reject, onFrame });
  ffmpegWorker().postMessage(request);
  return promise;
}

/** ffprobe's stdout for `args`, `file` mounted at `inputPath(file)`. */
export async function ffprobe(file: File, args: string[]): Promise<string[]> {
  const reply = await send<"lines">({ type: "probe", id: nextId++, file, args });
  return reply.lines;
}

export interface DecodeJob {
  runs: string[][];
  frameBytes: number;
  dropNth: number | null;
  cap: number;
}

/** Run `job`'s ffmpeg runs, handing each kept frame to `onFrame` as it is written. */
export function decodeFrames(
  file: File,
  job: DecodeJob,
  onFrame: (rgba: Uint8Array<ArrayBuffer>) => void,
): Promise<{ decoded: number; kept: number; failed: boolean }> {
  return send<"done">({ type: "decode", id: nextId++, file, ...job }, onFrame);
}

/**
 * Stop whatever ffmpeg is running: the core cannot be interrupted, so the
 * worker goes and the next job starts a fresh one from the compiled module.
 */
export function cancelFfmpeg(): void {
  for (const job of pending.values()) job.reject(new Cancelled());
  pending.clear();
  stopWorker();
}
