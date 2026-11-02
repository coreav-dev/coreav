#!/usr/bin/env bash
set -euo pipefail
DEST="${1:?usage: install-jm-decoder.sh /absolute/bin/dir}"
source "$(dirname "${BASH_SOURCE[0]}")/jm-decoder.env"
mkdir -p "$DEST"
DEST=$(cd "$DEST" && pwd)
WORK=$(mktemp -d "${TMPDIR:-/tmp}/coreav-jm.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
ROOTS="${SSL_CERT_FILE:-/etc/ssl/certs/ca-certificates.crt}"
curl --fail --location --retry 3 --max-time 120 "$JM_CA_URL" -o "$WORK/intermediate.pem"
openssl x509 -in "$WORK/intermediate.pem" -outform DER -out "$WORK/intermediate.der"
printf '%s  %s\n' "$JM_CA_SHA256" "$WORK/intermediate.der" | sha256sum --check --status
openssl x509 -inform DER -in "$WORK/intermediate.der" -out "$WORK/intermediate-pinned.pem"
openssl verify -CAfile "$ROOTS" "$WORK/intermediate-pinned.pem" > /dev/null
cat "$ROOTS" "$WORK/intermediate-pinned.pem" > "$WORK/ca.pem"
curl --fail --location --retry 3 --max-time 120 --cacert "$WORK/ca.pem" "$JM_URL" -o "$WORK/jm.zip"
printf '%s  %s\n' "$JM_SHA256" "$WORK/jm.zip" | sha256sum --check --status
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
