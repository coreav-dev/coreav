#!/usr/bin/env bash
set -euo pipefail
OUT="$RUNNER_TEMP/crf"
mkdir -p "$OUT/bin"
for arm in base cand; do
  if [ "$arm" = base ]; then sha="$BASELINE"; else sha="$CANDIDATE"; fi
  git worktree add --detach "$OUT/$arm" "$sha"
  cp ab/reference.rs "$OUT/$arm/crates/coreav-h264-encoder/examples/reference.rs"
  if [ "$arm" = cand ]; then
    python3 - "$OUT/$arm/crates/coreav-h264-encoder/examples/reference.rs" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1])
s = p.read_text()
old = 'let qp: u8 = a[2].parse()?;'
assert s.count(old) == 1
s = s.replace(old, 'let mode = match a[2].strip_prefix("crf:") { Some(v) => coreav_h264_encoder::RateMode::Crf(v.parse()?), None => coreav_h264_encoder::RateMode::FixedQp(a[2].parse()?) };')
assert s.count('width: w, height: h, qp,') == 1
p.write_text(s.replace('width: w, height: h, qp,', 'width: w, height: h, rate_mode: mode,'))
PY
  fi
  printf '\n[[example]]\nname = "reference"\nrequired-features = ["full"]\n' >> "$OUT/$arm/crates/coreav-h264-encoder/Cargo.toml"
  (cd "$OUT/$arm" && CARGO_TARGET_DIR="$OUT/target-$arm" cargo build --locked --release -p coreav-h264-encoder --features full --example reference)
  cp "$OUT/target-$arm/release/examples/reference" "$OUT/bin/$arm"
done
scripts/install-oracle-ffmpeg.sh "$OUT/bin"
