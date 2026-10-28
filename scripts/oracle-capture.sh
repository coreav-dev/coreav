#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MEDIA_DIR=""
OUT_DIR=""
ADAPTER="$ROOT/scripts/oracle-ffprobe.sh"
LEVELS="streams,packets,frames"

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --media)
      MEDIA_DIR="${2:?missing value for --media}"
      shift 2
      ;;
    --out)
      OUT_DIR="${2:?missing value for --out}"
      shift 2
      ;;
    --adapter)
      ADAPTER="${2:?missing value for --adapter}"
      shift 2
      ;;
    --levels)
      LEVELS="${2:?missing value for --levels}"
      shift 2
      ;;
    *)
      echo "usage: oracle-capture.sh --media DIR --out DIR [--adapter EXE] [--levels streams,packets,frames]" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$MEDIA_DIR" || -z "$OUT_DIR" || ! -d "$MEDIA_DIR" ]]; then
  echo "oracle-capture.sh: --media and --out are required; --media must be a directory" >&2
  exit 2
fi
if [[ ! -x "$ADAPTER" ]]; then
  echo "oracle-capture.sh: adapter is not executable: $ADAPTER" >&2
  exit 2
fi

IFS=, read -r -a LEVEL_ARRAY <<< "$LEVELS"
for level in "${LEVEL_ARRAY[@]}"; do
  case "$level" in
    streams|packets|frames) ;;
    *)
      echo "oracle-capture.sh: unsupported level: $level" >&2
      exit 2
      ;;
  esac
done
if [[ "${#LEVEL_ARRAY[@]}" -eq 0 ]]; then
  echo "oracle-capture.sh: --levels must list one or more levels" >&2
  exit 2
fi

mkdir -p "$OUT_DIR"
TIMEOUT="${COREAV_ORACLE_TIMEOUT:-120}"

validate_name() {
  local name="$1"
  case "$name" in
    ''|/*|*$'\n'*|*$'\r'*)
      echo 'oracle-capture.sh: invalid manifest/input name: must be a nonempty relative path without newlines' >&2
      exit 2
      ;;
  esac
  case "/$name/" in
    */../*)
      echo "oracle-capture.sh: invalid manifest/input name: '..' path components are forbidden" >&2
      exit 2
      ;;
  esac
}

capture_file() {
  local name="$1"
  validate_name "$name"
  local media_file="$MEDIA_DIR/$name"
  for level in "${LEVEL_ARRAY[@]}"; do
    local output="$OUT_DIR/$name.$level.json"
    mkdir -p "$(dirname "$output")"
    set +e
    timeout "$TIMEOUT" "$ADAPTER" "$level" "$media_file" > "$output"
    local exit_code=$?
    set -e
    if [[ "$exit_code" -ne 0 || ! -s "$output" ]]; then
      local escaped_name="$name"
      escaped_name="${escaped_name//\\/\\\\}"
      escaped_name="${escaped_name//\"/\\\"}"
      local status_field="probe_ok"
      [[ "$level" == "frames" ]] && status_field="decode_ok"
      printf '{"schema":"coreav-oracle/%s/1","file":"%s","%s":false}\n' \
        "$level" "$escaped_name" "$status_field" > "$output"
      echo "oracle-capture.sh: $level failed for $name (adapter status $exit_code)" >&2
    fi
  done
}

if [[ -f "$MEDIA_DIR/MANIFEST" ]]; then
  names=()
  while IFS= read -r entry || [[ -n "$entry" ]]; do
    [[ "$entry" =~ ^[[:space:]]*(#|$) ]] && continue
    read -r state name reason <<< "$entry"
    case "$state" in
      generated|skipped) validate_name "$name" ;;
      *)
        echo 'oracle-capture.sh: invalid manifest entry: expected generated NAME or skipped NAME REASON' >&2
        exit 2
        ;;
    esac
    [[ "$state" == "generated" ]] || continue
    if [[ -n "$reason" ]]; then
      echo 'oracle-capture.sh: invalid manifest name: generated NAME must occupy one line and contain no whitespace' >&2
      exit 2
    fi
    if [[ ! -f "$MEDIA_DIR/$name" ]]; then
      echo "oracle-capture.sh: manifest file is missing: $name" >&2
      exit 2
    fi
    names+=("$name")
  done < "$MEDIA_DIR/MANIFEST"
  for name in "${names[@]}"; do
    capture_file "$name"
  done
else
  while IFS= read -r -d '' media_file; do
    name="${media_file#"$MEDIA_DIR"/}"
    [[ "$name" == "MANIFEST" || "$name" == *.log ]] && continue
    capture_file "$name"
  done < <(find "$MEDIA_DIR" -type f -print0)
fi
