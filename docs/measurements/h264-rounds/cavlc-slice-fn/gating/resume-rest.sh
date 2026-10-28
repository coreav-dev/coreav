#!/bin/bash
# resume-rest.sh NAME ANALYZE.py "call:class:frames ...": the calls a stopped run-1t.sh did not do, one after another with resume-class.sh (same binaries, harness and options), waiting 5 minutes and trying again while the host gate keeps failing (recorded host fault); then the analysis.
NAME=$1; ANALYZE=$2; shift 2; OUT=$HOME/ab/$NAME/out
end=$(( $(date +%s) + 36000 ))
for c in "$@"; do
  IFS=: read call cls F <<< "$c"
  until [ -s $OUT/$call.$cls.json ] && grep -q "^$call.$cls done" $OUT/progress.txt; do
    [ $(date +%s) -lt $end ] || { echo "gave up at $call.$cls" >> $OUT/progress.txt; exit 7; }
    rm -f $OUT/STOPPED
    $HOME/ab/scripts/resume-class.sh $NAME $call $cls $F || { echo "retry $call.$cls $(date -u +%H:%M)" >> $OUT/progress.txt; sleep 300; }
  done
done
cp "$ANALYZE" $OUT/analyze.py; python3 -I $OUT/analyze.py $OUT > $OUT/analysis.txt || true
echo ALL >> $OUT/progress.txt
