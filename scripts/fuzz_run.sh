#!/usr/bin/env bash
# Builds and runs the fuzz targets for CI: the pull-request run in ci.yml and
# the daily run in fuzz-nightly.yml (docs/design/ci-checks.md, "The fuzz
# runs"). Every project builds into one target directory, target/fuzz, which
# the workflows cache.
#
# Nothing a fuzzer prints reaches the log. Each target's output goes to a
# file; a target that ends in a finding (a crash, a hang, an out-of-memory
# report or any other failure) leaves that file and its libFuzzer artifacts
# in OUT/findings/NAME, for the caller to keep privately or to drop
# (docs/design/fuzz-campaign.md, "Findings"). The log names the target only.
#
# Usage:
#   scripts/fuzz_run.sh setup                 install the toolchain and cargo-fuzz
#   scripts/fuzz_run.sh build PLAN            build every project PLAN names
#   scripts/fuzz_run.sh run PLAN SECONDS OUT  run each target for SECONDS
#   scripts/fuzz_run.sh prune                 drop what the cache need not keep
#   scripts/fuzz_run.sh toolchain             print the pinned nightly
#
# PLAN holds one "FUZZ_DIR TARGET" line per target (scripts/fuzz_plan.py).
set -euo pipefail

# Pinned so that the cached build stays valid until the pin moves.
TOOLCHAIN=nightly-2026-10-27
CARGO_FUZZ=0.13.2
TARGET_DIR="$PWD/target/fuzz"

fuzz() { cargo "+$TOOLCHAIN" fuzz "$@"; }

case "${1:-}" in
  setup)
    rustup toolchain install "$TOOLCHAIN" --profile minimal
    cargo install cargo-fuzz --version "$CARGO_FUZZ" --locked
    ;;
  build)
    awk '{print $1}' "$2" | uniq | while read -r dir; do
      echo "::group::build $dir"
      (cd "$dir" && fuzz build --target-dir "$TARGET_DIR") < /dev/null
      echo "::endgroup::"
    done
    ;;
  run)
    plan=$2 seconds=$3 out=$4
    mkdir -p "$out/logs" "$out/findings"
    failed=0
    while read -r dir target; do
      name="$(basename "$(dirname "$dir")")-$target"
      log="$out/logs/$name.log"
      # Reproducers of past findings (FUZZ_DIR/regressions/TARGET, kept in
      # git, unlike the corpus) run first, under the same checks.
      corpora=()
      if [ -d "$dir/regressions/$target" ]; then
        mkdir -p "$dir/corpus/$target"
        corpora=("./corpus/$target" "regressions/$target")
      fi
      if (cd "$dir" && fuzz run --target-dir "$TARGET_DIR" "$target" ${corpora[@]+"${corpora[@]}"} \
            -- "-max_total_time=$seconds" -rss_limit_mb=2048 -timeout=10) > "$log" 2>&1 < /dev/null; then
        echo "$dir $target: $(grep -o 'Done [0-9]* runs in [0-9]* second(s)' "$log" | tail -n 1)"
      else
        failed=1
        mkdir -p "$out/findings/$name"
        mv "$log" "$out/findings/$name/"
        if [ -d "$dir/artifacts/$target" ]; then
          cp -R "$dir/artifacts/$target" "$out/findings/$name/artifacts"
        fi
        echo "::error::$dir $target: a finding; its input and output are withheld from this log"
      fi
    done < "$plan"
    exit "$failed"
    ;;
  prune)
    # CoreAV's own crates and the fuzz targets are rebuilt from the checkout
    # anyway; the third-party dependencies are what a cached build saves.
    [ -d "$TARGET_DIR" ] || exit 0
    find "$TARGET_DIR" -type d \( -name release -o -name debug \) -prune -print | while read -r profile; do
      find "$profile" -maxdepth 1 -type f -delete
      if [ -d "$profile/deps" ]; then
        find "$profile/deps" -maxdepth 1 -type f ! -name 'lib*' -delete
      fi
      rm -rf "$profile"/deps/libcoreav_* "$profile"/build/coreav* "$profile"/.fingerprint/coreav* \
        "$profile/incremental" "$profile/examples"
    done
    ;;
  toolchain)
    echo "$TOOLCHAIN"
    ;;
  *)
    echo "usage: $0 setup | build PLAN | run PLAN SECONDS OUT | prune | toolchain" >&2
    exit 2
    ;;
esac
