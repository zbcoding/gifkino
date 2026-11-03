import { describe, expect, it } from "vite-plus/test";
import fixtureUrl from "../../../../tests/fixtures/test-8-frames.gif?inline";
import { optimize } from "./optimize";
import gifsicleUrl from "./gifsicle.wasm?url&inline";
import { runCommand } from "./wasi";

// `?inline` hands Node the bytes as data: URLs, which its fetch reads.
const bytes = async (url: string) => new Uint8Array(await (await fetch(url)).arrayBuffer());
const gifsicle = new WebAssembly.Module(await bytes(gifsicleUrl));
const fixture = await bytes(fixtureUrl);

function info(gif: Uint8Array): string {
  return new TextDecoder().decode(runCommand(gifsicle, ["gifsicle", "--info"], gif).stdout);
}

describe("gifsicle", () => {
  // The export's optimization pass must shrink the file without dropping a
  // frame, or the download would be a different animation than the document.
  it("optimizes a GIF to one no larger with every frame kept", () => {
    const out = optimize(gifsicle, fixture, 0);
    expect(out.length).toBeGreaterThan(0);
    expect(out.length).toBeLessThanOrEqual(fixture.length);
    expect(info(out)).toMatch(/ 8 images/);
  });

  // The lossy setting has to reach gifsicle, or it would be a dead control.
  it("passes the lossy setting on", () => {
    expect(optimize(gifsicle, fixture, 200)).not.toEqual(optimize(gifsicle, fixture, 0));
  });

  // A failure must throw so the export can fall back to the unoptimized file
  // rather than download gifsicle's empty output.
  it("throws on input it cannot read", () => {
    expect(() => optimize(gifsicle, new TextEncoder().encode("not a GIF"), 0)).toThrow(
      /gifsicle exited with 1/,
    );
  });
});
