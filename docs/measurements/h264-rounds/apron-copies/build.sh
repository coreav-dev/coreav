#!/bin/bash
# build.sh NAME: dist decode example of ~/ap/src-NAME into ~/ap/decode-NAME
set -e; n=$1
[ -z "${RUSTFLAGS:-}${CARGO_ENCODED_RUSTFLAGS:-}" ] || { echo "RUSTFLAGS set"; exit 4; }
cd ~/ap/src-$n && CARGO_TARGET_DIR=~/ap/tgt-$n cargo build --locked --profile dist --example decode -p coreav-h264 2>&1 | tail -1
cp ~/ap/tgt-$n/dist/examples/decode ~/ap/decode-$n
