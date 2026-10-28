set -u
mkdir -p out
chmod +x decode-* ffmpeg
{ lscpu; nproc; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; sha256sum decode-* ffmpeg h264_parity.py gate1t.py gate.h264 refs/* es/*; } > out/host.txt 2>&1
echo "start $(date -u +%T)" >> out/progress.txt
for i in 1 2 3; do
  python3 gate1t.py ref "ref$i" >> out/progress.txt 2>&1
  [ $i -lt 3 ] && sleep ${REF_SLEEP:-300}
done
echo "refs done $(date -u +%T)" >> out/progress.txt
for attempt in 1 2 3; do
  ok=0; for i in 1 2 3 4 5; do python3 gate1t.py check "before sd #$attempt" && { ok=1; break; }; [ $i -lt 5 ] && sleep ${CHECK_SLEEP:-60}; done
  if [ $ok = 0 ]; then echo "g.sd attempt $attempt: the host failed its gate, no call" >> out/progress.txt; continue; fi
  echo "g.sd attempt $attempt $(date -u +%T)" >> out/progress.txt
  python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 1800 --pin 2 --rounds 6 --shuffle-seed 20261031 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --null-bar 0.04 --spread-bar 1.5 --null-arm base --require-cpu 8581C --report out/g.sd.json es/41e324ebb708.0.sd.h264 es/5185721dda11.0.sd.h264 es/6f7799768413.0.sd.h264 es/7650ae466748.0.sd.h264 es/8ae90cadb5bc.0.sd.h264 es/dcb262cf0138.0.sd.h264 es/ec473c99c538.0.sd.h264 es/f9e973f44570.0.sd.h264 > out/g.sd.log 2>&1
  echo "g.sd rc=$? $(date -u +%T)" >> out/progress.txt
  if [ -f out/g.sd.json ] && python3 gate1t.py check "after sd #$attempt"; then touch out/g.sd.gate_ok; break; fi
  [ -f out/g.sd.json ] && mv out/g.sd.json out/g.sd.json.void$attempt
  [ -f out/g.sd.log ] && mv out/g.sd.log out/g.sd.log.void$attempt
done
for attempt in 1 2 3; do
  ok=0; for i in 1 2 3 4 5; do python3 gate1t.py check "before 720p #$attempt" && { ok=1; break; }; [ $i -lt 5 ] && sleep ${CHECK_SLEEP:-60}; done
  if [ $ok = 0 ]; then echo "g.720p attempt $attempt: the host failed its gate, no call" >> out/progress.txt; continue; fi
  echo "g.720p attempt $attempt $(date -u +%T)" >> out/progress.txt
  python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 900 --pin 2 --rounds 6 --shuffle-seed 20261031 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --null-bar 0.04 --spread-bar 1.5 --null-arm base --require-cpu 8581C --report out/g.720p.json es/1d4b1775d78a.0.720p.h264 es/278dbf5a63bb.0.720p.h264 es/36501d448eaa.0.720p.h264 es/3c593b8f0809.0.720p.h264 es/65ec61f95a00.0.720p.h264 es/6b18d9f0f88d.0.720p.h264 es/76e0b1692494.0.720p.h264 es/9db41aa16b5a.0.720p.h264 > out/g.720p.log 2>&1
  echo "g.720p rc=$? $(date -u +%T)" >> out/progress.txt
  if [ -f out/g.720p.json ] && python3 gate1t.py check "after 720p #$attempt"; then touch out/g.720p.gate_ok; break; fi
  [ -f out/g.720p.json ] && mv out/g.720p.json out/g.720p.json.void$attempt
  [ -f out/g.720p.log ] && mv out/g.720p.log out/g.720p.log.void$attempt
done
for attempt in 1 2 3; do
  ok=0; for i in 1 2 3 4 5; do python3 gate1t.py check "before 1080p #$attempt" && { ok=1; break; }; [ $i -lt 5 ] && sleep ${CHECK_SLEEP:-60}; done
  if [ $ok = 0 ]; then echo "g.1080p attempt $attempt: the host failed its gate, no call" >> out/progress.txt; continue; fi
  echo "g.1080p attempt $attempt $(date -u +%T)" >> out/progress.txt
  python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 300 --pin 2 --rounds 6 --shuffle-seed 20261031 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --null-bar 0.04 --spread-bar 1.5 --null-arm base --require-cpu 8581C --report out/g.1080p.json es/175433794e02.0.1080p.h264 es/6da34958296c.0.1080p.h264 es/6e90422a670a.0.1080p.h264 es/a6af290d9053.0.1080p.h264 es/a81de82106be.0.1080p.h264 es/b41f70a7fa60.0.1080p.h264 es/b71b6b31e54f.0.1080p.h264 es/c7ecda16e4a7.0.1080p.h264 > out/g.1080p.log 2>&1
  echo "g.1080p rc=$? $(date -u +%T)" >> out/progress.txt
  if [ -f out/g.1080p.json ] && python3 gate1t.py check "after 1080p #$attempt"; then touch out/g.1080p.gate_ok; break; fi
  [ -f out/g.1080p.json ] && mv out/g.1080p.json out/g.1080p.json.void$attempt
  [ -f out/g.1080p.log ] && mv out/g.1080p.log out/g.1080p.log.void$attempt
done
for attempt in 1 2 3; do
  ok=0; for i in 1 2 3 4 5; do python3 gate1t.py check "before 4k #$attempt" && { ok=1; break; }; [ $i -lt 5 ] && sleep ${CHECK_SLEEP:-60}; done
  if [ $ok = 0 ]; then echo "g.4k attempt $attempt: the host failed its gate, no call" >> out/progress.txt; continue; fi
  echo "g.4k attempt $attempt $(date -u +%T)" >> out/progress.txt
  python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 150 --pin 2 --rounds 6 --shuffle-seed 20261031 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --null-bar 0.04 --spread-bar 1.5 --null-arm base --require-cpu 8581C --report out/g.4k.json es/3b1b441e23ae.0.4k.h264 es/4b54de28c2b3.0.4k.h264 es/81ed4778ece2.0.4k.h264 es/ae16bb7c7192.0.4k.h264 es/af38b0cfd840.0.4k.h264 es/e624cbe0d2be.0.4k.h264 es/ed75c044e944.0.4k.h264 es/f8d92a911e72.0.4k.h264 > out/g.4k.log 2>&1
  echo "g.4k rc=$? $(date -u +%T)" >> out/progress.txt
  if [ -f out/g.4k.json ] && python3 gate1t.py check "after 4k #$attempt"; then touch out/g.4k.gate_ok; break; fi
  [ -f out/g.4k.json ] && mv out/g.4k.json out/g.4k.json.void$attempt
  [ -f out/g.4k.log ] && mv out/g.4k.log out/g.4k.log.void$attempt
done
echo "end $(date -u +%T)" >> out/progress.txt
