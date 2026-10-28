#!/usr/bin/env bash
# Installs the packed packages into new projects and runs `coreav` through
# npx (CI, .github/workflows/npm.yml):
#   test/smoke.sh PLATFORM PACKAGES EXPECTED
# PACKAGES holds the main package's tarball and PLATFORM's; EXPECTED the
# native command's outputs for basic.mp4 (probe.json, check.json, clip.mp4,
# clip.json) and web-upload.json. Two projects: with PLATFORM's package
# (the native binary; on Linux and macOS the installed command must be the
# binary itself), and without optional packages (the WASI build).
set -euo pipefail
platform="$1"
packages="$2"
expected="$3"
main="$(ls "$packages"/coreav-[0-9]*.tgz)"
native="$(ls "$packages"/coreav-cli-"$platform"-*.tgz)"

checks() {
  npx --no-install coreav --version
  mkdir -p media && cp "$expected/basic.mp4" media/
  # A relative path, with the platform's separator on Windows.
  if [ "$RUNNER_OS" = Windows ]; then rel='media\basic.mp4'; else rel=media/basic.mp4; fi
  npx --no-install coreav probe "$rel" > probe.json
  cmp probe.json "$expected/probe.json"
  # An absolute path, in the platform's own form.
  if [ "$RUNNER_OS" = Windows ]; then abs="$(cygpath -w "$PWD/media/basic.mp4")"; else abs="$PWD/media/basic.mp4"; fi
  (cd media && npx --no-install coreav clip --start 0 --end 1 "$abs" ../clip.mp4 > ../clip.json)
  cmp clip.mp4 "$expected/clip.mp4"
  cmp clip.json "$expected/clip.json"
  # An absolute path as the value of --name=value.
  if [ "$RUNNER_OS" = Windows ]; then policy="$(cygpath -w "$expected/web-upload.json")"; else policy="$expected/web-upload.json"; fi
  npx --no-install coreav check "--policy=$policy" "$rel" > check.json
  cmp check.json "$expected/check.json"
  # A missing file: exit 3 and a coreav.error/1 object on stderr only.
  set +e
  npx --no-install coreav probe missing.mp4 2> error.json
  code=$?
  set -e
  [ "$code" -eq 3 ]
  grep -q '"schema":"coreav.error/1"' error.json
  [ "$(wc -l < error.json)" -eq 1 ]
}

project() {
  rm -rf "$1" && mkdir "$1" && cd "$1"
  echo '{"name": "smoke", "private": true}' > package.json
}

echo "== native: $platform"
project "$RUNNER_TEMP/smoke-native"
npm install --no-audit --no-fund "$native" "$main"
if [ "$RUNNER_OS" != Windows ]; then
  cmp node_modules/coreav/bin/coreav "node_modules/@coreav/cli-$platform/bin/coreav"
fi
checks

echo "== fallback: the WASI build"
project "$RUNNER_TEMP/smoke-wasi"
npm install --no-audit --no-fund --omit=optional "$main"
[ ! -e "node_modules/@coreav/cli-$platform" ]
head -c 2 node_modules/coreav/bin/coreav | grep -q '#!'
checks
echo "smoke: ok"
