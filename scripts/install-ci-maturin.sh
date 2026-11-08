#!/usr/bin/env bash
# MIT License
#
# Copyright (c) 2021-present PyO3 Project and Contributors
#
# Permission is hereby granted, free of charge, to any person obtaining a copy
# of this software and associated documentation files (the "Software"), to deal
# in the Software without restriction, including without limitation the rights
# to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
# copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in all
# copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
# AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
# OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
# SOFTWARE.
#
# Upstream: https://github.com/PyO3/maturin-action
# Revision: e83996d129638aa358a18fbd1dfb82f0b0fb5d3b
# Ported: image/target mappings, Rust/uv and Python setup, patchelf/cffi,
# target checks, environment prefixes and macOS SDK defaults.
# New: pinned wheel acquisition, verified extraction, mode control,
# verified-executable transport and regression coverage.
# CI-only build glue; not an input to shipped packages or runtime notices.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
source "$root/scripts/maturin.env"
mode=${1:?expected metadata, install, build or verify-artifacts}
platform=${2:?expected linux-x86_64, linux-arm64 or macos-arm64}
case "$platform" in
  linux-x86_64|linux-arm64)
    url=$MATURIN_LINUX_URL
    sha256=$MATURIN_LINUX_SHA256
    ;;
  macos-arm64)
    url=$MATURIN_MACOS_URL
    sha256=$MATURIN_MACOS_SHA256
    ;;
  *) echo "unsupported maturin platform: $platform" >&2; exit 1 ;;
esac

if [[ "$mode" == metadata ]]; then
  printf 'tool=maturin-wheel\nversion=%s\nsha256=%s\n' "$MATURIN_VERSION" "$sha256"
  exit 0
fi
if [[ "$mode" == verify-artifacts ]]; then
  python3 -I - <<'PY'
from pathlib import Path
from zipfile import ZipFile

wheels = list(Path('dist').glob('*.whl'))
if not wheels:
    raise SystemExit('reason=ci_maturin_missing_wheels')
for wheel in wheels:
    with ZipFile(wheel) as archive:
        if any(Path(name).name == 'install-ci-maturin.sh' for name in archive.namelist()):
            raise SystemExit('reason=ci_build_glue_in_wheel')
PY
  exit 0
fi
if [[ "$mode" == build ]]; then
  shift 2
  temporary=$(mktemp -d "${RUNNER_TEMP:?}/coreav-maturin-build.XXXXXX")
  trap 'rm -rf "$temporary"' EXIT
  "$root/scripts/install-ci-maturin.sh" install "$platform" "$temporary"
  toolchain=$(python3 -I -c 'import sys, tomllib; print(tomllib.load(open(sys.argv[1], "rb"))["toolchain"]["channel"])' "$root/rust-toolchain.toml")
  export CARGO_INCREMENTAL=${CARGO_INCREMENTAL-0}
  export CARGO_TERM_COLOR=${CARGO_TERM_COLOR-always}
  cd "${GITHUB_WORKSPACE:?}"
  if [[ "$platform" == macos-arm64 ]]; then
    rustup update --no-self-update "$toolchain"
    rustup override set "$toolchain"
    rustup component add llvm-tools-preview || true
    rustup target add --toolchain "$toolchain" aarch64-apple-darwin
    python3 -m pip install cffi || true
    export DEVELOPER_DIR=${DEVELOPER_DIR:-/Applications/Xcode.app/Contents/Developer}
    export SDKROOT=${SDKROOT:-/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk}
    "$temporary/maturin" build --target aarch64-apple-darwin "$@"
    exit 0
  fi
  case "$platform" in
    linux-x86_64)
      image=quay.io/pypa/manylinux2014_x86_64:latest
      target=x86_64-unknown-linux-gnu
      manylinux=auto
      ;;
    linux-arm64)
      image=ghcr.io/rust-cross/manylinux_2_28-cross:aarch64
      target=aarch64-unknown-linux-gnu
      manylinux=2_28
      ;;
  esac
  docker_env=(-e DEBIAN_FRONTEND=noninteractive -e ARCHFLAGS -e _PYTHON_SYSCONFIGDATA_NAME)
  while IFS= read -r name; do
    case "$name" in
      CARGO_HOME) ;;
      ACTIONS_*|CARGO_*|CC*|CFLAGS*|CMAKE_*|CPPFLAGS*|CXX*|CXXFLAGS*|GITHUB_*|JEMALLOC_*|LDFLAGS*|LD_*|MATURIN_*|PYO3_*|RUST*|SCCACHE_*|TARGET_*)
        docker_env+=(-e "$name") ;;
    esac
  done < <(compgen -e)
  docker run --rm --interactive --workdir "$GITHUB_WORKSPACE" "${docker_env[@]}" \
    -v "$GITHUB_WORKSPACE:$GITHUB_WORKSPACE" \
    -v "$temporary:/opt/coreav-maturin:ro" \
    "$image" bash -s -- "$toolchain" "$target" "$manylinux" "$@" <<'SH'
set -euo pipefail
toolchain=$1
target=$2
manylinux=$3
shift 3
if command -v rustup >/dev/null; then
  rm -rf "$HOME/.rustup/toolchains/"
  rustup toolchain install stable
else
  curl --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
rustup update --no-self-update "$toolchain"
rustup override set "$toolchain"
rustup component add llvm-tools-preview || true
export PATH="$PATH:/opt/python/cp37-cp37m/bin:/opt/python/cp38-cp38/bin:/opt/python/cp39-cp39/bin:/opt/python/cp310-cp310/bin:/opt/python/cp311-cp311/bin:/opt/python/cp312-cp312/bin"
command -v uv >/dev/null || curl -LsSf https://astral.sh/uv/install.sh | sh
command -v patchelf >/dev/null || uv tool install patchelf
python3 -m pip install --user cffi || true
if [[ ! -d $(rustc --print target-libdir --target "$target") ]]; then
  rustup target add "$target"
fi
if [[ "$manylinux" != auto ]]; then
  set -- --manylinux "$manylinux" "$@"
fi
/opt/coreav-maturin/maturin build --target "$target" "$@"
SH
  sudo chown -R "$(id -u):$(id -g)" "$root/target" "$root/dist" || true
  exit 0
fi
if [[ "$mode" != install ]]; then
  echo "expected metadata, install, build or verify-artifacts" >&2
  exit 1
fi

destination=${3:?expected installation directory}
cache_dir=${COREAV_CI_ARCHIVE_DIR:-${RUNNER_TEMP:?}/coreav-ci-archives}
mkdir -p "$destination"
temporary=$(mktemp -d "${RUNNER_TEMP:?}/coreav-maturin.XXXXXX")
trap 'rm -rf "$temporary"' EXIT
"$root/scripts/fetch-ci-archive.sh" "$url" "$sha256" "$temporary/maturin.whl" \
  "$cache_dir/maturin-wheel-$MATURIN_VERSION-$sha256.archive"
python3 -I - "$temporary/maturin.whl" "$temporary/maturin" "$MATURIN_VERSION" <<'PY'
import pathlib
import sys
import zipfile

wheel, output, version = sys.argv[1:]
with zipfile.ZipFile(wheel) as archive:
    pathlib.Path(output).write_bytes(archive.read(f"maturin-{version}.data/scripts/maturin"))
PY
chmod 755 "$temporary/maturin"
mv -f "$temporary/maturin" "$destination/maturin"
