// The page's handle on gifsicle: loaded on the first export, never before.

import type { GifsicleReply, GifsicleRequest } from "./worker";

/** `gifsicle -O3 [--lossy=N]` over `gif` in a worker of its own; rejects if it fails to load or run. */
export function runGifsicle(gif: Uint8Array<ArrayBuffer>, lossy: number) {
  const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  const { promise, resolve, reject } = Promise.withResolvers<Uint8Array<ArrayBuffer>>();
  worker.onmessage = ({ data }: MessageEvent<GifsicleReply>) =>
    "gif" in data ? resolve(data.gif) : reject(new Error(data.error));
  worker.onerror = (event) => reject(new Error(event.message || "gifsicle worker failed"));
  worker.postMessage({ gif, lossy } satisfies GifsicleRequest);
  return promise.finally(() => worker.terminate());
}
