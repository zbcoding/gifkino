import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { defineConfig, type Plugin } from "vite-plus";

// ffmpeg.wasm's core is fetched from a CDN at run time (src/video/ffmpeg.ts),
// pinned to the installed package: its version picks the file, its SHA-256 is
// what the download must hash to.
const ffmpegCore = new URL("node_modules/@ffmpeg/core/", import.meta.url);
const ffmpegCoreVersion: string = JSON.parse(
  readFileSync(new URL("package.json", ffmpegCore), "utf8"),
).version;
const ffmpegCoreSha256 = createHash("sha256")
  .update(readFileSync(new URL("dist/esm/ffmpeg-core.wasm", ffmpegCore)))
  .digest("hex");

/**
 * The core's Emscripten glue names its wasm with `new URL("ffmpeg-core.wasm",
 * import.meta.url)`, which makes the build emit the 30 MB file as an asset. It
 * is never fetched from there (the worker's `instantiateWasm` is handed the
 * module compiled from the CDN), so the reference goes. Throws if the glue no
 * longer has it, rather than silently shipping the file again.
 */
function ffmpegCoreWithoutWasm(): Plugin {
  const reference = 'new URL("ffmpeg-core.wasm",import.meta.url).href';
  return {
    name: "ffmpeg-core-without-wasm",
    enforce: "pre",
    transform(code, id) {
      if (!id.includes("/@ffmpeg/core/dist/esm/ffmpeg-core.js")) return;
      if (!code.includes(reference)) {
        throw new Error(`${id} no longer names its wasm as ${reference}; update this plugin`);
      }
      return code.replace(reference, '"ffmpeg-core.wasm"');
    },
  };
}

export default defineConfig({
  // Served by the landing page's canister under /edit/ (see web/build.sh).
  base: "/edit/",
  // The wasm glue is an ES module that finds its .wasm via import.meta.url.
  worker: { format: "es", plugins: () => [ffmpegCoreWithoutWasm()] },
  // Nothing inlined as data: URLs, which the site's CSP (public/_headers) refuses.
  build: { target: "es2024", assetsInlineLimit: 0 },
  // The welcome page and favicon use the landing page's icon from ../public;
  // the canvas's rotate cursor is the desktop's, from the repo's resources/.
  // About reads the version from the workspace Cargo.toml.
  server: { fs: { allow: ["../.."] } },
  // ffmpeg.wasm's core JS is served as the file it ships as, not pre-bundled.
  optimizeDeps: { exclude: ["@ffmpeg/core"] },
  define: {
    __FFMPEG_CORE_VERSION__: JSON.stringify(ffmpegCoreVersion),
    __FFMPEG_CORE_SHA256__: JSON.stringify(ffmpegCoreSha256),
  },
  fmt: {},
  lint: {
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
    rules: { "vite-plus/prefer-vite-plus-imports": "error" },
    ignorePatterns: ["src/wasm/**", "dist/**"],
    options: { typeAware: true, typeCheck: true },
  },
});
