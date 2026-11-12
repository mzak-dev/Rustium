#!/usr/bin/env bash
# Packages a release build of the `rustium` binary for one target.
#
#   scripts/package.sh <target> <version> <out-dir>
#
# Expects `cargo build --release --locked -p rustium-app --target <target>` to have run. Writes
# `<out-dir>/rustium-v<version>-<target>.zip` on Windows targets and `.tar.gz` elsewhere, each with the binary,
# LICENSE, README.md and CHANGELOG.md in a `rustium-v<version>-<target>/` folder, and prints the archive path.
# Fonts, icons and locales are compiled into the binary, so nothing else has to ship next to it.
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <target> <version> <out-dir>" >&2
  exit 2
fi
target=$1
version=$2
mkdir -p "$3"
out_dir=$(cd "$3" && pwd)

executable=rustium
[[ $target == *windows* ]] && executable=rustium.exe
binary="target/$target/release/$executable"
if [[ ! -f $binary ]]; then
  echo "error: $binary does not exist; build it first" >&2
  exit 1
fi

name="rustium-v$version-$target"
staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
mkdir "$staging/$name"
cp "$binary" LICENSE README.md "$staging/$name/"
[[ -f CHANGELOG.md ]] && cp CHANGELOG.md "$staging/$name/"

if [[ $target == *windows* ]]; then
  archive="$out_dir/$name.zip"
  (cd "$staging" && 7z a -tzip -bso0 -bsp0 "$archive" "$name")
else
  archive="$out_dir/$name.tar.gz"
  tar -C "$staging" -czf "$archive" "$name"
fi
echo "$archive"
