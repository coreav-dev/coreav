#!/usr/bin/env bash
set -euo pipefail
OUT="$1"
BASELINE="$2"
CANDIDATE="$3"
ROOT="$PWD"
mkdir -p "$OUT/bin"
for pair in "main:$BASELINE" "cand:$CANDIDATE"; do
  arm="${pair%%:*}"
  sha="${pair#*:}"
  git cat-file -e "$sha^{commit}"
  git worktree add --detach "$OUT/$arm" "$sha"
  cp "$ROOT/ab/abt.rs" "$OUT/$arm/crates/coreav-h264-encoder/examples/abt.rs"
  if [ "$arm" = cand ]; then
    python3 - "$OUT/$arm/crates/coreav-h264-encoder/examples/abt.rs" <<'PY'
import pathlib, sys
p = pathlib.Path(sys.argv[1])
s = p.read_text()
old = 'Encoder::new(settings, Limits { frames: n as u64, ..Limits::default() })?'
assert s.count(old) == 1
p.write_text(s.replace(old, 'Encoder::with_threads(settings, Limits { frames: n as u64, ..Limits::default() }, a[8].parse()?)?'))
PY
  fi
  printf '\n[[example]]\nname = "abt"\nrequired-features = ["full"]\n' >> "$OUT/$arm/crates/coreav-h264-encoder/Cargo.toml"
  (cd "$OUT/$arm" && CARGO_TARGET_DIR="$OUT/target-$arm" cargo build --locked --release -p coreav-h264-encoder --features full --example abt)
  cp "$OUT/target-$arm/release/examples/abt" "$OUT/bin/$arm-abt"
done
scripts/install-oracle-ffmpeg.sh "$OUT/bin"
python3 - "$OUT/bin" "$BASELINE" "$CANDIDATE" <<'PY'
import hashlib, json, pathlib, subprocess, sys
root = pathlib.Path(sys.argv[1])
hashes = {arm: hashlib.sha256((root / f'{arm}-abt').read_bytes()).hexdigest() for arm in ['main', 'cand']}
assert len(set(hashes.values())) == 2
(root / 'build.json').write_text(json.dumps({'baseline': sys.argv[2], 'candidate': sys.argv[3], 'binary_sha256': hashes, 'rustc': subprocess.check_output(['rustc', '-vV'], text=True)}, indent=2))
PY
