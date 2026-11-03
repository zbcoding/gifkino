// Just enough WASI preview 1 to run a command-line program over pipes: argv,
// stdin from bytes, stdout and stderr collected, no files and no environment.
// gifsicle reads its GIF on stdin and writes the result to stdout.

export interface CommandOutput {
  exitCode: number;
  stdout: Uint8Array<ArrayBuffer>;
  stderr: string;
}

const SUCCESS = 0;
const EBADF = 8;
const ENOENT = 44;
const ESPIPE = 70;
const FILETYPE_REGULAR_FILE = 4; // not a character device, so isatty() is false

class Exit {
  code: number;
  constructor(code: number) {
    this.code = code;
  }
}

/** Run a WASI command module to completion. */
export function runCommand(
  module: WebAssembly.Module,
  args: string[],
  stdin: Uint8Array,
): CommandOutput {
  const encoder = new TextEncoder();
  const argv = args.map((arg) => encoder.encode(`${arg}\0`));
  const stdout: Uint8Array[] = [];
  const stderr: Uint8Array[] = [];
  let read = 0;
  let memory: WebAssembly.Memory;
  const view = () => new DataView(memory.buffer);
  const bytes = (ptr: number, len: number) => new Uint8Array(memory.buffer, ptr, len);

  /** Each (pointer, length) pair of an iovec array. */
  function* iovecs(iovs: number, count: number) {
    for (let i = 0; i < count; i++) {
      yield bytes(view().getUint32(iovs + i * 8, true), view().getUint32(iovs + i * 8 + 4, true));
    }
  }

  const wasi = {
    args_sizes_get(countPtr: number, sizePtr: number) {
      view().setUint32(countPtr, argv.length, true);
      view().setUint32(
        sizePtr,
        argv.reduce((sum, arg) => sum + arg.length, 0),
        true,
      );
      return SUCCESS;
    },
    args_get(argvPtr: number, bufPtr: number) {
      argv.forEach((arg, i) => {
        view().setUint32(argvPtr + i * 4, bufPtr, true);
        bytes(bufPtr, arg.length).set(arg);
        bufPtr += arg.length;
      });
      return SUCCESS;
    },
    environ_sizes_get(countPtr: number, sizePtr: number) {
      view().setUint32(countPtr, 0, true);
      view().setUint32(sizePtr, 0, true);
      return SUCCESS;
    },
    environ_get: () => SUCCESS,
    clock_time_get(_id: number, _precision: bigint, timePtr: number) {
      view().setBigUint64(timePtr, BigInt(Date.now()) * 1_000_000n, true);
      return SUCCESS;
    },
    fd_fdstat_get(fd: number, statPtr: number) {
      if (fd > 2) return EBADF;
      bytes(statPtr, 24).fill(0);
      view().setUint8(statPtr, FILETYPE_REGULAR_FILE);
      view().setBigUint64(statPtr + 8, 0xffff_ffff_ffff_ffffn, true); // every right
      return SUCCESS;
    },
    fd_fdstat_set_flags: (fd: number) => (fd > 2 ? EBADF : SUCCESS),
    fd_close: () => SUCCESS,
    fd_prestat_get: () => EBADF, // no preopened directories
    fd_prestat_dir_name: () => EBADF,
    path_open: () => ENOENT,
    fd_seek: () => ESPIPE,
    fd_read(fd: number, iovs: number, count: number, readPtr: number) {
      if (fd !== 0) return EBADF;
      const start = read;
      for (const buffer of iovecs(iovs, count)) {
        const chunk = stdin.subarray(read, read + buffer.length);
        buffer.set(chunk);
        read += chunk.length;
        if (chunk.length < buffer.length) break;
      }
      view().setUint32(readPtr, read - start, true);
      return SUCCESS;
    },
    fd_write(fd: number, iovs: number, count: number, writtenPtr: number) {
      const sink = fd === 1 ? stdout : fd === 2 ? stderr : null;
      if (!sink) return EBADF;
      let written = 0;
      for (const buffer of iovecs(iovs, count)) {
        sink.push(buffer.slice());
        written += buffer.length;
      }
      view().setUint32(writtenPtr, written, true);
      return SUCCESS;
    },
    proc_exit(code: number) {
      throw new Exit(code);
    },
  };

  const instance = new WebAssembly.Instance(module, { wasi_snapshot_preview1: wasi });
  memory = instance.exports.memory as WebAssembly.Memory;
  let exitCode = 0;
  try {
    (instance.exports._start as () => void)();
  } catch (error) {
    if (!(error instanceof Exit)) throw error;
    exitCode = error.code;
  }
  return { exitCode, stdout: concat(stdout), stderr: new TextDecoder().decode(concat(stderr)) };
}

function concat(chunks: Uint8Array[]): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(chunks.reduce((sum, chunk) => sum + chunk.length, 0));
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.length;
  }
  return out;
}
