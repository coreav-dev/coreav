#!/usr/bin/env python3
"""Black-box check of FLAC, ALAC, AC-3 and E-AC-3 audio copied into MP4
(docs/design/remux.md, "Audio sample entries") against a pinned FFmpeg.

Generates small sources with the given ffmpeg (lavfi tones and test
pictures, the recipes in SOURCES below), runs every CoreAV command that
copies audio into MP4 on them with --strict-safe, and checks each output
with the given ffprobe and ffmpeg, used as black boxes:

- streams: the output's audio stream has the source's codec, channel count
  and sample rate;
- remux and "coreav ffmpeg -c copy" (MP4, MOV, M4A and Matroska sources):
  every audio packet's size and MD5 equal the source's, in order, at the
  source's presentation time (exactly for MP4 sources, within 1 ms for
  Matroska, which stores milliseconds), and the decoded audio's MD5
  ("ffmpeg -f md5") equals the source's;
- clip: the output's audio packets are a run of the source's, in order;
- split: the segments' audio packets, in order, are the source's;
- join, and "coreav ffmpeg -f concat -c copy", of a file with itself: the
  output's audio packets are the source's twice;
- every output decodes without an error.

Clips, segments and joins change the edit lists, so their packets are
compared with edit lists ignored. The binaries are black boxes; no
implementation source is read. Exits 1 if any check fails.

usage: audio_copy_check.py --coreav PATH [--ffmpeg PATH] [--ffprobe PATH]
                           --work DIR [--report REPORT.json]
"""
import argparse
import json
import pathlib
import shutil
import subprocess
import sys

TIMEOUT = 300
VIDEO = ["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25:duration=2"]
H264 = ["-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "25"]
KEYS = ("codec_name", "channels", "sample_rate")
SIX = ["-af", "pan=5.1|c0=c0|c1=c0|c2=c0|c3=c0|c4=c0|c5=c0"]
STEREO = ["-ac", "2"]


def tone(rate):
    return ["-f", "lavfi", "-i", f"sine=frequency=440:duration=2:sample_rate={rate}"]


CODECS = {
    "flac": ["-c:a", "flac"],
    "alac": ["-c:a", "alac", "-sample_fmt", "s16p"],
    "ac3": ["-c:a", "ac3"],
    "eac3": ["-c:a", "eac3"],
}

# (file name, ffmpeg arguments before the output). Every source is remuxed
# and copied; MP4 sources with video are also clipped, split and joined.
SOURCES = [
    ("flac.mp4", tone(48_000) + STEREO + CODECS["flac"]),
    # 96 kHz does not fit the sample entry's 16-bit rate field.
    ("flac_96k_24.mp4", tone(96_000) + ["-c:a", "flac", "-sample_fmt", "s32"]),
    ("alac.m4a", tone(44_100) + STEREO + CODECS["alac"]),
    ("alac_96k_24.m4a", tone(96_000) + ["-c:a", "alac", "-sample_fmt", "s32p"]),
    ("ac3.mp4", tone(48_000) + STEREO + CODECS["ac3"]),
    ("ac3_51.mov", tone(48_000) + SIX + CODECS["ac3"]),
    ("eac3.mp4", tone(48_000) + STEREO + CODECS["eac3"]),
    ("eac3_51.mp4", tone(48_000) + SIX + CODECS["eac3"]),
    ("flac.mkv", tone(48_000) + STEREO + CODECS["flac"]),
    ("flac_96k_24.mkv", tone(96_000) + ["-c:a", "flac", "-sample_fmt", "s32"]),
    ("alac.mkv", tone(44_100) + STEREO + CODECS["alac"]),
    ("ac3_44k.mkv", tone(44_100) + STEREO + CODECS["ac3"]),
    ("ac3_51.mkv", tone(48_000) + SIX + CODECS["ac3"]),
    ("eac3.mkv", tone(48_000) + STEREO + CODECS["eac3"]),
] + [(f"av_{codec}.{ext}", VIDEO + tone(48_000) + H264 + STEREO + args + ["-shortest"])
     for codec, args in CODECS.items() for ext in ("mp4", "mkv")]

EXPECTED = {
    "run": lambda a, b: bool(b) and any(a[i:i + len(b)] == b for i in range(len(a) - len(b) + 1)),
    "all": lambda a, b: a == b,
    "twice": lambda a, b: a + a == b,
}


def run(args):
    return subprocess.run(args, capture_output=True, timeout=TIMEOUT, check=False)


def ffprobe_json(ffprobe, *args):
    out = run([ffprobe, "-v", "error", "-of", "json", *args])
    if out.returncode != 0:
        raise RuntimeError(out.stderr.decode(errors="replace").strip())
    return json.loads(out.stdout)


def audio_stream(ffprobe, path):
    streams = ffprobe_json(ffprobe, "-select_streams", "a:0", "-show_entries",
                           "stream=" + ",".join(KEYS), str(path))["streams"]
    return {k: streams[0].get(k) for k in KEYS} if streams else None


def packets(ffprobe, path, ignore_edits):
    """(size, MD5, presentation time) of every audio packet, in order."""
    edits = ["-ignore_editlist", "1"] if ignore_edits else []
    found = ffprobe_json(ffprobe, *edits, "-select_streams", "a:0", "-show_packets", "-show_data_hash",
                         "MD5", "-show_entries", "packet=size,data_hash,pts_time", str(path))["packets"]
    return [(int(p["size"]), p["data_hash"], float(p.get("pts_time", "nan"))) for p in found]


def decodes(problems, ffmpeg, path):
    """The decoded audio's MD5; a problem when ffmpeg reports an error."""
    out = run([ffmpeg, "-v", "error", "-i", str(path), "-map", "0:a:0", "-f", "md5", "-"])
    md5, errors = out.stdout.decode().strip(), out.stderr.decode(errors="replace").strip()
    if out.returncode != 0 or errors or not md5:
        problems.append(f"{path.name} does not decode cleanly: {errors[:300]}")
    return md5


def same_stream(problems, ffprobe, source, output):
    a, b = audio_stream(ffprobe, source), audio_stream(ffprobe, output)
    if a != b:
        problems.append(f"audio stream: source {a}, output {b}")


def check_copy(tools, source, output, problems):
    """A whole-file copy: the packets, their times and the decoded audio."""
    same_stream(problems, tools.ffprobe, source, output)
    a, b = packets(tools.ffprobe, source, False), packets(tools.ffprobe, output, False)
    tolerance = 0.001 if source.suffix == ".mkv" else 1e-6
    if [p[:2] for p in a] != [p[:2] for p in b]:
        problems.append(f"packets: {len(a)} in the source, {len(b)} in the output, or other bytes")
    else:
        late = [n for n, (x, y) in enumerate(zip(a, b)) if not abs(x[2] - y[2]) <= tolerance]
        if late:
            n = late[0]
            problems.append(f"{len(late)} packets at other times, the first {n}: {a[n][2]} s, output {b[n][2]} s")
    want, got = decodes(problems, tools.ffmpeg, source), decodes(problems, tools.ffmpeg, output)
    if want != got:
        problems.append(f"decoded audio: source {want}, output {got}")


def check_packets(tools, source, outputs, problems, expected):
    """Outputs whose audio packets, edit lists ignored, must be
    expected(the source's packets, the outputs' packets in order)."""
    for output in outputs:
        same_stream(problems, tools.ffprobe, source, output)
        decodes(problems, tools.ffmpeg, output)
    a = [p[:2] for p in packets(tools.ffprobe, source, True)]
    b = [p[:2] for o in outputs for p in packets(tools.ffprobe, o, True)]
    if not outputs or not expected(a, b):
        problems.append(f"audio packets: {len(b)} in {len(outputs)} outputs, not the expected ones of the "
                        f"source's {len(a)}")


def cases(source, out):
    """(command, coreav arguments, the outputs it writes, kind of check)."""
    stem = source.name
    remux, copy = out / f"{stem}.remux.mp4", out / f"{stem}.copy.mp4"
    yield "remux", ["remux", str(source), str(remux)], lambda: [remux], "copy"
    yield "ffmpeg -c copy", ["ffmpeg", "-v", "error", "-i", str(source), "-c", "copy", str(copy)], \
        lambda: [copy], "copy"
    if not stem.startswith("av_") or source.suffix != ".mp4":
        return
    clip, join, concat = (out / f"{stem}.{kind}.mp4" for kind in ("clip", "join", "concat"))
    yield "clip", ["clip", "--start", "1.2", "--end", "1.9", str(source), str(clip)], lambda: [clip], "run"
    yield "split", ["split", "--every", "1", str(source), str(out / f"{stem}.split-%d.mp4")], \
        lambda: sorted(out.glob(f"{stem}.split-*.mp4"), key=lambda p: int(p.stem.rsplit("-", 1)[1])), "all"
    yield "join", ["join", "--output", str(join), str(source), str(source)], lambda: [join], "twice"
    listing = out / f"{stem}.concat.txt"
    listing.write_text(f"file '{source}'\nfile '{source}'\n")
    yield "ffmpeg -f concat -c copy", ["ffmpeg", "-v", "error", "-f", "concat", "-safe", "0", "-i", str(listing),
                                       "-c", "copy", str(concat)], lambda: [concat], "twice"


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--coreav", required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--work", required=True, type=pathlib.Path)
    parser.add_argument("--report", type=pathlib.Path)
    tools = parser.parse_args()
    sources, out = tools.work / "sources", tools.work / "outputs"
    shutil.rmtree(tools.work, ignore_errors=True)
    sources.mkdir(parents=True)
    out.mkdir()
    results = []
    for name, recipe in SOURCES:
        source = sources / name
        made = run([tools.ffmpeg, "-hide_banner", "-loglevel", "error", "-y", *recipe, str(source)])
        if made.returncode != 0:
            results.append({"source": name, "command": "generate",
                            "problems": [made.stderr.decode(errors="replace")[:500]]})
            continue
        for command, args, outputs, kind in cases(source, out):
            problems = []
            done = run([tools.coreav, "--strict-safe", *args])
            if done.returncode != 0:
                problems.append(f"exit {done.returncode}: {done.stderr.decode(errors='replace').strip()[:500]}")
            elif kind == "copy":
                check_copy(tools, source, outputs()[0], problems)
            else:
                check_packets(tools, source, outputs(), problems, EXPECTED[kind])
            results.append({"source": name, "command": command, "problems": problems})
    for r in results:
        print(f"{'FAIL' if r['problems'] else 'ok  '} {r['command']:24} {r['source']}"
              + "".join(f"\n       {p}" for p in r["problems"]))
    version = run([tools.ffmpeg, "-version"]).stdout.decode().splitlines()[:1]
    failed = sum(bool(r["problems"]) for r in results)
    summary = {"ffmpeg": version[0] if version else None, "cases": len(results), "failed": failed}
    if tools.report:
        tools.report.write_text(json.dumps({"summary": summary, "results": results}, indent=1) + "\n")
    print(json.dumps(summary))
    return 1 if failed or not results else 0


if __name__ == "__main__":
    sys.exit(main())
