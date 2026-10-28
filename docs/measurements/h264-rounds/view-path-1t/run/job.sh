set -u
mkdir -p out
chmod +x decode-* ffmpeg
{ lscpu; nproc; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; sha256sum decode-* ffmpeg h264_parity.py gate.h264 es/*; } > out/host.txt 2>&1
ref() {
python3 - "$1" <<'PY'
import argparse, json, sys
sys.path.insert(0, ".")
import h264_parity as h
ns = argparse.Namespace(ffmpeg="./ffmpeg", gate_stream="gate.h264", gate_frames=900, gate_threads=8, gate_pin=None, prefix=[], min_seconds=3.0)
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
echo "t1.4k $(date -u +%T)" >> out/progress.txt
python3 h264_parity.py --decoder b2=./decode-b2 --decoder vp2=./decode-vp2 --ffmpeg ./ffmpeg --frames 150 --threads 1 --rounds 10 --min-seconds 3.0 --shuffle-seed 20261027 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --null-bar 0.02 --spread-bar 1.25 --null-arm b2 --pin 3 --require-cpu 8581C --report out/t1.4k.json es/3b1b441e23ae.0.4k.h264 es/4b54de28c2b3.0.4k.h264 es/81ed4778ece2.0.4k.h264 es/ae16bb7c7192.0.4k.h264 es/af38b0cfd840.0.4k.h264 es/e624cbe0d2be.0.4k.h264 es/ed75c044e944.0.4k.h264 es/f8d92a911e72.0.4k.h264 > out/t1.4k.log 2>&1
echo "t1.4k rc=$?" >> out/progress.txt
ref after
echo "end $(date -u +%T)" >> out/progress.txt
