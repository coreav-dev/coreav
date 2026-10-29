set -u
mkdir -p out
chmod +x bin/*
(sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq libdav1d7 libvpx9) > out/apt.log 2>&1
{ lscpu; nproc; free -g; uname -a; df -h /dev/shm; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; ldd bin/coreav; sha256sum bin/* transcode_speed.py transcode_check.py; bin/ffmpeg -version | head -1; } > out/host.txt 2>&1
if ldd bin/coreav | grep -q "not found"; then echo "missing libraries" >> out/progress.txt; exit 2; fi
export PATH="$PWD/bin:$PATH"
echo "start $(date -u +%T)" >> out/progress.txt
python3 -I transcode_speed.py run --selection selection.json --files files --coreav bin/coreav --ffmpeg bin/ffmpeg --ffprobe bin/ffprobe --arm "ffmpeg=--encoder ffmpeg --ffmpeg $PWD/bin/ffmpeg" --arm "x264=--encoder x264 --x264 $PWD/bin/x264" --gate gate.mp4 --out out --tmp /dev/shm/transcode-speed --classes vp9 > out/speed.log 2>&1
echo "rc=$? end $(date -u +%T)" >> out/progress.txt
