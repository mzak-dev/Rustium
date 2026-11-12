#!/usr/bin/env bash
# Builds the browser version of Rustium into a folder that any static web server (GitHub Pages) can serve.
#
#   scripts/build-web.sh [--dev] [OUT_DIR]
#
# OUT_DIR defaults to target/web. --dev builds without optimizations (faster, much larger). Needs the
# wasm32-unknown-unknown target (rust-toolchain.toml installs it) and wasm-pack (`cargo install wasm-pack`), which
# brings wasm-bindgen and wasm-opt along. Serve the folder over HTTP to try it, e.g. `python3 -m http.server -d
# target/web`; opening index.html as a file does not work.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
profile=--release
if [ "${1:-}" = "--dev" ]; then
  profile=--dev
  shift
fi
out="$(realpath -m "${1:-$root/target/web}")"

# wasm-pack runs the wasm-opt it finds on PATH before downloading its own (binaryen version_130). Old ones, such as
# binaryen 108 from Ubuntu's archive, either reject the module or produce one that fails to start in the browser.
if [ "$profile" = --release ] && command -v wasm-opt > /dev/null; then
  wasm_opt_version="$(wasm-opt --version | grep -o 'version [0-9]*' | grep -o '[0-9]*' || true)"
  if [ "${wasm_opt_version:-0}" -lt 120 ]; then
    echo "error: $(command -v wasm-opt) is binaryen ${wasm_opt_version:-?}; remove it from PATH or install 120 or newer" >&2
    exit 1
  fi
fi

rm -rf "$out"
mkdir -p "$out"
wasm-pack build "$root/crates/rustium-web" "$profile" --target web --no-pack \
  --out-dir "$out/pkg" --out-name rustium_web
rm -f "$out/pkg/.gitignore"
cp "$root/web/index.html" "$out/index.html"
cp "$root/crates/rustium-app/assets/logo.png" "$out/logo.png"
# GitHub Pages runs sites through Jekyll unless told not to.
touch "$out/.nojekyll"

wasm="$out/pkg/rustium_web_bg.wasm"
echo "Built $out: $(du -h "$wasm" | cut -f1) wasm, $(gzip -9 -c "$wasm" | wc -c | numfmt --to=iec) gzipped"
