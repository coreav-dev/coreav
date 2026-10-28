#!/bin/bash
# count.sh TREE OUT: instructions a call of Auto::filter_edge per plane,
# direction and bS, for the kernel crate of TREE (1,100 minus 100 calls).
T=$(cd $(dirname $0) && pwd); K=$1; OUT=$2; D=$(mktemp -d)
mkdir -p $D/src; cp $T/dbkcount.rs $D/src/main.rs; sed "s#KPATH#$K/crates/coreav-kernels-h264#" $T/dbkcount.Cargo.toml > $D/Cargo.toml
(cd $D && cargo build -q --release) || exit 1
B=$D/target/release/dbkcount
for l in 1 0; do for r in 0 1; do for bs in 1 2 4; do echo "$l $r $bs"; done; done; done | xargs -P 8 -L1 sh -c '
 c() { valgrind --tool=callgrind --callgrind-out-file=/dev/null --toggle-collect=dbkcount::run_shape '"$B"' $1 $2 $3 $4 2>&1 | grep "Collected" | awk "{print \$4}"; }
 a=$(c $0 $1 $2 100); b=$(c $0 $1 $2 1100); echo "$0 $1 $2 $(( (b - a) / 1000 ))"' > $OUT
sort -k1,1r -k2,2n -k3,3n $OUT -o $OUT
