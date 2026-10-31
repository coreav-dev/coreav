#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LEVEL="${1:?usage: oracle-ffprobe.sh LEVEL MEDIA_FILE}"
MEDIA_FILE="${2:?usage: oracle-ffprobe.sh LEVEL MEDIA_FILE}"
FFPROBE="${FFPROBE:-ffprobe}"
FFMPEG="${FFMPEG:-ffmpeg}"
COREAV_ORACLE="${COREAV_ORACLE:-$ROOT/target/release/coreav-oracle}"
FILE_NAME="${MEDIA_FILE##*/}"

if [[ ! -x "$COREAV_ORACLE" ]]; then
  echo "oracle-ffprobe.sh: COREAV_ORACLE is not executable: $COREAV_ORACLE" >&2
  exit 2
fi

if [[ "$#" -ne 2 ]]; then
  echo "usage: oracle-ffprobe.sh LEVEL MEDIA_FILE" >&2
  exit 2
fi

# The pinned static build converts DVB text (the SDT's service names in a
# transport stream) with iconv, which loads the host's gconv modules; on a
# host whose glibc differs from the build's (Ubuntu 24.04) that crashes.
# With no modules to load, iconv fails cleanly and the names are left as
# read. No compared field depends on them (docs/design/mpegts.md).
export GCONV_PATH="${GCONV_PATH:-/nonexistent/gconv}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT
RAW="$TMP_DIR/raw"

case "$LEVEL" in
  streams)
    set +e
    "$FFPROBE" -v error -of json -show_format -show_streams "$MEDIA_FILE" > "$RAW"
    EXIT_CODE=$?
    set -e
    "$COREAV_ORACLE" normalize streams --file "$FILE_NAME" --exit-code "$EXIT_CODE" --input "$RAW"
    ;;
  packets)
    set +e
    "$FFPROBE" -v error -of json -show_streams -show_packets "$MEDIA_FILE" > "$RAW"
    EXIT_CODE=$?
    set -e
    "$COREAV_ORACLE" normalize packets --file "$FILE_NAME" --exit-code "$EXIT_CODE" --input "$RAW"
    ;;
  frames)
    META="$TMP_DIR/streams.csv"
    set +e
    "$FFPROBE" -v error -show_entries stream=index,codec_type -of csv=p=0 "$MEDIA_FILE" > "$META"
    PROBE_EXIT_CODE=$?
    set -e
    if [[ "$PROBE_EXIT_CODE" -ne 0 ]]; then
      : > "$RAW"
      "$COREAV_ORACLE" normalize frames --file "$FILE_NAME" --exit-code "$PROBE_EXIT_CODE" --stream-map "" --input "$RAW"
      exit 0
    fi
    STREAM_MAP=""
    while IFS=, read -r input_index codec_type _; do
      if [[ "$input_index" =~ ^[0-9]+$ ]] && [[ "$codec_type" == "video" || "$codec_type" == "audio" ]]; then
        if [[ -n "$STREAM_MAP" ]]; then
          STREAM_MAP+=","
        fi
        STREAM_MAP+="$input_index"
      fi
    done < "$META"
    if [[ -z "$STREAM_MAP" ]]; then
      : > "$RAW"
      "$COREAV_ORACLE" normalize frames --file "$FILE_NAME" --exit-code 0 --stream-map "" --input "$RAW"
      exit 0
    fi
    FFMPEG_ARGS=()
    IFS=, read -r -a INPUT_INDICES <<< "$STREAM_MAP"
    for input_index in "${INPUT_INDICES[@]}"; do
      FFMPEG_ARGS+=(-map "0:$input_index")
    done
    set +e
    mapfile -t AUDIO_DECODE_INPUT < "$ROOT/scripts/oracle-audio-args.txt"
    "$FFMPEG" -nostdin -v error "${AUDIO_DECODE_INPUT[@]}" -i "$MEDIA_FILE" "${FFMPEG_ARGS[@]}" -fps_mode passthrough -f framemd5 - > "$RAW"
    EXIT_CODE=$?
    set -e
    "$COREAV_ORACLE" normalize frames --file "$FILE_NAME" --exit-code "$EXIT_CODE" --stream-map "$STREAM_MAP" --input "$RAW"
    ;;
  *)
    echo "oracle-ffprobe.sh: unsupported level: $LEVEL" >&2
    exit 2
    ;;
esac
