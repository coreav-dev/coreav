#!/bin/bash
set -u
cd "$HOME/job"
mkdir -p out
B=$PWD/bin
export ARMS_FILES=$PWD/files
{
  echo "== host"; uname -srm; nproc; lscpu | grep -E 'Model name|^CPU\(s\)|Thread|Core|Socket|MHz|L3'
  cat /proc/cmdline | tr ' ' '\n' | grep -v -i 'root=\|BOOT_IMAGE' | tr '\n' ' '; echo
} > out/host.txt 2>&1
sudo DEBIAN_FRONTEND=noninteractive apt-get -qq update >/dev/null 2>&1
sudo DEBIAN_FRONTEND=noninteractive apt-get -qq install -y libdav1d7 >/dev/null 2>&1
dpkg-query -W libdav1d7 >> out/host.txt 2>&1
"$B/ffmpeg" -version | head -1 >> out/host.txt
"$B/coreav" --version >> out/host.txt 2>&1
sha256sum bin/* >> out/host.txt
test "$(nproc)" = 8 || { echo "host: expected 8 CPUs, got $(nproc)" >> out/host.txt; exit 3; }
sudo mkdir -p /dev/shm/thumbbench /dev/shm/arms && sudo chmod 1777 /dev/shm/thumbbench /dev/shm/arms
S=bench/spikes/thumbnail-bench
for set in dev holdout; do
  echo "== speed $set $(date -u +%H:%M:%S)"
  python3 -I $S/speed.py --coreav "$B/coreav" --ffmpeg "$B/ffmpeg" --ffprobe "$B/ffprobe" --set $set \
    --classes av1 --work "$PWD/work" --out out/speed-$set.jsonl
done
echo "== arms $(date -u +%H:%M:%S)"
python3 -I arms.py --files files --list list.jsonl --coreav "$B/coreav" --decode-oracle "$B/decode-oracle" \
  --ffmpeg "$B/ffmpeg" --ffprobe "$B/ffprobe" --scripts bench/scripts --out out/arms.jsonl
echo "== end $(date -u +%H:%M:%S)"
