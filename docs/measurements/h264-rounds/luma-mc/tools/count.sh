#!/bin/bash
# count.sh TREE OUT: instructions a call of Auto::luma_mc and luma_mc_avg, per
# shape and position, for the kernel crate of the checkout TREE, by callgrind
# (the difference of 1,100 and 100 calls, over 1,000). Writes shapes to OUT.
T=$(cd $(dirname $0) && pwd); K=$1; OUT=$2; D=$(mktemp -d)
mkdir -p $D/src; cp $T/lmccount.rs $D/src/main.rs; sed "s#KPATH#$K/crates/coreav-kernels-h264#" $T/lmccount.Cargo.toml > $D/Cargo.toml
(cd $D && cargo build -q --release) || exit 1
B=$D/target/release/lmccount
for avg in 0 1; do for wh in "16 16" "16 8" "8 16" "8 8" "8 4" "4 8" "4 4"; do for xf in 0 1 2 3; do for yf in 0 1 2 3; do
 echo "$wh $xf $yf $avg"; done; done; done; done | xargs -P 8 -L1 sh -c '
 c() { valgrind --tool=callgrind --callgrind-out-file=/dev/null --toggle-collect=lmccount::run_shape '"$B"' $1 $2 $3 $4 $5 $6 2>&1 | grep "Collected" | awk "{print \$4}"; }
 a=$(c $0 $1 $2 $3 $4 100); b=$(c $0 $1 $2 $3 $4 1100); echo "$0 $1 $2 $3 $4 $(( (b - a) / 1000 ))"' > $OUT
sort -n -k5 -k1r -k2r -k3 -k4 $OUT -o $OUT
