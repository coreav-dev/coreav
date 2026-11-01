#!/usr/bin/env python3
"""Decode the same H.264 streams in the decoder's two modes and compare the output.

The default build has the feature `fast` and runs the decoder's x86-64-v3 instance of its hot paths where the CPU has
the features (docs/design/h264.md, "Kernel round: the hot paths compiled twice"); the safe build, without `fast`,
compiles no `unsafe` of its own and runs the baseline instance. Each stream is decoded by both builds' `decode` example
at each thread count, and every decode's output (the MD5 of its raw frames) and exit status must be the same.

    python3 -I scripts/h264_modes_identity.py --streams DIR [--threads 1,8] [--report report.json]
    python3 -I scripts/h264_modes_identity.py --generate FFMPEG --streams DIR [...]

`--streams DIR` decodes every `*.h264` file in DIR whole. `--generate FFMPEG` first writes a synthetic set into DIR,
encoded with the given FFmpeg binary (a black-box tool, the pinned oracle of scripts/install-oracle-ffmpeg.sh, with
libx264): the coding tools the two instances differ in (CABAC residual blocks, motion compensation) under several
settings each, since the training set itself is not available everywhere this runs, and monochrome streams
(`gray-*.h264`) with the same tools.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time

# (name, size, frames, extra filters, x264 parameters): CABAC and CAVLC, P and B slices, both direct modes,
# explicit and implicit weighted prediction, 8x8 transforms, several slices a picture, a cropped height, and
# quantizers from very low (large levels, the Exp-Golomb suffix) to high (few coefficients).
SYNTHETIC = [
    ("cabac-b-weightp", "1280x720", 48, "", "cabac=1:bframes=3:b-pyramid=normal:weightp=2:8x8dct=1:ref=4:qp=24"),
    ("cabac-fade", "640x360", 60, ",fade=type=in:start_frame=0:nb_frames=40", "cabac=1:bframes=2:weightp=2:weightb=1:qp=26"),
    ("cavlc-b", "640x360", 60, "", "cabac=0:bframes=2:weightb=1:8x8dct=0:ref=3:qp=26"),
    ("cavlc-p-slices", "640x360", 60, "", "cabac=0:bframes=0:slices=4:qp=28"),
    ("temporal-direct-crop", "960x540", 40, "", "direct=temporal:bframes=2:weightp=1:qp=22"),
    ("intra-8x8", "640x360", 20, "", "keyint=1:8x8dct=1:qp=20"),
    ("intra-4x4", "640x360", 20, "", "keyint=1:8x8dct=0:qp=30"),
    ("partitions", "640x480", 40, "", "partitions=all:subme=9:me=umh:direct=spatial:bframes=3:qp=30"),
    ("low-qp-slices", "320x240", 30, "", "qp=2:slices=3:bframes=1"),
    ("high-qp-1080p", "1920x1080", 16, "", "qp=45:bframes=2"),
]

# Monochrome (4:0:0) streams, the same entries' kind with `-pix_fmt gray`: CABAC and CAVLC, B slices with weighted and
# temporal direct prediction, intra only, several slices a picture, a cropped height (docs/design/h264.md,
# "Monochrome"). CI also compares their frames with the pinned FFmpeg's (scripts/h264_agreement.py).
MONOCHROME = [
    ("gray-cabac-b", "640x360", 60, "", "cabac=1:bframes=3:b-pyramid=normal:weightp=2:8x8dct=1:ref=4:qp=24"),
    ("gray-cavlc-b", "640x360", 60, "", "cabac=0:bframes=2:weightb=1:8x8dct=0:ref=3:qp=26"),
    ("gray-cavlc-slices", "640x360", 60, "", "cabac=0:bframes=0:slices=4:qp=28"),
    ("gray-intra", "640x360", 20, "", "keyint=1:8x8dct=1:qp=20"),
    ("gray-crop-temporal", "960x540", 40, "", "direct=temporal:bframes=2:weightp=1:qp=22"),
    ("gray-lowqp", "320x240", 30, "", "qp=2:slices=3:bframes=1"),
]


def chroma_format_idc(path):
    """chroma_format_idc of the first sequence parameter set in an Annex B stream (clause 7.3.2.1.1)."""
    data = open(path, "rb").read()
    start = data.find(b"\x00\x00\x01")
    while start >= 0 and data[start + 3] & 0x1F != 7:
        start = data.find(b"\x00\x00\x01", start + 3)
    if start < 0:
        raise SystemExit(f"{path}: no sequence parameter set")
    end = data.find(b"\x00\x00\x01", start + 3)
    rbsp = data[start + 4:end if end >= 0 else len(data)].replace(b"\x00\x00\x03", b"\x00\x00")
    bits = "".join(f"{b:08b}" for b in rbsp)
    pos = 24

    def ue():
        nonlocal pos
        zeros = bits.index("1", pos) - pos
        value = int(bits[pos + zeros:pos + 2 * zeros + 1], 2) - 1
        pos += 2 * zeros + 1
        return value

    ue()
    return ue() if rbsp[0] in (100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135) else 1


def generate(ffmpeg, out):
    os.makedirs(out, exist_ok=True)
    for pix_fmt, streams in (("yuv420p", SYNTHETIC), ("gray", MONOCHROME)):
        for name, size, frames, filters, params in streams:
            dst = os.path.join(out, f"{name}.h264")
            subprocess.run(
                [ffmpeg, "-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i", f"testsrc2=size={size}:rate=30",
                 "-vf", f"noise=alls=12:allf=t+u{filters}", "-frames:v", str(frames), "-pix_fmt", pix_fmt,
                 "-c:v", "libx264", "-preset", "medium", "-x264-params", params, "-f", "h264", dst],
                check=True,
            )
            # FFmpeg falls back to a format libx264 takes, with only a warning, when it does not take the one asked for.
            if (chroma_format_idc(dst) == 0) != (pix_fmt == "gray"):
                raise SystemExit(f"{dst}: chroma_format_idc {chroma_format_idc(dst)} for -pix_fmt {pix_fmt}")


def build(features, target):
    cmd = ["cargo", "build", "--release", "--locked", "--example", "decode", "-p", "coreav-h264",
           "--target-dir", target, *features]
    subprocess.run(cmd, check=True)
    return os.path.join(target, "release", "examples", "decode")


def decode(binary, stream, threads):
    """The MD5 of the decoded frames, read as they are written: a training stream's are a gigabyte."""
    md5, size = hashlib.md5(), 0
    with subprocess.Popen([binary, "--threads", str(threads), stream], stdout=subprocess.PIPE,
                          stderr=subprocess.DEVNULL) as p:
        while chunk := p.stdout.read(1 << 20):
            md5.update(chunk)
            size += len(chunk)
    return {"md5": md5.hexdigest(), "status": p.returncode, "bytes": size}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--streams", required=True)
    ap.add_argument("--generate", metavar="FFMPEG")
    ap.add_argument("--threads", default="1,8")
    ap.add_argument("--target-dir", default="target/modes-identity")
    ap.add_argument("--report")
    args = ap.parse_args()
    if args.generate:
        generate(args.generate, args.streams)
    streams = sorted(f for f in os.listdir(args.streams) if f.endswith(".h264"))
    if not streams:
        raise SystemExit("no streams")
    threads = [int(t) for t in args.threads.split(",")]
    builds = {
        "fast": build([], os.path.join(args.target_dir, "fast")),
        "safe": build(["--no-default-features"], os.path.join(args.target_dir, "safe")),
    }
    cpu = open("/proc/cpuinfo").read() if os.path.exists("/proc/cpuinfo") else ""
    flags = next((l.split(":", 1)[1].split() for l in cpu.splitlines() if l.startswith("flags")), [])
    v3 = all(f in flags for f in ("avx", "avx2", "bmi1", "bmi2", "f16c", "fma", "abm", "movbe", "popcnt",
                                 "pni", "ssse3", "sse4_1", "sse4_2"))
    started = time.time()
    results, different = [], 0
    for name in streams:
        path = os.path.join(args.streams, name)
        for t in threads:
            got = {b: decode(binary, path, t) for b, binary in builds.items()}
            same = len({(g["md5"], g["status"]) for g in got.values()}) == 1
            different += not same
            results.append({"stream": name, "threads": t, "same": same, **got})
            print(("same " if same else "DIFFERENT ") + f"{name} threads {t} "
                  + " ".join(f"{b} {g['md5'][:12]} ({g['status']}, {g['bytes']} bytes)" for b, g in got.items()))
            sys.stdout.flush()
    summary = {"streams": len(streams), "threads": threads, "decodes_compared": len(results),
               "different": different, "cpu_has_v3": v3, "seconds": round(time.time() - started, 1),
               "results": results}
    print(json.dumps({k: v for k, v in summary.items() if k != "results"}))
    if args.report:
        with open(args.report, "w") as f:
            json.dump(summary, f, indent=1)
    step_summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if step_summary:
        with open(step_summary, "a") as f:
            f.write(f"### H.264 modes: {len(results) - different} of {len(results)} decodes identical in both modes "
                    f"({len(streams)} streams, threads {args.threads}; the default build ran its "
                    f"{'x86-64-v3' if v3 else 'baseline'} instance)\n")
            for r in results:
                if not r["same"]:
                    f.write(f"- DIFFERENT {r['stream']} at {r['threads']} threads\n")
    raise SystemExit(1 if different else 0)


if __name__ == "__main__":
    main()
