#!/usr/bin/env bash
# `zig cc` or `zig ar` for a musl target, as the C compiler and archiver of
# the crates that compile C (ring, for HTTPS): scripts/zig.sh cc|ar ARGS.
# ZIG is the command that runs zig (default `zig`, for example
# `python3 -m ziglang`); ZIG_TARGET its target (x86_64-linux-musl,
# aarch64-linux-musl). cc-rs passes Rust's target triple as --target, which
# zig does not read; it is replaced by ZIG_TARGET.
set -euo pipefail
tool="$1"
shift
if [ "$tool" = ar ]; then
  exec ${ZIG:-zig} ar "$@"
fi
args=()
for arg in "$@"; do
  case "$arg" in
    --target=*) ;;
    *) args+=("$arg") ;;
  esac
done
exec ${ZIG:-zig} cc -target "$ZIG_TARGET" "${args[@]}"
