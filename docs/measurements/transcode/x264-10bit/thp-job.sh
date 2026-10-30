set -u
mkdir -p out
chmod +x bin/*
(sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq linux-perf python3-numpy strace) > out/apt.log 2>&1
sudo sysctl -q kernel.perf_event_paranoid=-1 kernel.kptr_restrict=0
{ lscpu; nproc; free -g; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; cat /sys/kernel/mm/transparent_hugepage/enabled /sys/kernel/mm/transparent_hugepage/defrag; python3 --version; sha256sum bin/* why.py why3.py thp.py; bin/x264 --version | head -1; perf --version; } > out/host.txt 2>&1
echo "start $(date -u +%T)" >> out/progress.txt
python3 -I thp.py --bin bin --files files --out out --tmp /dev/shm/thp > out/thp.log 2>&1
echo "rc=$? end $(date -u +%T)" >> out/progress.txt
