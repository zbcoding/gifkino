#!/usr/bin/env bash
# Rebuild gifsicle.wasm: upstream gifsicle compiled unmodified for WASI
# preview 1 with wasi-sdk. The binary is committed, so a site build does not
# need this; run it to update gifsicle or to reproduce the shipped file.
# gifsicle is GPL-2.0 (its COPYING); this script is its build recipe.
set -euo pipefail

gifsicle=1.98
gifsicle_sha256=e9d4cc0816db95c64d32e79f778836db0f1f7eb15868490bd330104e5146b66e
wasi_sdk=34
wasi_sdk_sha256=b761e3a0721dbae9c09a0059e5fdb2bf917d1b4a8a7b430fb3b5aafb0984b2c4

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
work=${GIFSICLE_BUILD_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/gifkino-gifsicle}
mkdir -p "$work"
cd "$work"

fetch() { # url sha256
  local file=${1##*/}
  [ -f "$file" ] || curl -fL --retry 3 -o "$file" "$1"
  echo "$2  $file" | sha256sum -c --quiet
  tar xzf "$file"
}

sdk=wasi-sdk-$wasi_sdk.0-x86_64-linux
fetch "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-$wasi_sdk/$sdk.tar.gz" "$wasi_sdk_sha256"
fetch "https://www.lcdf.org/gifsicle/gifsicle-$gifsicle.tar.gz" "$gifsicle_sha256"

cd "gifsicle-$gifsicle"
# WASI has no processes or temp files: --change-color-command's popen and
# tmpnam are stubbed to fail, which gifsicle already reports as an error.
printf '#include <stdio.h>\n#define popen(c, m) NULL\n#define pclose(f) (-1)\n#define tmpnam(s) NULL\n' >wasi-no-popen.h
./configure --host=wasm32-wasi --disable-gifview --disable-gifdiff --disable-threads \
  CC="$work/$sdk/bin/clang --sysroot=$work/$sdk/share/wasi-sysroot" \
  CFLAGS="-O2 -include $PWD/wasi-no-popen.h" \
  LDFLAGS="-Wl,--strip-all" \
  AR="$work/$sdk/bin/llvm-ar" RANLIB="$work/$sdk/bin/llvm-ranlib"
make clean
make -C src gifsicle
cp src/gifsicle "$here/gifsicle.wasm"
ls -l "$here/gifsicle.wasm"
