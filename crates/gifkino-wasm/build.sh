#!/usr/bin/env bash
# Build the browser editor's WebAssembly module and its JavaScript glue into
# web/editor/src/wasm/. Needs the wasm32-unknown-unknown target and a
# wasm-bindgen CLI of exactly the version Cargo.toml pins the crate to.
set -euo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
target_dir=$(cargo metadata --format-version 1 --no-deps --manifest-path "$repo/Cargo.toml" |
  sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')

cargo build --profile wasm-release --target wasm32-unknown-unknown -p gifkino-wasm \
  --manifest-path "$repo/Cargo.toml"
wasm-bindgen --target web --out-dir "$repo/web/editor/src/wasm" \
  "$target_dir/wasm32-unknown-unknown/wasm-release/gifkino_wasm.wasm"
