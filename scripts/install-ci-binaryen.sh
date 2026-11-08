#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/binaryen.env"
if [ "$#" -ne 1 ]; then
  echo "usage: install-ci-binaryen.sh DIRECTORY" >&2
  exit 2
fi
case "$(uname -sm)" in
  'Linux x86_64') platform=x86_64-linux; digest="$BINARYEN_SHA256_X86_64_LINUX" ;;
  'Darwin arm64') platform=arm64-macos; digest="$BINARYEN_SHA256_ARM64_MACOS" ;;
  *) echo "error: no pinned Binaryen archive for this platform" >&2; exit 1 ;;
esac
mkdir -p "$1"
destination="$(cd "$1" && pwd)"
temporary="$(mktemp -d "$destination/.binaryen.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT
cache="${COREAV_CI_ARCHIVE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/coreav-ci-archives}"
name="binaryen-version_${BINARYEN_VERSION}-${platform}.tar.gz"
bash "$here/fetch-ci-archive.sh" \
  "https://github.com/WebAssembly/binaryen/releases/download/version_${BINARYEN_VERSION}/$name" \
  "$digest" "$temporary/$name" "$cache/binaryen-${BINARYEN_VERSION}-${digest}.archive"
mkdir "$temporary/extracted"
tar xzf "$temporary/$name" -C "$temporary/extracted"
opt="binaryen-version_${BINARYEN_VERSION}/bin/wasm-opt"
"$temporary/extracted/$opt" --version >&2
rm -rf "$destination/binaryen-version_${BINARYEN_VERSION}"
mv "$temporary/extracted/binaryen-version_${BINARYEN_VERSION}" "$destination/"
printf '%s\n' "$destination/$opt"
