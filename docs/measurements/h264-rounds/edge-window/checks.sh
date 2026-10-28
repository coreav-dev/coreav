#!/bin/bash
# checks.sh: identity (dec-k vs dec-base, 32 streams, 1 and 8 threads), the tests, then conformance on k (~/coreav).
cd ~/r5/ew
./ident.sh ~/r5/ew/dec-base ~/r5/ew/dec-k > identity.txt 2>&1
echo "identity: $(awk '{print $3}' identity.txt | sort | uniq -c | tr '\n' ' ')" > checks.txt
cd ~/coreav
echo "tests: $(CARGO_TARGET_DIR=~/r5/tgt-test cargo test -q --release -p coreav-h264 -p coreav-kernels-h264 2>&1 | grep -o '[0-9]* passed; [0-9]* failed' | awk '{p+=$1; f+=$3} END {print p" passed, "f" failed"}')" >> ~/r5/ew/checks.txt
rm -rf ~/conf/work/*
echo "detect 1,2,8: $(python3 -I scripts/h264_conformance.py --zips ~/conf/zips --work ~/conf/work --threads 1,2,8 --report ~/r5/ew/conf-detect.json 2>&1 | tail -1)" >> ~/r5/ew/checks.txt
echo "scalar 1,2,8: $(python3 -I scripts/h264_conformance.py --zips ~/conf/zips --work ~/conf/work --kernels scalar --threads 1,2,8 --report ~/r5/ew/conf-scalar.json 2>&1 | tail -1)" >> ~/r5/ew/checks.txt
echo DONE >> ~/r5/ew/checks.txt
