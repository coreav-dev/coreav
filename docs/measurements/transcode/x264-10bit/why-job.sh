set -u
part=$1
mkdir -p out
chmod +x bin/*
(sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq linux-perf python3-numpy strace) > out/apt.log 2>&1
if [ "$part" = intel ]; then
  {
    echo 'deb http://deb.debian.org/debian-debug trixie-debug main' | sudo tee /etc/apt/sources.list.d/debug.list
    sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends x264 ffmpeg
    dbg=$(apt-cache pkgnames | grep -E '^(x264|ffmpeg|libx264-[0-9]+|libavcodec[0-9]+|libavformat[0-9]+|libavutil[0-9]+|libswscale[0-9]+)-dbgsym$' | sort)
    echo "dbgsym: $dbg"
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq $dbg
  } >> out/apt.log 2>&1
fi
sudo sysctl -q kernel.perf_event_paranoid=-1 kernel.kptr_restrict=0
{ lscpu; nproc; free -g; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; sha256sum bin/* why.py; bin/ffmpeg -version | head -1; bin/x264 --version | head -1; perf --version; strace -V | head -1; df -h /dev/shm | tail -1; if [ -x /usr/bin/x264 ]; then dpkg -l x264 ffmpeg 'libx264-*' 'libavcodec[0-9]*' 2>/dev/null | grep '^ii'; /usr/bin/x264 --version | head -1; ldd /usr/bin/x264 | grep -i x264; fi; } > out/host.txt 2>&1
perf list 2>/dev/null | gzip > out/perf-list.txt.gz
echo "start $(date -u +%T)" >> out/progress.txt
python3 -I why.py --part "$part" --bin bin --files files --out out --tmp /dev/shm/why > out/why.log 2>&1
echo "rc=$? end $(date -u +%T)" >> out/progress.txt
