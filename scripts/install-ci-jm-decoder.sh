#!/usr/bin/env bash
set -euo pipefail
DEST="${1:?usage: install-ci-jm-decoder.sh /absolute/bin/dir}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$here/jm-decoder.env"
mkdir -p "$DEST"
DEST=$(cd "$DEST" && pwd)
WORK=$(mktemp -d "${TMPDIR:-/tmp}/coreav-jm.XXXXXX")
CA_STAGE=
trap 'rm -rf "$WORK"; if [ -n "$CA_STAGE" ]; then rm -f "$CA_STAGE"; fi' EXIT
ROOTS="${SSL_CERT_FILE:-/etc/ssl/certs/ca-certificates.crt}"
CACHE="${COREAV_CI_ARCHIVE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/coreav-ci-archives}"
CA_CACHE="$CACHE/jm-ca-${JM_VERSION}-${JM_CA_SHA256}.archive"
if ! { cp "$CA_CACHE" "$WORK/intermediate.der" 2>/dev/null && \
  printf '%s  %s\n' "$JM_CA_SHA256" "$WORK/intermediate.der" | shasum -a 256 -c - >/dev/null 2>&1; }; then
  curl --fail --silent --show-error --location --retry 8 --retry-all-errors \
    --retry-connrefused --retry-max-time 360 --connect-timeout 20 --max-time 300 \
    --speed-limit 1024 --speed-time 30 "$JM_CA_URL" -o "$WORK/intermediate.pem"
  openssl x509 -in "$WORK/intermediate.pem" -outform DER -out "$WORK/intermediate.der"
  printf '%s  %s\n' "$JM_CA_SHA256" "$WORK/intermediate.der" | shasum -a 256 -c - >&2
fi
openssl x509 -inform DER -in "$WORK/intermediate.der" -out "$WORK/intermediate-pinned.pem"
openssl verify -CAfile "$ROOTS" "$WORK/intermediate-pinned.pem" > /dev/null
if mkdir -p "$CACHE" 2>/dev/null && CA_STAGE=$(mktemp "$CA_CACHE.XXXXXX" 2>/dev/null); then
  if cp "$WORK/intermediate.der" "$CA_STAGE" && mv -f "$CA_STAGE" "$CA_CACHE"; then
    CA_STAGE=
  else
    echo 'JM certificate cache publication unavailable; continuing with verified certificate' >&2
  fi
fi
cat "$ROOTS" "$WORK/intermediate-pinned.pem" > "$WORK/ca.pem"
bash "$here/fetch-ci-archive.sh" "$JM_URL" "$JM_SHA256" "$WORK/jm.zip" \
  "$CACHE/jm-${JM_VERSION}-${JM_SHA256}.archive" --cacert "$WORK/ca.pem"
unzip -q "$WORK/jm.zip" -d "$WORK/source"
BUILD=$(find "$WORK/source" -path '*/ldecod/Makefile' -print -quit)
test -n "$BUILD"
if ! make -C "${BUILD%/*}" -j2 CC='cc -fcommon' > "$WORK/build.log" 2>&1; then
  echo 'JM black-box decoder build failed' >&2
  python3 - "$WORK/build.log" <<'PY'
import pathlib
import sys
text = pathlib.Path(sys.argv[1]).read_text(errors='replace')
categories = {'No rule to make target': 'missing-build-target',
              'multiple definition': 'legacy-common-symbols',
              'undefined reference': 'unresolved-symbol',
              'unrecognized command-line option': 'unsupported-compiler-option',
              'No such file or directory': 'missing-build-input',
              ': error:': 'compilation-error'}
print('JM build diagnostic categories: ' + ', '.join(value for key, value in categories.items() if key in text))
PY
  exit 1
fi
BINARY=$(find "$WORK/source" -type f \( -name ldecod.exe -o -name ldecod \) -print -quit)
test -n "$BINARY" && test -x "$BINARY"
install -m 755 "$BINARY" "$DEST/ldecod"
{
  printf 'JM_VERSION=%s\nJM_SHA256=%s\n' "$JM_VERSION" "$JM_SHA256"
  printf "BUILD_COMMAND=make -C JM/ldecod -j2 CC='cc -fcommon'\n"
  cc --version | head -1
  sha256sum "$DEST/ldecod"
} > "$DEST/jm-build.txt"
