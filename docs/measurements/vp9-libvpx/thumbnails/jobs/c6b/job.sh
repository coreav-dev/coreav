set -u
mkdir -p out
chmod +x bin/*
(sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq libvpx9 libdav1d7) > out/apt.log 2>&1
{ lscpu; nproc; free -g; uname -a; df -h /dev/shm; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; dpkg-query -W libvpx9 libdav1d7; for b in bin/coreav-libvpx bin/coreav-fallback bin/decode-oracle; do echo "== $b"; ldd $b; done; sha256sum bin/* vp9_speed.py thumbnail_oracle.py transcode_check.py; bin/ffmpeg -version | head -1; bin/ffmpeg -hide_banner -decoders | grep -E " (vp9|libvpx-vp9) "; echo "default decoder for VP9:"; bin/ffmpeg -hide_banner -v verbose -i files/13e0154bf1ddeb368ae418c66e8ae3d817a1dc8341de3203fa89bfbae6404e52.webm -frames:v 1 -f null - 2>&1 | grep -E "Stream #0:[0-9]+ -> #0:[0-9]+ \(vp9"; } > out/host.txt 2>&1
if ldd bin/coreav-libvpx bin/coreav-fallback bin/decode-oracle | grep -q "not found"; then echo "missing libraries" >> out/progress.txt; exit 2; fi
export PATH="$PWD/bin:$PATH"
echo "start $(date -u +%T)" >> out/progress.txt
python3 -I vp9_speed.py run --selection selection.json --files files --coreav libvpx=$PWD/bin/coreav-libvpx --coreav fallback=$PWD/bin/coreav-fallback --ffmpeg $PWD/bin/ffmpeg --ffprobe $PWD/bin/ffprobe --gate files/13e0154bf1ddeb368ae418c66e8ae3d817a1dc8341de3203fa89bfbae6404e52.webm --out out --tmp /dev/shm/vp9-speed --decode-oracle $PWD/bin/decode-oracle --decode-rounds 3 > out/speed.log 2>&1
echo "speed rc=$? end $(date -u +%T)" >> out/progress.txt
python3 -I vp9_speed.py oracle --selection selection.json --files files --decode-oracle $PWD/bin/decode-oracle --ffmpeg $PWD/bin/ffmpeg --ffprobe $PWD/bin/ffprobe --out out --jobs 2 > out/oracle.log 2>&1
echo "oracle rc=$? end $(date -u +%T)" >> out/progress.txt
