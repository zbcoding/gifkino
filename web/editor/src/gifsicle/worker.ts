// gifsicle's worker: one optimization per worker, off the page's thread and
// off the editor worker, so the document stays responsive while it runs.

import { optimize } from "./optimize";

export interface GifsicleRequest {
  gif: Uint8Array<ArrayBuffer>;
  lossy: number;
}
export type GifsicleReply = { gif: Uint8Array<ArrayBuffer> } | { error: string };

self.onmessage = async ({ data }: MessageEvent<GifsicleRequest>) => {
  let reply: GifsicleReply;
  try {
    // Not compileStreaming: that needs the server to label it application/wasm.
    const wasm = await fetch(new URL("./gifsicle.wasm", import.meta.url));
    const module = await WebAssembly.compile(await wasm.arrayBuffer());
    reply = { gif: optimize(module, data.gif, data.lossy) };
  } catch (error) {
    reply = { error: error instanceof Error ? error.message : String(error) };
  }
  self.postMessage(reply, { transfer: "gif" in reply ? [reply.gif.buffer] : [] });
};
