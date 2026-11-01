set -u
mkdir -p out
chmod +x decode-* cg.sh
{ lscpu; nproc; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; sha256sum decode-* es/*; } > out/host.txt 2>&1
{ sudo apt-get -q update && sudo DEBIAN_FRONTEND=noninteractive apt-get -q -y install linux-perf valgrind && sudo sysctl -q kernel.perf_event_paranoid=-1; } > out/perf-install.log 2>&1
echo "start $(date -u +%T)" >> out/progress.txt
python3 run.py 8 0 >> out/progress.txt 2>&1
echo "stat end rc=$? $(date -u +%T)" >> out/progress.txt
bash cg.sh
echo "cg end $(date -u +%T) $(wc -l < out/cg/done.txt) calls" >> out/progress.txt
for f in out/cg/*.out; do gzip -9 $f; done
echo "end $(date -u +%T)" >> out/progress.txt
