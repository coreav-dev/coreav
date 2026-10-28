#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURES_DIR="${1:?usage: oracle-selfcheck.sh FIXTURES_DIR WORK_DIR}"
WORK_DIR="${2:?usage: oracle-selfcheck.sh FIXTURES_DIR WORK_DIR}"
COREAV_ORACLE="${COREAV_ORACLE:-$ROOT/target/release/coreav-oracle}"

if [[ "$#" -ne 2 ]]; then
  echo "usage: oracle-selfcheck.sh FIXTURES_DIR WORK_DIR" >&2
  exit 2
fi
if [[ ! -x "$COREAV_ORACLE" ]]; then
  echo "oracle-selfcheck.sh: COREAV_ORACLE is not executable: $COREAV_ORACLE" >&2
  exit 2
fi

mkdir -p "$WORK_DIR"
scripts="$ROOT/scripts/oracle-capture.sh"
"$scripts" --media "$FIXTURES_DIR" --out "$WORK_DIR/reference"
"$scripts" --media "$FIXTURES_DIR" --out "$WORK_DIR/candidate"
"$COREAV_ORACLE" compare \
  --reference "$WORK_DIR/reference" \
  --candidate "$WORK_DIR/candidate" \
  --profile fixtures \
  --report "$WORK_DIR/report.json"
"$COREAV_ORACLE" selfcheck \
  --reference "$WORK_DIR/reference" \
  --candidate "$WORK_DIR/candidate"
