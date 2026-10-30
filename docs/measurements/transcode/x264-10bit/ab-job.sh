set -u
mkdir -p out
chmod +x bin/*
(sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq libdav1d7 python3-numpy) > out/apt.log 2>&1
{ lscpu; nproc; free -g; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; sha256sum bin/* ab.py profile.py; bin/ffmpeg -version | head -1; bin/x264 --version | head -1; } > out/host.txt 2>&1
echo "start $(date -u +%T)" >> out/progress.txt
python3 ab.py --bin bin --files files --out out --tmp /dev/shm/ab > out/ab.log 2>&1
echo "rc=$? end $(date -u +%T)" >> out/progress.txt
