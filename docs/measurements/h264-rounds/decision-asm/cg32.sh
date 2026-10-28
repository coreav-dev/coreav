#!/bin/bash
# cg32.sh OUTDIR BIN...: callgrind total instructions per stream at a tenth of the class frame counts.
out=$1; shift
declare -A FR=([sd]=180 [720p]=90 [1080p]=30 [4k]=15)
for f in $(ls /tmp/pmu/up/es); do c=$(echo $f | cut -d. -f3); for b in "$@"; do
  echo "valgrind --tool=callgrind --callgrind-out-file=/dev/null /tmp/rb/$b --frames ${FR[$c]} --discard /tmp/pmu/up/es/$f >/dev/null 2>$out/${f:0:12}.$b.log"
done; done | xargs -P 7 -I{} timeout 1800 bash -c "{}"
for f in $(ls /tmp/pmu/up/es); do echo -n "${f:0:12} $(echo $f | cut -d. -f3)"; for b in "$@"; do echo -n " $b $(grep -h Collected $out/${f:0:12}.$b.log | awk '{print $4}')"; done; echo; done > $out/summary.txt
