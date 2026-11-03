import { runCommand } from "./wasi";

/**
 * Desktop `pipeline::gif::optimize`: `gifsicle -O3`, plus `--lossy` when the
 * setting is on, over an exported GIF. Throws when gifsicle fails.
 */
export function optimize(gifsicle: WebAssembly.Module, gif: Uint8Array, lossy: number) {
  const args = ["gifsicle", "-O3"];
  if (lossy > 0) args.push(`--lossy=${lossy}`);
  const { exitCode, stdout, stderr } = runCommand(gifsicle, args, gif);
  if (exitCode !== 0) throw new Error(`gifsicle exited with ${exitCode}: ${stderr.trim()}`);
  return stdout;
}
