import { afterEach, expect, test } from "vite-plus/test";
import { createClient } from "./rpc";
import type { Port } from "./rpc";
import { createHandler } from "./worker/serve";
import type { Decode } from "./worker/serve";
import type { Request } from "./worker/protocol";
import type { Editor, Frames } from "./wasm/gifkino_wasm.js";

// The real handler behind a real MessageChannel, with an editor stand-in:
// what crosses is exactly what crosses between the page and the worker.
const channels: MessageChannel[] = [];

function connect(editor: Partial<Editor>, decode: Decode = () => ({}) as Frames) {
  const channel = new MessageChannel();
  channels.push(channel);
  const handle = createHandler(editor as Editor, decode);
  channel.port2.onmessage = (event: MessageEvent<Request>) =>
    handle(event.data, (response, transfer) => channel.port2.postMessage(response, transfer ?? []));
  channel.port1.start();
  return createClient(channel.port1 as unknown as Port);
}

afterEach(() => {
  for (const channel of channels.splice(0)) channel.port1.close();
});

const stubSnapshot: Partial<Editor> = {
  revision: 4,
  frameCount: 2,
  frames: () => [],
  overlays: () => [],
};

test("a method that throws rejects the caller's promise with the editor's message", async () => {
  const request = connect({
    resize: () => {
      throw new Error("invalid type: expected u32");
    },
  });
  await expect(request("call", { method: "resize", args: [-1, 5] })).rejects.toThrow(
    "invalid type: expected u32",
  );
});

test("a failed request does not poison the next one", async () => {
  let calls = 0;
  const request = connect({
    ...stubSnapshot,
    undo: () => {
      calls += 1;
      if (calls === 1) throw new Error("first fails");
      return true;
    },
  });
  await expect(request("call", { method: "undo", args: [] })).rejects.toThrow("first fails");
  const reply = await request("call", { method: "undo", args: [] }, { snapshot: true });
  expect(reply.value).toBe(true);
  expect(reply.doc).toMatchObject({ revision: 4, frameCount: 2 });
});

test("an edit's reply carries the document as it stands after the edit", async () => {
  const editor = { ...stubSnapshot, frameCount: 3 };
  const request = connect({
    ...editor,
    duplicateFrames(frames) {
      editor.frameCount = 3 + (frames as number[]).length;
      return { label: "Frames duplicated", framesTouched: 1 };
    },
    get frameCount() {
      return editor.frameCount;
    },
  });
  const reply = await request(
    "call",
    { method: "duplicateFrames", args: [[0]] },
    { snapshot: true },
  );
  expect(reply.value).toEqual({ label: "Frames duplicated", framesTouched: 1 });
  expect(reply.doc?.frameCount).toBe(4);
});

test("export progress reaches the caller before the file does", async () => {
  const request = connect({
    exportGif: (_settings, progress) => {
      progress?.(1, 2);
      progress?.(2, 2);
      return new Uint8Array([71, 73, 70]);
    },
  });
  const seen: number[][] = [];
  const reply = await request(
    "exportGif",
    { settings: null },
    { progress: (...values) => seen.push(values) },
  );
  expect(seen).toEqual([
    [1, 2],
    [2, 2],
  ]);
  expect([...reply.value]).toEqual([71, 73, 70]);
});

test("open without staged frames is an error, not a silently empty document", async () => {
  const request = connect({ open: () => {} });
  await expect(request("open", {})).rejects.toThrow("No frames staged");
});

test("staged frames are opened once and freed", async () => {
  let freed = 0;
  const frames = { length: 8, width: 4, height: 3, free: () => (freed += 1) } as unknown as Frames;
  const opened: Frames[] = [];
  const request = connect(
    { ...stubSnapshot, needsFit: () => false, open: (f) => void opened.push(f) },
    () => frames,
  );
  const staged = await request("stage", { source: { kind: "gif", bytes: new Uint8Array(1) } });
  expect(staged.value).toEqual({ length: 8, width: 4, height: 3, needsFit: false });
  await request("open", {}, { snapshot: true });
  expect(opened).toEqual([frames]);
  expect(freed).toBe(1);
  await expect(request("open", {})).rejects.toThrow("No frames staged");
});
