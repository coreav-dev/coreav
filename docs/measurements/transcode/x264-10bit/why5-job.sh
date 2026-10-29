set -u
mkdir -p out
chmod +x bin/*
{
  sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq linux-perf python3-numpy binutils
  echo 'deb http://deb.debian.org/debian-debug trixie-debug main' | sudo tee /etc/apt/sources.list.d/debug.list
  sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends x264 x264-dbgsym
} > out/apt.log 2>&1
sudo sysctl -q kernel.perf_event_paranoid=-1 kernel.kptr_restrict=0
{ lscpu; nproc; free -g; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; sha256sum bin/* why.py why3.py why5.py; bin/x264 --version | head -1; perf --version; dpkg -l x264 x264-dbgsym 'libx264-*' 2>/dev/null | grep '^ii'; /usr/bin/x264 --version | head -1; ldd /usr/bin/x264 | grep -i x264; } > out/host.txt 2>&1
echo "start $(date -u +%T)" >> out/progress.txt
python3 -I why5.py --bin bin --files files --out out --tmp /dev/shm/why5 > out/why5.log 2>&1
echo "rc=$? end $(date -u +%T)" >> out/progress.txt
