set -u
mkdir -p out
chmod +x bin/*
(sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq linux-perf python3-numpy) > out/apt.log 2>&1
sudo sysctl -q kernel.perf_event_paranoid=-1 kernel.kptr_restrict=0
sudo mount -o remount,mode=755 /sys/kernel/tracing >> out/apt.log 2>&1
{ lscpu; nproc; free -g; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; sha256sum bin/* why.py why2.py; bin/x264 --version | head -1; perf --version; } > out/host.txt 2>&1
echo "start $(date -u +%T)" >> out/progress.txt
python3 why2.py --bin bin --files files --out out --tmp /dev/shm/why2 > out/why2.log 2>&1
echo "rc=$? end $(date -u +%T)" >> out/progress.txt
