#!/bin/bash
# Plan S2a's job (plan.md): call graphs of us1 and us8 for the callers of libc's and the kernel's growth. Writes out/.
set -euo pipefail
cd "$(dirname "$0")"
J=$PWD; OUT=$J/out; mkdir -p $OUT/cg
sudo DEBIAN_FRONTEND=noninteractive apt-get -qq update >/dev/null
sudo DEBIAN_FRONTEND=noninteractive apt-get -qq install -y linux-perf libc6-dbg >/dev/null
sudo sysctl -q kernel.perf_event_paranoid=-1 kernel.kptr_restrict=0
{ uname -a; perf --version; grep -m1 'model name' /proc/cpuinfo; echo "nproc $(nproc)"
  lscpu | grep -E '^Thread'; dpkg -l libc6 libc6-dbg | tail -2; sha256sum $J/prof-acf3ee2f $J/job2.sh; } > $OUT/host.txt 2>&1
if [ "$(nproc)" != 8 ] || [ "$(lscpu | sed -n 's/^Thread(s) per core: *//p')" != 1 ]; then
  echo "host check failed" >> $OUT/host.txt; exit 4
fi
declare -A FR=([sd]=1800 [720p]=900 [1080p]=300 [4k]=150)
SEL=$(cd $J/es && { ls *.sd.h264; for c in 720p 1080p 4k; do x=(*.$c.h264); echo "${x[0]} ${x[1]}"; done; })
for f in $SEL; do
  c=$(echo $f | cut -d. -f3); n=${FR[$c]}
  for t in 1 8; do
    pin=(); [ $t = 1 ] && pin=(taskset -c 3)
    perf record -q -F 1000 -e cycles --call-graph dwarf,16384 -o /tmp/p.data -- "${pin[@]}" $J/prof-acf3ee2f \
      --frames $n --threads $t --discard $J/es/$f > /dev/null 2>&1
    perf script --no-inline -i /tmp/p.data -F period,ip,sym,dso 2>/dev/null | gzip > $OUT/cg/${f%.h264}.$t.txt.gz
    rm -f /tmp/p.data
  done
done
echo done > $OUT/done
