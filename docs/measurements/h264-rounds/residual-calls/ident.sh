#!/bin/bash
# ident.sh A B: md5 of the output of builds /tmp/rb/bin-A and bin-B on the 32 streams at a tenth of the class frame counts, 1 and 8 threads.
declare -A FR=([sd]=180 [720p]=90 [1080p]=30 [4k]=15)
one() { f=$1; b=$2; t=$3; c=$(echo $f | cut -d. -f3); m=$(/tmp/rb/bin-$b --frames ${FR[$c]} --threads $t /tmp/pmu/up/es/$f | md5sum | cut -c1-32); echo "$m ${f:0:12} $b $t"; }
export -f one; export FR_SD=1
for f in $(ls /tmp/pmu/up/es); do for b in $1 $2; do for t in 1 8; do echo "$f $b $t"; done; done; done | \
  xargs -P 8 -L 1 bash -c 'declare -A FR=([sd]=180 [720p]=90 [1080p]=30 [4k]=15); f=$0; b=$1; t=$2; c=$(echo $f | cut -d. -f3); m=$(/tmp/rb/bin-$b --frames ${FR[$c]} --threads $t /tmp/pmu/up/es/$f | md5sum | cut -c1-32); echo "$m ${f:0:12} $b $t"'
