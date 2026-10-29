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
  python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 1800 --pin 2 --rounds 5 --shuffle-seed 20261029 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --null-bar 0.04 --spread-bar 1.5 --null-arm base --require-cpu 8581C --report out/g.sd.json es/176aa2655e6b.0.sd.h264 es/280136b1ab5c.0.sd.h264 es/2eb7edc04e08.0.sd.h264 es/35757bdf512b.0.sd.h264 es/4a28c14fa0b2.0.sd.h264 es/4e2642615842.0.sd.h264 es/4f2715fe7626.0.sd.h264 es/75cf7bf8391b.0.sd.h264 es/770db99b2698.0.sd.h264 es/7a580ff996e1.0.sd.h264 es/9b3a84c821e8.0.sd.h264 es/9b833fff929e.0.sd.h264 es/9d99b565f2ec.0.sd.h264 es/d3d3bdb17b38.0.sd.h264 es/d56594fb1f83.0.sd.h264 es/ff84167e699d.0.sd.h264 > out/g.sd.log 2>&1
  echo "g.sd rc=$? $(date -u +%T)" >> out/progress.txt
  if [ -f out/g.sd.json ] && python3 gate1t.py check "after sd #$attempt"; then touch out/g.sd.gate_ok; break; fi
  [ -f out/g.sd.json ] && mv out/g.sd.json out/g.sd.json.void$attempt
  [ -f out/g.sd.log ] && mv out/g.sd.log out/g.sd.log.void$attempt
done
for attempt in 1 2 3; do
  ok=0; for i in 1 2 3 4 5; do python3 gate1t.py check "before 720p #$attempt" && { ok=1; break; }; [ $i -lt 5 ] && sleep ${CHECK_SLEEP:-60}; done
  if [ $ok = 0 ]; then echo "g.720p attempt $attempt: the host failed its gate, no call" >> out/progress.txt; continue; fi
  echo "g.720p attempt $attempt $(date -u +%T)" >> out/progress.txt
  python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 900 --pin 2 --rounds 5 --shuffle-seed 20261029 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --null-bar 0.04 --spread-bar 1.5 --null-arm base --require-cpu 8581C --report out/g.720p.json es/178de4a3a5a2.0.720p.h264 es/391ec79e178a.0.720p.h264 es/3c331ca6a971.0.720p.h264 es/4dc155fa1f76.0.720p.h264 es/4f27662a0f72.0.720p.h264 es/59cf223b15f2.0.720p.h264 es/64e286230fca.0.720p.h264 es/64ef5412ed81.0.720p.h264 es/67138964d796.0.720p.h264 es/7eb29ce39b5c.0.720p.h264 es/985a28c5abe5.0.720p.h264 es/a9b1e81dc8f4.0.720p.h264 es/b1c6f2742fff.0.720p.h264 es/c187706da87d.0.720p.h264 es/f2a05e5926d8.0.720p.h264 es/f99007f006ea.0.720p.h264 > out/g.720p.log 2>&1
  echo "g.720p rc=$? $(date -u +%T)" >> out/progress.txt
  if [ -f out/g.720p.json ] && python3 gate1t.py check "after 720p #$attempt"; then touch out/g.720p.gate_ok; break; fi
  [ -f out/g.720p.json ] && mv out/g.720p.json out/g.720p.json.void$attempt
  [ -f out/g.720p.log ] && mv out/g.720p.log out/g.720p.log.void$attempt
done
for attempt in 1 2 3; do
  ok=0; for i in 1 2 3 4 5; do python3 gate1t.py check "before 1080p #$attempt" && { ok=1; break; }; [ $i -lt 5 ] && sleep ${CHECK_SLEEP:-60}; done
  if [ $ok = 0 ]; then echo "g.1080p attempt $attempt: the host failed its gate, no call" >> out/progress.txt; continue; fi
  echo "g.1080p attempt $attempt $(date -u +%T)" >> out/progress.txt
  python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 300 --pin 2 --rounds 5 --shuffle-seed 20261029 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --null-bar 0.04 --spread-bar 1.5 --null-arm base --require-cpu 8581C --report out/g.1080p.json es/334602b1b17e.0.1080p.h264 es/473ec24b05eb.0.1080p.h264 es/48640c5a3dd7.0.1080p.h264 es/4d9717b4616b.0.1080p.h264 es/5f9c31932296.0.1080p.h264 es/8433b87853b2.0.1080p.h264 es/8754f59f19d5.0.1080p.h264 es/ac8f060d2280.0.1080p.h264 es/cfedf7425184.0.1080p.h264 es/d0f1969d25de.0.1080p.h264 es/d4d0cacfe7ff.0.1080p.h264 es/e137204903ad.0.1080p.h264 es/e3595ddcba67.0.1080p.h264 es/ea3b4d48a931.0.1080p.h264 es/f487a36e1311.0.1080p.h264 es/f6988cf7c878.0.1080p.h264 > out/g.1080p.log 2>&1
  echo "g.1080p rc=$? $(date -u +%T)" >> out/progress.txt
  if [ -f out/g.1080p.json ] && python3 gate1t.py check "after 1080p #$attempt"; then touch out/g.1080p.gate_ok; break; fi
  [ -f out/g.1080p.json ] && mv out/g.1080p.json out/g.1080p.json.void$attempt
  [ -f out/g.1080p.log ] && mv out/g.1080p.log out/g.1080p.log.void$attempt
done
for attempt in 1 2 3; do
  ok=0; for i in 1 2 3 4 5; do python3 gate1t.py check "before 4k #$attempt" && { ok=1; break; }; [ $i -lt 5 ] && sleep ${CHECK_SLEEP:-60}; done
  if [ $ok = 0 ]; then echo "g.4k attempt $attempt: the host failed its gate, no call" >> out/progress.txt; continue; fi
  echo "g.4k attempt $attempt $(date -u +%T)" >> out/progress.txt
  python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 150 --pin 2 --rounds 5 --shuffle-seed 20261029 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --null-bar 0.04 --spread-bar 1.5 --null-arm base --require-cpu 8581C --report out/g.4k.json es/37fe6d70a7e9.0.4k.h264 es/4762d0d77ce6.0.4k.h264 es/5411a2f76f31.0.4k.h264 es/5d2107684f26.0.4k.h264 es/64a8a139a3fd.0.4k.h264 es/6b7d211adc0d.0.4k.h264 es/71e12a58ccdd.0.4k.h264 es/a0b54eb67683.0.4k.h264 es/aa0205e7e850.0.4k.h264 es/b716b0c80a63.0.4k.h264 es/b844246b511b.0.4k.h264 es/c22aa0507424.0.4k.h264 es/c2c99564b931.0.4k.h264 es/d67bb9087d1a.0.4k.h264 es/eb872a518732.0.4k.h264 es/ee0d6661e357.0.4k.h264 > out/g.4k.log 2>&1
  echo "g.4k rc=$? $(date -u +%T)" >> out/progress.txt
  if [ -f out/g.4k.json ] && python3 gate1t.py check "after 4k #$attempt"; then touch out/g.4k.gate_ok; break; fi
  [ -f out/g.4k.json ] && mv out/g.4k.json out/g.4k.json.void$attempt
  [ -f out/g.4k.log ] && mv out/g.4k.log out/g.4k.log.void$attempt
done
echo "end $(date -u +%T)" >> out/progress.txt
