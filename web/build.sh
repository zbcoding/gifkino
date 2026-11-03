#!/usr/bin/env bash
# Assemble the canister's site in web/dist: the landing page from public/ at /,
# the browser editor's build (which builds its wasm module first) at /edit/.
set -euo pipefail

web=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

(cd "$web/editor" && vp install --frozen-lockfile && vp run build)

rm -rf "$web/dist"
cp -r "$web/public" "$web/dist"
cp -r "$web/editor/dist" "$web/dist/edit"
