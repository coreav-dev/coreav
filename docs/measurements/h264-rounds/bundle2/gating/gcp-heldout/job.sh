set -u
mkdir -p out
chmod +x decode-* ffmpeg
{ lscpu; nproc; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; sha256sum decode-* ffmpeg h264_parity.py gate.h264 es/*; } > out/host.txt 2>&1
ref() {
python3 - "$1" <<'PY'
import argparse, json, sys
sys.path.insert(0, ".")
import h264_parity as h
ns = argparse.Namespace(ffmpeg="./ffmpeg", gate_stream="gate.h264", gate_frames=900, gate_threads=8, gate_pin=None, prefix=[], min_seconds=3)
median, repeat = h.gate_sample(ns)
try:
    d = json.load(open("out/ref.json"))
except OSError:
    d = {}
d[sys.argv[1]] = {"median": median, "repeat": repeat}
json.dump(d, open("out/ref.json", "w"), indent=1)
PY
}
ref before
echo "t8.sd $(date -u +%T)" >> out/progress.txt
python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 1800 --threads 8 --rounds 5 --min-seconds 3 --shuffle-seed 20261026 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --gate-threads 8 --null-bar 0.02 --spread-bar 1.25 --null-arm base --require-cpu 8581C --report out/t8.sd.json es/176aa2655e6b.0.sd.h264 es/280136b1ab5c.0.sd.h264 es/2eb7edc04e08.0.sd.h264 es/35757bdf512b.0.sd.h264 es/4a28c14fa0b2.0.sd.h264 es/4e2642615842.0.sd.h264 es/4f2715fe7626.0.sd.h264 es/75cf7bf8391b.0.sd.h264 es/770db99b2698.0.sd.h264 es/7a580ff996e1.0.sd.h264 es/9b3a84c821e8.0.sd.h264 es/9b833fff929e.0.sd.h264 es/9d99b565f2ec.0.sd.h264 es/d3d3bdb17b38.0.sd.h264 es/d56594fb1f83.0.sd.h264 es/ff84167e699d.0.sd.h264 > out/t8.sd.log 2>&1
echo "t8.sd rc=$?" >> out/progress.txt
echo "t8.720p $(date -u +%T)" >> out/progress.txt
python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 900 --threads 8 --rounds 5 --min-seconds 3 --shuffle-seed 20261026 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --gate-threads 8 --null-bar 0.02 --spread-bar 1.25 --null-arm base --require-cpu 8581C --report out/t8.720p.json es/178de4a3a5a2.0.720p.h264 es/391ec79e178a.0.720p.h264 es/3c331ca6a971.0.720p.h264 es/4dc155fa1f76.0.720p.h264 es/4f27662a0f72.0.720p.h264 es/59cf223b15f2.0.720p.h264 es/64e286230fca.0.720p.h264 es/64ef5412ed81.0.720p.h264 es/67138964d796.0.720p.h264 es/7eb29ce39b5c.0.720p.h264 es/985a28c5abe5.0.720p.h264 es/a9b1e81dc8f4.0.720p.h264 es/b1c6f2742fff.0.720p.h264 es/c187706da87d.0.720p.h264 es/f2a05e5926d8.0.720p.h264 es/f99007f006ea.0.720p.h264 > out/t8.720p.log 2>&1
echo "t8.720p rc=$?" >> out/progress.txt
echo "t8.1080p $(date -u +%T)" >> out/progress.txt
python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 300 --threads 8 --rounds 5 --min-seconds 3 --shuffle-seed 20261026 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --gate-threads 8 --null-bar 0.02 --spread-bar 1.25 --null-arm base --require-cpu 8581C --report out/t8.1080p.json es/334602b1b17e.0.1080p.h264 es/473ec24b05eb.0.1080p.h264 es/48640c5a3dd7.0.1080p.h264 es/4d9717b4616b.0.1080p.h264 es/5f9c31932296.0.1080p.h264 es/8433b87853b2.0.1080p.h264 es/8754f59f19d5.0.1080p.h264 es/ac8f060d2280.0.1080p.h264 es/cfedf7425184.0.1080p.h264 es/d0f1969d25de.0.1080p.h264 es/d4d0cacfe7ff.0.1080p.h264 es/e137204903ad.0.1080p.h264 es/e3595ddcba67.0.1080p.h264 es/ea3b4d48a931.0.1080p.h264 es/f487a36e1311.0.1080p.h264 es/f6988cf7c878.0.1080p.h264 > out/t8.1080p.log 2>&1
echo "t8.1080p rc=$?" >> out/progress.txt
echo "t8.4k $(date -u +%T)" >> out/progress.txt
python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 150 --threads 8 --rounds 5 --min-seconds 3 --shuffle-seed 20261026 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --gate-threads 8 --null-bar 0.02 --spread-bar 1.25 --null-arm base --require-cpu 8581C --report out/t8.4k.json es/37fe6d70a7e9.0.4k.h264 es/4762d0d77ce6.0.4k.h264 es/5411a2f76f31.0.4k.h264 es/5d2107684f26.0.4k.h264 es/64a8a139a3fd.0.4k.h264 es/6b7d211adc0d.0.4k.h264 es/71e12a58ccdd.0.4k.h264 es/a0b54eb67683.0.4k.h264 es/aa0205e7e850.0.4k.h264 es/b716b0c80a63.0.4k.h264 es/b844246b511b.0.4k.h264 es/c22aa0507424.0.4k.h264 es/c2c99564b931.0.4k.h264 es/d67bb9087d1a.0.4k.h264 es/eb872a518732.0.4k.h264 es/ee0d6661e357.0.4k.h264 > out/t8.4k.log 2>&1
echo "t8.4k rc=$?" >> out/progress.txt
ref after
echo "end $(date -u +%T)" >> out/progress.txt
