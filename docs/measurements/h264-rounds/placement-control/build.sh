#!/bin/bash
# W: a scratch directory holding wt (a worktree of the base commit), p (the patches), bin.
# build.sh MODE VARIANT: MODE d (default) | f (functions and loops on 64-byte lines) | o (f + hot-function order file);
# VARIANT base | n1 | n2 | dk (a patch in $W/p). Output $W/bin/MODE-VARIANT.
set -euo pipefail; W=${W:-/tmp/pl}
m=$1; v=$2; cd $W/wt; git checkout -q -- crates ; git clean -fdq crates
[ "$v" != base ] && git apply $W/p/$v.patch
F="-C llvm-args=-align-all-functions=6 -C llvm-args=-align-loops=64"
case $m in d) R="";; f) R="$F";; o) R="$F -C link-arg=-Wl,--symbol-ordering-file=$W/order.txt -C link-arg=-Wl,--no-warn-symbol-ordering";; esac
RUSTFLAGS="$R" CARGO_TARGET_DIR=$W/t-$m cargo build -q --locked --profile dist --example decode -p coreav-h264
cp $W/t-$m/dist/examples/decode $W/bin/$m-$v
git checkout -q -- crates ; git clean -fdq crates
