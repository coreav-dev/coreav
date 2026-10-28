#!/bin/bash -eu
# OSS-Fuzz build script for CoreAV (tools/oss-fuzz/README.md).
#
# Builds every fuzz target of every crates/*/fuzz project, the same list CI
# fuzzes and the fuzz campaigns run (scripts/fuzz_plan.py --all), into $OUT
# as CRATE_TARGET (crate without its coreav- prefix, dashes as
# underscores): two crates have `probe` and `packets` targets. Each target
# gets the dictionary of the formats it reads, when there is one.

cd "$SRC/coreav"
# The OSS-Fuzz image sets RUSTUP_TOOLCHAIN to its nightly, which overrides the
# workspace's rust-toolchain.toml; elsewhere, the default toolchain.
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-$(rustup default | cut -d' ' -f1)}"
DICTS="$SRC/coreav/tools/oss-fuzz"

python3 scripts/fuzz_plan.py --all > "$WORK/coreav-fuzz-targets.txt"
count=0
while read -r project target; do
  (cd "$project" && cargo fuzz build -O --debug-assertions "$target")
  crate=$(basename "$(dirname "$project")")
  name="${crate#coreav-}"
  name="${name//-/_}_${target}"
  cp "$project/target/x86_64-unknown-linux-gnu/release/$target" "$OUT/$name"
  case "$crate/$target" in
    coreav-mkv/* | coreav-isobmff-write/mkv_remux) dict=ebml.dict ;;
    coreav-isobmff/* | coreav-isobmff-write/* | coreav-text/subtitles | coreav-wasm/*) dict=isobmff.dict ;;
    *) dict= ;;
  esac
  if [ -n "$dict" ]; then
    cp "$DICTS/$dict" "$OUT/$name.dict"
  fi
  count=$((count + 1))
done < "$WORK/coreav-fuzz-targets.txt"
echo "built $count fuzz targets"
