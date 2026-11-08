#!/usr/bin/env bash
set -euo pipefail
if [[ $# -ne 2 ]]; then
  echo 'usage: install-ci-check-tool.sh actionlint|cargo-deny DIRECTORY' >&2
  exit 2
fi
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
source "$HERE/ci-tools.env"
TOOL=$1
DEST=$2
if [[ "$(uname -sm)" != 'Linux x86_64' ]]; then
  echo 'reason=ci_check_tool_unsupported_platform' >&2
  exit 1
fi
case "$TOOL" in
  actionlint)
    VERSION=$ACTIONLINT_VERSION
    DIGEST=$ACTIONLINT_SHA256
    URL="https://github.com/rhysd/actionlint/releases/download/v$VERSION/actionlint_${VERSION}_linux_amd64.tar.gz"
    MEMBER=actionlint
    ;;
  cargo-deny)
    VERSION=$CARGO_DENY_VERSION
    DIGEST=$CARGO_DENY_SHA256
    NAME="cargo-deny-$VERSION-x86_64-unknown-linux-musl"
    URL="https://github.com/EmbarkStudios/cargo-deny/releases/download/$VERSION/$NAME.tar.gz"
    MEMBER="$NAME/cargo-deny"
    ;;
  *) echo 'reason=ci_check_tool_unknown' >&2; exit 2 ;;
esac
WORK=$(mktemp -d "${TMPDIR:-/tmp}/coreav-ci-check-tool.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
CACHE_DIR="${COREAV_CI_ARCHIVE_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/coreav-ci-archives}"
bash "$HERE/fetch-ci-archive.sh" "$URL" "$DIGEST" "$WORK/archive" \
  "$CACHE_DIR/$TOOL-$VERSION-$DIGEST.archive"
tar -xzf "$WORK/archive" -C "$WORK" "$MEMBER"
mkdir -p "$DEST"
install -m 755 "$WORK/$MEMBER" "$DEST/$TOOL"
"$DEST/$TOOL" --version >&2
