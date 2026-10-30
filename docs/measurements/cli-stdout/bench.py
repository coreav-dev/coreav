#!/usr/bin/env python3
"""Media on standard output: `coreav remux IN -` before and after the
fix, against the same command writing a file and against FFmpeg 9.0.2
writing MP4 to `pipe:1`. The method and the bars are in
docs/design/cli-stdout.md; this script only runs them.

    bench.py --base BIN --new BIN --ffmpeg FF --ffprobe FP --corpus DIR \
             --ram DIR --out DIR [--rounds 21] [--gate-gap 60]

Writes DIR/inputs.json, DIR/times.json (every timing, voided rounds
included), DIR/gate.json and DIR/summary.md.
"""
import argparse
import hashlib
import json
import os
import pathlib
import statistics
import subprocess
import time

TARGETS = [1e6, 4e6, 12e6, 30e6, None]  # bytes; None is the largest
PRODUCER_CPUS = {2, 3, 4, 5}
CONSUMER_CPUS = {6}
GATE_CPUS = {2}
GATE_TOLERANCE = 0.05
MAX_VOIDS = 3


def pinned(cpus):
    return lambda: os.sched_setaffinity(0, cpus)


def arms(a, src, ram):
    out = str(ram / "out.mp4")
    ff = [a.ffmpeg, "-nostdin", "-v", "error", "-i", str(src), "-map", "0", "-c", "copy",
          "-f", "mp4", "-movflags", "frag_keyframe+empty_moov", "pipe:1"]
    return {
        "base-pipe": ([a.base, "remux", str(src), "-"], None),
        "base-file": ([a.base, "remux", "--force", str(src), out], out),
        "new-pipe": ([a.new, "remux", str(src), "-"], None),
        "new-file": ([a.new, "remux", "--force", str(src), out], out),
        "ffmpeg-pipe": (ff, None),
    }


def run_once(cmd, out, capture=False):
    """Wall time of the command, its stdout piped through `cat` to
    /dev/null (or captured), or of the command alone when it writes a file.
    `cat` starts before the clock, so its own start-up is not timed: the
    time runs from starting the command until `cat` has read the last byte
    and exited.

    Every timed wait blocks in `waitpid`. A wait with a timeout polls,
    sleeping 1, 2, 4 ... up to 50 ms between polls, which
    rounded run-01 to run-03's times up to the next poll. The hard timeout
    is the one around the whole script."""
    if out is not None:
        t0 = time.perf_counter()
        p = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           preexec_fn=pinned(PRODUCER_CPUS))
        t = time.perf_counter() - t0
        code = p.returncode
        data = pathlib.Path(out).read_bytes() if capture else None
        pathlib.Path(out).unlink(missing_ok=True)
        return t, code, data
    if capture:
        t0 = time.perf_counter()
        prod = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                preexec_fn=pinned(PRODUCER_CPUS))
        data, _ = prod.communicate(timeout=600)
        return time.perf_counter() - t0, prod.returncode, data
    read_end, write_end = os.pipe()
    cat = subprocess.Popen(["cat"], stdin=read_end, stdout=subprocess.DEVNULL,
                           preexec_fn=pinned(CONSUMER_CPUS))
    os.close(read_end)
    t0 = time.perf_counter()
    prod = subprocess.Popen(cmd, stdout=write_end, stderr=subprocess.DEVNULL,
                            preexec_fn=pinned(PRODUCER_CPUS))
    os.close(write_end)
    code = prod.wait()
    cat.wait()
    return time.perf_counter() - t0, code, None


def gate_time(a, ref):
    ts = []
    for _ in range(3):
        t0 = time.perf_counter()
        subprocess.run([a.ffmpeg, "-nostdin", "-v", "error", "-threads", "1", "-i", str(ref),
                        "-an", "-frames:v", "300", "-f", "null", "-"], check=True, preexec_fn=pinned(GATE_CPUS),
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        ts.append(time.perf_counter() - t0)
    return statistics.median(ts)


def streams(a, path):
    p = subprocess.run([a.ffprobe, "-v", "error", "-show_entries", "stream=codec_type",
                        "-of", "csv=p=0", str(path)], capture_output=True, text=True)
    return sorted(p.stdout.split()) if p.returncode == 0 else None


def keeps_every_stream(a, path):
    """CoreAV's remux exits 0 when it drops a stream (a codec it does not
    allow), while FFmpeg's `-map 0` copies it: such an input is not the same
    operation on both sides. Added after run-01 (docs/design/cli-stdout.md)."""
    p = subprocess.run([a.new, "remux", str(path), "-"], stdout=subprocess.DEVNULL,
                       stderr=subprocess.PIPE, text=True, timeout=600)
    return p.returncode == 0 and not json.loads(p.stderr).get("dropped_streams")


def select(a, ram):
    """One video and one audio stream; both tools remux it, CoreAV
    dropping no stream; the file
    closest in size to each target, and the largest."""
    files = sorted(p for p in pathlib.Path(a.corpus).iterdir() if p.is_file())
    files.sort(key=lambda p: p.stat().st_size)
    chosen, refused = [], []
    for target in TARGETS:
        if target is None:
            order = sorted(files, key=lambda p: -p.stat().st_size)
        else:
            order = sorted(files, key=lambda p: (abs(p.stat().st_size - target), p.name))
        for p in order:
            if p in chosen:
                continue
            if streams(a, p) != ["audio", "video"]:
                continue
            ok = all(run_once(cmd, out)[1] == 0
                     for name, (cmd, out) in arms(a, p, ram).items()
                     if name in ("new-file", "ffmpeg-pipe")) and keeps_every_stream(a, p)
            if ok:
                chosen.append(p)
                break
            refused.append(p.name)
    return chosen, refused


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    ap = argparse.ArgumentParser()
    for name in ("base", "new", "ffmpeg", "ffprobe", "corpus"):
        ap.add_argument("--" + name, required=True)
    ap.add_argument("--ram", required=True, type=pathlib.Path)
    ap.add_argument("--out", required=True, type=pathlib.Path)
    ap.add_argument("--rounds", type=int, default=21)
    ap.add_argument("--gate-gap", type=float, default=60, help="seconds between gate tries; 0 only for smoke tests")
    a = ap.parse_args()
    a.out.mkdir(parents=True, exist_ok=True)

    chosen, refused = select(a, a.ram)
    # Inputs are read from the RAM disk, as the outputs are written there.
    inputs = []
    for p in chosen:
        dst = a.ram / p.name
        dst.write_bytes(p.read_bytes())
        inputs.append(dst)
    # Identity: the pipe carries the file's bytes, before and after.
    identity = {}
    for src in inputs:
        got = {name: run_once(cmd, out, capture=True)
               for name, (cmd, out) in arms(a, src, a.ram).items()}
        hashes = {name: sha(d) for name, (_, code, d) in got.items()}
        assert all(code == 0 for _, code, _ in got.values()), src
        assert hashes["base-pipe"] == hashes["new-pipe"] == hashes["new-file"] == hashes["base-file"], src
        identity[src.name] = {"sha256": hashes["new-pipe"], "bytes": len(got["new-pipe"][2]),
                              "ffmpeg_bytes": len(got["ffmpeg-pipe"][2])}
    (a.out / "inputs.json").write_text(json.dumps(
        {"binaries": {k: sha(pathlib.Path(getattr(a, k)).read_bytes()) for k in ("base", "new", "ffmpeg")},
         "inputs": [{"name": p.name, "size": p.stat().st_size, **identity[p.name]} for p in inputs],
         "refused": refused}, indent=1))

    # The gate's reference: three measurements a minute apart that agree
    # within the tolerance (docs/design/cli-stdout.md).
    ref = inputs[len(inputs) // 2]
    refs = []
    for i in range(3):
        if i:
            time.sleep(a.gate_gap)
        refs.append(gate_time(a, ref))
    gate_ref = statistics.median(refs)
    gate = {"input": ref.name, "reference": refs, "checks": []}
    if max(abs(r / gate_ref - 1) for r in refs) > GATE_TOLERANCE:
        gate["verdict"] = "host unstable at start"
        (a.out / "gate.json").write_text(json.dumps(gate, indent=1))
        raise SystemExit("host unstable at start")

    def healthy(when, rnd):
        t = gate_time(a, ref)
        ok = abs(t / gate_ref - 1) <= GATE_TOLERANCE
        gate["checks"].append({"round": rnd, "when": when, "time": t, "ok": ok})
        return ok

    names = list(arms(a, inputs[0], a.ram))
    for src in inputs:  # warm-up
        for name, (cmd, out) in arms(a, src, a.ram).items():
            run_once(cmd, out)
    times = []
    rnd = 0
    while rnd < a.rounds:
        voids = 0
        while True:
            while not healthy("before", rnd):
                time.sleep(a.gate_gap)
            rows = []
            for src in inputs:
                order = names[rnd % len(names):] + names[:rnd % len(names)]
                for name in order:
                    cmd, out = arms(a, src, a.ram)[name]
                    t, code, _ = run_once(cmd, out)
                    rows.append({"round": rnd, "input": src.name, "arm": name, "time": t, "code": code})
            ok = healthy("after", rnd)
            for r in rows:
                r["void"] = not ok
            times.extend(rows)
            if ok:
                break
            voids += 1
            if voids >= MAX_VOIDS:
                gate["verdict"] = f"round {rnd} void {voids} times"
                (a.out / "gate.json").write_text(json.dumps(gate, indent=1))
                (a.out / "times.json").write_text(json.dumps(times, indent=0))
                raise SystemExit(gate["verdict"])
        rnd += 1
    gate["verdict"] = "passed"
    (a.out / "gate.json").write_text(json.dumps(gate, indent=1))
    (a.out / "times.json").write_text(json.dumps(times, indent=0))


if __name__ == "__main__":
    main()
