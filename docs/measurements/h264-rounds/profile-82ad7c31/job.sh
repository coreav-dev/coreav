#!/bin/bash
# The PMU profile's job, run on the VM by scripts/gcp_vm_job.py from a directory holding this script, rec_pmu.py,
# es/ (the 32 training streams), prof-<commit> (CoreAV with line tables) and ffmpeg (the BtbN build). Writes out/.
#
# The attribution build of FFmpeg 9.0.2 (function symbols only, no debug information) is built here from the pinned
# release tarball with the original profile's configuration plus --disable-debug; make's output goes to a log that is
# deleted unread (only the exit status is kept), and the source tree and the tarball are deleted right after make,
# before anything is profiled. out/build-order.txt records that order with times.
# ../profile-ae609547-pmu/job.sh with --disable-doc added to the configuration. A copy of ../profile-acf3ee2f-pmu/job.sh: it runs rec_pmu.py's `ipc` mode (one thread, into out/t1) and then its
# `ipc8` mode (eight threads, into out/t8), on a VM of eight cores with one thread each.
set -euo pipefail
cd "$(dirname "$0")"
J=$PWD; OUT=$J/out; mkdir -p $OUT
CORE=$(ls $J/prof-* | head -1)
note() { echo "$(date -u +%FT%TZ) $*" >> $OUT/build-order.txt; }
sudo DEBIAN_FRONTEND=noninteractive apt-get -qq update >/dev/null
sudo DEBIAN_FRONTEND=noninteractive apt-get -qq install -y linux-perf build-essential nasm xz-utils curl >/dev/null
sudo sysctl -q kernel.perf_event_paranoid=-1 kernel.kptr_restrict=0
{
  uname -a; perf --version; gcc --version | head -1; nasm -v
  grep -m1 'model name' /proc/cpuinfo; lscpu | grep -E '^Model:|CPU family|Stepping|Thread|Core|Socket|^CPU\(s\)'
  grep -o 'avx512[a-z_0-9]*' /proc/cpuinfo | sort -u | tr '\n' ' '; echo
  echo "pmu flags: $(grep -m1 flags /proc/cpuinfo | grep -o 'arch_perfmon\|pdcm\|arch_lbr' | tr '\n' ' ')"
  ls /sys/bus/event_source/devices/ | tr '\n' ' '; echo
  cat /sys/bus/event_source/devices/cpu*/caps/pmu_name 2>/dev/null || true
  sha256sum $CORE $J/ffmpeg
} > $OUT/host.txt 2>&1

# The attribution build.
note "download start"
curl -fsSL -o /tmp/ffmpeg-9.0.2.tar.xz https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz
echo "8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e  /tmp/ffmpeg-9.0.2.tar.xz" | sha256sum -c --quiet
note "tarball sha256 verified"
mkdir -p /tmp/ffb && tar -xJf /tmp/ffmpeg-9.0.2.tar.xz -C /tmp/ffb
set +e
(cd /tmp/ffb/ffmpeg-9.0.2 && ./configure --disable-everything --enable-decoder=h264 --enable-parser=h264 \
  --enable-demuxer=h264 --enable-muxer=null,rawvideo --enable-encoder=rawvideo,wrapped_avframe \
  --enable-protocol=file,pipe --enable-filter=null,format,scale --disable-stripping --disable-debug --disable-doc \
  > /tmp/ff-build.log 2>&1 && make -j8 >> /tmp/ff-build.log 2>&1)
st=$?
set -e
note "configure and make exit status $st"
[ $st = 0 ] && cp /tmp/ffb/ffmpeg-9.0.2/ffmpeg_g $J/ffattr
rm -rf /tmp/ffb /tmp/ffmpeg-9.0.2.tar.xz /tmp/ff-build.log
note "source tree, tarball and build log deleted (log unread)"
[ $st = 0 ] || exit 2
# Section names only: no .debug_* section may be present.
readelf -SW $J/ffattr | sed -n 's/^ *\[ *[0-9]*\] \([^ ]*\).*/\1/p' > $OUT/ffattr-sections.txt
if grep -q '^\.debug' $OUT/ffattr-sections.txt; then note "debug sections present; stopping"; exit 3; fi
note "no .debug_* sections"
nm -n --defined-only $J/ffattr > $OUT/ffattr.nm.txt
readelf -lW $J/ffattr > $OUT/ffattr.readelf-l.txt
sha256sum $J/ffattr >> $OUT/host.txt
# The attribution build decodes as the timing build does.
F=$J/es/9db41aa16b5a.0.720p.h264
for b in ffmpeg ffattr; do
  echo "$b $($J/$b -nostdin -v error -threads 1 -i $F -frames:v 300 -f rawvideo - | md5sum | cut -c1-32)" >> $OUT/ffattr-output.txt
done
note "profiling start"
for m in ipc ipc8; do
  d=$OUT/t${m#ipc}; [ $m = ipc ] && d=$OUT/t1
  mkdir -p $d && cp $OUT/ffattr.nm.txt $OUT/ffattr.readelf-l.txt $d/
  python3 -I $J/rec_pmu.py $J/es $d $CORE $J/ffmpeg $J/ffattr 2 $m > $d/rec.log 2>&1
  note "profiling $m done"
done
