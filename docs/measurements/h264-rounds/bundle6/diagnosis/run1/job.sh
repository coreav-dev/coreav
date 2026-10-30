set -u
mkdir -p out
chmod +x decode-*
{ lscpu; nproc; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; sha256sum decode-* es/*; } > out/host.txt 2>&1
{ sudo apt-get -q update && sudo DEBIAN_FRONTEND=noninteractive apt-get -q -y install linux-perf && sudo sysctl -q kernel.perf_event_paranoid=-1; } > out/perf-install.log 2>&1
echo "start $(date -u +%T)" >> out/progress.txt
python3 run.py ${STAT_ROUNDS:-8} ${TD_ROUNDS:-2} >> out/progress.txt 2>&1
echo "end rc=$? $(date -u +%T)" >> out/progress.txt
