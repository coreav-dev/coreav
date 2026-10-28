#!/bin/bash
# The banded hints' part-only probe's job (README.md), run on the VM by scripts/gcp_vm_job.py from a directory holding
# this script, rec_hints.py, es/ (the 32 training streams), prof-<arm> for each arm (CoreAV with line tables) and
# ffmpeg (the BtbN build, timed in perf stat only; no FFmpeg is built or profiled here). Writes out/.
set -euo pipefail
cd "$(dirname "$0")"
J=$PWD; OUT=$J/out; mkdir -p $OUT
sudo DEBIAN_FRONTEND=noninteractive apt-get -qq update >/dev/null
sudo DEBIAN_FRONTEND=noninteractive apt-get -qq install -y linux-perf >/dev/null
sudo sysctl -q kernel.perf_event_paranoid=-1 kernel.kptr_restrict=0
{
  uname -a; perf --version
  grep -m1 'model name' /proc/cpuinfo; lscpu | grep -E '^Model:|CPU family|Stepping|Thread|Core|Socket|^CPU\(s\)'
  grep -o 'avx512[a-z_0-9]*\|3dnowprefetch' /proc/cpuinfo | sort -u | tr '\n' ' '; echo
  cat /sys/bus/event_source/devices/cpu*/caps/pmu_name 2>/dev/null || true
  sha256sum $J/prof-* $J/ffmpeg
} > $OUT/host.txt 2>&1
python3 -I $J/rec_hints.py $J/es $OUT 2 $J/ffmpeg base=$J/prof-base E=$J/prof-E EW=$J/prof-EW EO=$J/prof-EO \
  EWO=$J/prof-EWO > $OUT/rec.log 2>&1
