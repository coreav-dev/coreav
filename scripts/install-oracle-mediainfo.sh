#!/usr/bin/env bash
# Install the pinned MediaInfo command-line binary, a test-only
# black box. Only the official self-contained CLI build is fetched; no
# libraries and never any source archive.
set -euo pipefail
DEST="${1:?usage: install-oracle-mediainfo.sh /absolute/bin/dir}"
VERSION=26.10
ARCHIVE="MediaInfo_CLI_${VERSION}_Lambda_x86_64.zip"
SHA256=3087b8d4906a47d876a66a500b8a6af63de8e9da9992d295a702b4ca79fb3fdd
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || {
  echo 'The pinned MediaInfo binary supports Linux x86_64 only.' >&2
  exit 1
}
command -v unzip > /dev/null || {
  echo 'install-oracle-mediainfo.sh needs unzip.' >&2
  exit 1
}
mkdir -p "$DEST"
WORK=$(mktemp -d "$DEST/.download.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
curl --fail --location --retry 3 \
  "https://mediaarea.net/download/binary/mediainfo/$VERSION/$ARCHIVE" \
  --output "$WORK/mediainfo.zip"
echo "$SHA256  $WORK/mediainfo.zip" | sha256sum --check --strict
unzip -q "$WORK/mediainfo.zip" bin/mediainfo LICENSE -d "$WORK/x"
install -m 755 "$WORK/x/bin/mediainfo" "$DEST/mediainfo"
install -m 644 "$WORK/x/LICENSE" "$DEST/mediainfo-LICENSE"
REPORTED="$("$DEST/mediainfo" --Version)"
echo "$REPORTED"
grep -qx "MediaInfoLib - v$VERSION" <<< "$REPORTED" || {
  echo "install-oracle-mediainfo.sh: expected MediaInfoLib v$VERSION" >&2
  exit 1
}
