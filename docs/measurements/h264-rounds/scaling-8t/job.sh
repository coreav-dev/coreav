#!/bin/bash
# Plan S2's job (plan.md), run on the VM by scripts/gcp_vm_job.py from a directory holding this script, scale8.py,
# es/ (the 32 training streams), m-acf3, prof-acf3ee2f and ffmpeg (the BtbN build). Writes out/.
set -euo pipefail
cd "$(dirname "$0")"
J=$PWD; OUT=$J/out; mkdir -p $OUT
sudo DEBIAN_FRONTEND=noninteractive apt-get -qq update >/dev/null
sudo DEBIAN_FRONTEND=noninteractive apt-get -qq install -y linux-perf >/dev/null
sudo sysctl -q kernel.perf_event_paranoid=-1 kernel.kptr_restrict=0
{
  uname -a; perf --version
  grep -m1 'model name' /proc/cpuinfo; lscpu | grep -E '^Model:|CPU family|Stepping|Thread|Core|Socket|^CPU\(s\)'
  echo "nproc $(nproc)"; echo "smt $(cat /sys/devices/system/cpu/smt/active 2>/dev/null || echo unknown)"
  cat /sys/bus/event_source/devices/cpu*/caps/pmu_name 2>/dev/null || true
  sha256sum $J/m-acf3 $J/prof-acf3ee2f $J/ffmpeg $J/scale8.py $J/job.sh
  (cd $J/es && sha256sum *.h264)
} > $OUT/host.txt 2>&1
# S2-3: the host must have 8 vCPUs with SMT off.
if [ "$(nproc)" != 8 ] || [ "$(lscpu | sed -n 's/^Thread(s) per core: *//p')" != 1 ]; then
  echo "host check failed" >> $OUT/host.txt; exit 4
fi
python3 -I $J/scale8.py $J/es $OUT $J/m-acf3 $J/prof-acf3ee2f $J/ffmpeg 7 2 2 4880 > $OUT/scale8.log 2>&1
