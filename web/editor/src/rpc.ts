// The page's side of the worker protocol: one promise per request, settled by
// the worker's result or error, with progress reports delivered in between.

import type { DocSnapshot, Op, Ops, Request, Response } from "./worker/protocol";

/** What the client needs of a `Worker` (or, under test, a `MessagePort`). */
export interface Port {
  postMessage(message: Request, transfer: Transferable[]): void;
  addEventListener(type: "message", listener: (event: MessageEvent<Response>) => void): void;
}

export interface Reply<T> {
  value: T;
  /** The document after the request, when it asked for a snapshot. */
  doc: DocSnapshot | null;
}

export interface RequestOptions {
  snapshot?: boolean;
  progress?: (...values: number[]) => void;
  /** Buffers in the params to move rather than copy. */
  transfer?: Transferable[];
}

export type Client = <K extends Op>(
  op: K,
  params: Ops[K]["params"],
  options?: RequestOptions,
) => Promise<Reply<Ops[K]["result"]>>;

interface Pending {
  resolve: (reply: Reply<unknown>) => void;
  reject: (error: Error) => void;
  progress?: (...values: number[]) => void;
}

export function createClient(port: Port): Client {
  const pending = new Map<number, Pending>();
  let nextId = 0;

  port.addEventListener("message", ({ data }) => {
    const waiting = pending.get(data.id);
    if (!waiting) return;
    if (data.kind === "progress") {
      waiting.progress?.(...data.values);
      return;
    }
    pending.delete(data.id);
    if (data.kind === "error") waiting.reject(new Error(data.message));
    else waiting.resolve({ value: data.value, doc: data.doc });
  });

  return <K extends Op>(op: K, params: Ops[K]["params"], options: RequestOptions = {}) => {
    const id = nextId++;
    const request = { ...params, id, op, snapshot: options.snapshot ?? false } as Request;
    const { promise, resolve, reject } = Promise.withResolvers<Reply<Ops[K]["result"]>>();
    pending.set(id, { resolve: resolve as Pending["resolve"], reject, progress: options.progress });
    port.postMessage(request, options.transfer ?? []);
    return promise;
  };
}
