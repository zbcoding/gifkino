import { defineConfig } from "vite-plus";

export default defineConfig({
  // Served by the landing page's canister under /edit/ (see web/build.sh).
  base: "/edit/",
  // The wasm glue is an ES module that finds its .wasm via import.meta.url.
  worker: { format: "es" },
  // Nothing inlined as data: URLs, which the site's CSP (public/_headers) refuses.
  build: { target: "es2024", assetsInlineLimit: 0 },
  // The welcome page and favicon use the landing page's icon from ../public;
  // the canvas's rotate cursor is the desktop's, from the repo's resources/.
  server: { fs: { allow: ["../.."] } },
  fmt: {},
  lint: {
    jsPlugins: [{ name: "vite-plus", specifier: "vite-plus/oxlint-plugin" }],
    rules: { "vite-plus/prefer-vite-plus-imports": "error" },
    ignorePatterns: ["src/wasm/**", "dist/**"],
    options: { typeAware: true, typeCheck: true },
  },
});
