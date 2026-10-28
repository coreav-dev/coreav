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
echo "t8.sd $(date -u +%T)" >> out/progress.txt
python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 1800 --threads 8 --rounds 10 --min-seconds 3.0 --shuffle-seed 20261030 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --gate-threads 8 --null-bar 0.02 --spread-bar 1.25 --null-arm base --require-cpu 8581C --report out/t8.sd.json es/41e324ebb708.0.sd.h264 es/5185721dda11.0.sd.h264 es/6f7799768413.0.sd.h264 es/7650ae466748.0.sd.h264 es/8ae90cadb5bc.0.sd.h264 es/dcb262cf0138.0.sd.h264 es/ec473c99c538.0.sd.h264 es/f9e973f44570.0.sd.h264 > out/t8.sd.log 2>&1
echo "t8.sd rc=$?" >> out/progress.txt
echo "t8.720p $(date -u +%T)" >> out/progress.txt
python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 900 --threads 8 --rounds 10 --min-seconds 3.0 --shuffle-seed 20261030 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --gate-threads 8 --null-bar 0.02 --spread-bar 1.25 --null-arm base --require-cpu 8581C --report out/t8.720p.json es/1d4b1775d78a.0.720p.h264 es/278dbf5a63bb.0.720p.h264 es/36501d448eaa.0.720p.h264 es/3c593b8f0809.0.720p.h264 es/65ec61f95a00.0.720p.h264 es/6b18d9f0f88d.0.720p.h264 es/76e0b1692494.0.720p.h264 es/9db41aa16b5a.0.720p.h264 > out/t8.720p.log 2>&1
echo "t8.720p rc=$?" >> out/progress.txt
echo "t8.1080p $(date -u +%T)" >> out/progress.txt
python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 300 --threads 8 --rounds 10 --min-seconds 3.0 --shuffle-seed 20261030 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --gate-threads 8 --null-bar 0.02 --spread-bar 1.25 --null-arm base --require-cpu 8581C --report out/t8.1080p.json es/175433794e02.0.1080p.h264 es/6da34958296c.0.1080p.h264 es/6e90422a670a.0.1080p.h264 es/a6af290d9053.0.1080p.h264 es/a81de82106be.0.1080p.h264 es/b41f70a7fa60.0.1080p.h264 es/b71b6b31e54f.0.1080p.h264 es/c7ecda16e4a7.0.1080p.h264 > out/t8.1080p.log 2>&1
echo "t8.1080p rc=$?" >> out/progress.txt
echo "t8.4k $(date -u +%T)" >> out/progress.txt
python3 h264_parity.py --decoder base=./decode-base --decoder k=./decode-k --ffmpeg ./ffmpeg --frames 150 --threads 8 --rounds 10 --min-seconds 3.0 --shuffle-seed 20261030 --gate-stream gate.h264 --gate-frames 900 --gate-every 4 --gate-threads 8 --null-bar 0.02 --spread-bar 1.25 --null-arm base --require-cpu 8581C --report out/t8.4k.json es/3b1b441e23ae.0.4k.h264 es/4b54de28c2b3.0.4k.h264 es/81ed4778ece2.0.4k.h264 es/ae16bb7c7192.0.4k.h264 es/af38b0cfd840.0.4k.h264 es/e624cbe0d2be.0.4k.h264 es/ed75c044e944.0.4k.h264 es/f8d92a911e72.0.4k.h264 > out/t8.4k.log 2>&1
echo "t8.4k rc=$?" >> out/progress.txt
ref after
echo "end $(date -u +%T)" >> out/progress.txt
