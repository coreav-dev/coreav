set -u
mkdir -p out
chmod +x bin/*
(sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq python3-numpy strace) > out/apt.log 2>&1
{ lscpu; nproc; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; cat /proc/sys/fs/pipe-max-size; python3 --version; sha256sum bin/* diag.py profile.py; bin/x264 --version | head -1; } > out/host.txt 2>&1
echo "start $(date -u +%T)" >> out/progress.txt
python3 diag.py --bin bin --files files --out out --tmp /dev/shm/diag > out/diag.log 2>&1
echo "rc=$? end $(date -u +%T)" >> out/progress.txt
