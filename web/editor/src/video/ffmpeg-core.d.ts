// @ffmpeg/core ships no types: the parts of its Emscripten module used here.

declare module "@ffmpeg/core" {
  export interface Core {
    FS: {
      mkdir(path: string): void;
      mount(type: unknown, options: { files: File[] }, path: string): void;
      unmount(path: string): void;
      makedev(major: number, minor: number): number;
      registerDevice(dev: number, ops: object): void;
      mkdev(path: string, dev: number): void;
      filesystems: { WORKERFS: unknown };
    };
    /** The last run's exit code. */
    ret: number;
    exec(...args: string[]): number;
    ffprobe(...args: string[]): number;
    reset(): void;
    setLogger(logger: (log: { type: string; message: string }) => void): void;
  }

  export default function createFFmpegCore(options: object): Promise<Core>;
}
