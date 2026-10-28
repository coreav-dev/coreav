#!/bin/bash
# checks.sh: identity (dec-k vs dec-base, 32 streams, 1 and 8 threads), then the workspace tests and conformance on k.
cd ~/b4
./ident.sh ~/b4/dec-base ~/b4/dec-k > identity.txt 2>&1
echo "identity: $(awk '{print $3}' identity.txt | sort | uniq -c | tr '\n' ' ')" > checks.txt
cd ~/b4/tree
echo "tests: $(cargo test -q --release -p coreav-h264 -p coreav-kernels-h264 2>&1 | grep -E '^test result' | awk '{p+=$4; f+=$6} END{print p" passed, "f" failed"}')" >> ~/b4/checks.txt
echo "detect 1,2,8: $(python3 -I scripts/h264_conformance.py --zips ~/conf/zips --work ~/conf/work --threads 1,2,8 --report ~/b4/conf-detect.json 2>&1 | tail -1)" >> ~/b4/checks.txt
echo "scalar 1,2,8: $(python3 -I scripts/h264_conformance.py --zips ~/conf/zips --work ~/conf/work --kernels scalar --threads 1,2,8 --report ~/b4/conf-scalar.json 2>&1 | tail -1)" >> ~/b4/checks.txt
echo DONE >> ~/b4/checks.txt
