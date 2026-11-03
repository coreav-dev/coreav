#!/usr/bin/env bash
set -euo pipefail
DEST="${1:?usage: install-oracle-ffmpeg-macos.sh /absolute/bin/dir}"
HERE=$(dirname "${BASH_SOURCE[0]}")
source "$HERE/oracle-ffmpeg-macos.env"
[[ "$(uname -s)" == Darwin && "$(uname -m)" == arm64 ]] || {
  echo 'The macOS oracle supports macOS arm64 only.' >&2
  exit 1
}
mkdir -p "$DEST"
WORK=$(mktemp -d "$DEST/.download.XXXXXX")
REPLACING=0
COMMITTED=0
cleanup() {
  STATUS=$?
  RESTORED=1
  if [[ "$REPLACING" == 1 && "$COMMITTED" == 0 ]]; then
    for NAME in ffmpeg ffprobe; do
      if [[ -f "$WORK/old-$NAME" ]]; then
        mv -f "$WORK/old-$NAME" "$DEST/$NAME" || RESTORED=0
      else
        rm -f "$DEST/$NAME" || RESTORED=0
      fi
    done
  fi
  if [[ "$RESTORED" == 1 ]]; then
    rm -rf "$WORK"
  else
    echo "Could not restore the oracle pair; recovery files remain in $WORK" >&2
    STATUS=1
  fi
  exit "$STATUS"
}
trap cleanup EXIT
for NAME in ffmpeg ffprobe; do
  case "$NAME" in
    ffmpeg) DOWNLOAD_URL=$MACOS_FFMPEG_URL; HASH=$MACOS_FFMPEG_SHA256 ;;
    ffprobe) DOWNLOAD_URL=$MACOS_FFPROBE_URL; HASH=$MACOS_FFPROBE_SHA256 ;;
  esac
  curl --fail --location --retry 5 --retry-connrefused --retry-delay 10 \
    --retry-max-time 900 --connect-timeout 20 --max-time 900 \
    "$DOWNLOAD_URL" --output "$WORK/$NAME.zip"
  echo "$HASH  $WORK/$NAME.zip" | shasum -a 256 --check
  unzip -p "$WORK/$NAME.zip" "$NAME" > "$WORK/$NAME"
  chmod 755 "$WORK/$NAME"
done
python3 -I "$HERE/oracle_pin_macos.py" --verify-bin "$WORK"
for NAME in ffmpeg ffprobe; do
  if [[ -L "$DEST/$NAME" || ( -e "$DEST/$NAME" && ! -f "$DEST/$NAME" ) ]]; then
    echo "Refusing a nonregular oracle destination: $DEST/$NAME" >&2
    exit 1
  fi
  if [[ -f "$DEST/$NAME" ]]; then
    cp -p "$DEST/$NAME" "$WORK/old-$NAME"
  fi
done
REPLACING=1
install -m 755 "$WORK/ffmpeg" "$WORK/ffprobe" "$DEST/"
python3 -I "$HERE/oracle_pin_macos.py" --verify-bin "$DEST"
"$DEST/ffmpeg" -version
"$DEST/ffprobe" -version
COMMITTED=1
