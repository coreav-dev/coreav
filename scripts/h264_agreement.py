#!/usr/bin/env python3
"""Compare the decoder with FFmpeg frame for frame on real streams.

Bar C2 of docs/design/h264.md: on the corpus, every stream that agreed with
FFmpeg in N1 still agrees. Both sides decode the same Annex B elementary
stream; FFmpeg reports a per-frame MD5 (`-f framemd5`) and this script
hashes the frames the decoder writes, so nothing but the decoded samples is
compared.

    python3 -I scripts/h264_agreement.py --ffmpeg ffmpeg \\
        --decoder target/release/examples/decode --frames 100 \\
        [--report agreement.json] STREAM.264 ...

A stream agrees when the two sides produce the same number of frames and
every frame hashes the same. The report names the first frame that differs.
"""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import time


def ffmpeg_frames(ffmpeg, stream, frames):
    """(size, md5) of each frame FFmpeg decodes."""
    cmd = [ffmpeg, "-v", "error", "-threads", "1", "-i", stream]
    if frames:
        cmd += ["-frames:v", str(frames)]
    cmd += ["-fps_mode", "passthrough", "-f", "framemd5", "-"]
    run = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if run.returncode != 0:
        raise RuntimeError(f"ffmpeg: {run.stderr.strip().splitlines()[-1:]}")
    out = []
    for line in run.stdout.splitlines():
        if line.startswith("#") or not line.strip():
            continue
        parts = [p.strip() for p in line.split(",")]
        if len(parts) < 6:
            continue
        out.append((int(parts[4]), parts[5]))
    return out


def our_frames(decoder, stream, frames, sizes):
    """The MD5 of each frame the decoder writes, reading `sizes[i]` bytes."""
    cmd = [decoder]
    if frames:
        cmd += ["--frames", str(frames)]
    cmd += [stream]
    with subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE) as p:
        out = []
        for size in sizes:
            data = p.stdout.read(size)
            if len(data) < size:
                break
            out.append(hashlib.md5(data).hexdigest())
        rest = p.stdout.read()
        p.wait()
        return out, len(rest), p.stderr.read().decode(errors="replace"), p.returncode


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ffmpeg", default="ffmpeg")
    ap.add_argument("--decoder", default="target/release/examples/decode")
    ap.add_argument("--frames", type=int, default=0, help="0 decodes everything")
    ap.add_argument("--report")
    ap.add_argument("streams", nargs="+")
    args = ap.parse_args()

    results = []
    agree = 0
    started = time.time()
    for stream in args.streams:
        name = os.path.basename(stream)
        try:
            reference = ffmpeg_frames(args.ffmpeg, stream, args.frames)
        except RuntimeError as e:
            results.append({"stream": name, "agrees": False, "why": str(e)})
            print(f"FAIL {name}: {e}")
            continue
        sizes = [s for s, _ in reference]
        hashes, extra, errors, status = our_frames(args.decoder, stream, args.frames, sizes)
        why = None
        if status:
            why = f"the decoder exited with status {status}"
        elif len(hashes) != len(reference):
            why = f"{len(hashes)} frames, FFmpeg made {len(reference)}"
        elif extra:
            why = f"{extra} bytes past the last frame FFmpeg made"
        else:
            for i, ((_, want), got) in enumerate(zip(reference, hashes)):
                if want != got:
                    why = f"frame {i} differs"
                    break
        results.append(
            {
                "stream": name,
                "frames": len(hashes),
                "agrees": why is None,
                "why": why,
                "errors": errors.strip().splitlines()[:3],
            }
        )
        if why is None:
            agree += 1
            print(f"agree {name} ({len(hashes)} frames)")
        else:
            print(f"FAIL  {name}: {why}")
        sys.stdout.flush()

    summary = {
        "streams": len(results),
        "agree": agree,
        "differ": len(results) - agree,
        "seconds": round(time.time() - started, 1),
        "results": results,
    }
    print(json.dumps({k: v for k, v in summary.items() if k != "results"}))
    if args.report:
        with open(args.report, "w") as f:
            json.dump(summary, f, indent=1)
    raise SystemExit(1 if summary["differ"] else 0)


if __name__ == "__main__":
    main()
