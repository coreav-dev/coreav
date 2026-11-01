#!/usr/bin/env bash
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
. "$here/binaryen.env"
if [ "$#" -ne 1 ]; then
  echo "usage: install-binaryen.sh DIRECTORY" >&2
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
name="binaryen-version_${BINARYEN_VERSION}-${platform}.tar.gz"
curl --fail --silent --show-error --location --retry 2 \
  "https://github.com/WebAssembly/binaryen/releases/download/version_${BINARYEN_VERSION}/$name" -o "$temporary/$name"
printf '%s  %s\n' "$digest" "$temporary/$name" | shasum -a 256 -c - >&2
tar xzf "$temporary/$name" -C "$destination"
opt="$destination/binaryen-version_${BINARYEN_VERSION}/bin/wasm-opt"
"$opt" --version >&2
printf '%s\n' "$opt"
