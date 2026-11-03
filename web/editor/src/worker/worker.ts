// Worker entry: owns the one wasm `Editor`, whose calls are synchronous and
// can take seconds, so the page never runs them on its own thread.

import init, {
  delaysForFps,
  Editor,
  fitCanvas,
  Frames,
  handleAt,
  orientedCorners,
} from "../wasm/gifkino_wasm.js";
import type { Decode, Post } from "./serve";
import { createHandler } from "./serve";
import type { Request } from "./protocol";
import { rasterizeText } from "./text";

const decode: Decode = (source, progress) => {
  switch (source.kind) {
    case "gif":
      return Frames.fromGif(source.bytes, (decoded) => {
        progress(decoded);
        return true;
      });
    case "image":
      return Frames.fromImage(source.bytes);
    case "rgba": {
      const frames = new Frames();
      for (const { rgba, delayCs } of source.frames) {
        frames.push(source.width, source.height, rgba, delayCs);
      }
      return frames;
    }
  }
};

const ready = init().then(() => {
  const editor = new Editor();
  editor.setTextRasterizer(rasterizeText);
  return createHandler(editor, decode, { handleAt, orientedCorners, fitCanvas, delaysForFps });
});

const post: Post = (response, transfer = []) => self.postMessage(response, { transfer });

// `then` callbacks run in registration order, so requests stay in order too.
self.addEventListener("message", (event: MessageEvent<Request>) => {
  ready.then(
    (handle) => handle(event.data, post),
    (error) =>
      post({ id: event.data.id, kind: "error", message: `Editor module failed to load: ${error}` }),
  );
});
