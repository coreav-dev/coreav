#!/usr/bin/env python3
"""Black-box oracles for Matroska/WebM-to-MP4 remuxes (docs/design/mkv-remux.md).

`check SOURCE OUTPUT SUMMARY` compares one output with its source, using the
`coreav.remux/1` summary that `coreav remux` printed (which source stream
each output stream is):

- layout: the output's top-level boxes are `ftyp`, `moov`, `mdat`, in that
  order, and ffprobe reads it as an MP4 file;
- kept: every source stream of a codec the remux copies is in the output,
  unless the summary drops it as encrypted, without frames or with nothing
  presented (not trusting the summary for anything else); with a stream
  selection (`check`'s `selected`, as `coreav ffmpeg -map` makes one),
  every selected stream of such a codec;
- streams: one output stream per kept source stream, with the source's
  codec, profile, dimensions, colour description, sample rate and channels
  in ffprobe;
- packets: `ffmpeg -i FILE -map 0:N -c copy -f framemd5` of each kept
  stream has the source stream's packets, in order, with the same size and
  MD5, and each packet's presentation time within 1 ms of the source's for
  video and within 1 ms plus half the frame's duration for audio, both read
  with `-copyts`. For H.264 that the summary marks `annex_b` (rewritten from
  start codes to NAL unit lengths) the decoded frames' MD5s are compared
  instead, and their count;
- presentation: every packet is inside the output stream's presented range
  (ffprobe's start time and duration, from the edit list), audio priming
  aside;
- MediaInfo, when given: an MPEG-4 file with one track per kept stream.

Prints a JSON result. `fixtures --coreav BIN --media DIR --out DIR` remuxes
every Matroska fixture with `coreav remux` and checks each output; the
fixtures with nothing MP4 can carry must be refused with the expected
message (FIXTURE_REFUSALS). `tools/mkv-adapter/remux_corpus.py` runs the
same checks on the corpus. ffprobe, ffmpeg and MediaInfo are black boxes.
"""
import argparse
import json
import pathlib
import subprocess
import sys
from fractions import Fraction

TIMEOUT = 3600
# The codecs this remux copies (ffprobe names), and the reasons a stream of
# one of them may still be left out.
COPYABLE = ("h264", "hevc", "av1", "vp9", "opus", "aac")
ACCEPTED_DROPS = ("encrypted streams are not copied", "the stream has no frames",
                  "the file ends before the stream's first frame", "nothing of the stream is presented")
STREAM_KEYS = ("codec_name", "profile", "width", "height", "color_range", "color_space",
               "color_transfer", "color_primaries", "sample_rate", "channels")


def run(argv):
    done = subprocess.run(argv, capture_output=True, timeout=TIMEOUT, check=False)
    return done.returncode, done.stdout


def ffprobe(path):
    code, out = run(["ffprobe", "-v", "error", "-of", "json", "-show_format", "-show_streams", str(path)])
    return json.loads(out) if code == 0 else None


def framemd5(path, index, copy=True):
    argv = ["ffmpeg", "-nostdin", "-v", "error", "-copyts", "-i", str(path), "-map", f"0:{index}"]
    if copy:
        argv += ["-c", "copy"]
    else:
        argv += ["-fps_mode", "passthrough"]
    argv += ["-f", "framemd5", "-"]
    code, out = run(argv)
    if code != 0:
        return None, []
    base, packets = None, []
    for line in out.decode("utf-8", "replace").splitlines():
        if line.startswith("#tb"):
            num, den = line.split(":", 1)[1].strip().split("/")
            base = Fraction(int(num), int(den))
        elif line and not line.startswith("#"):
            parts = [p.strip() for p in line.split(",")]
            if len(parts) >= 6:
                packets.append({"pts": int(parts[2]), "duration": int(parts[3]), "size": int(parts[4]),
                                "md5": parts[5]})
    return base, packets


def top_level(path):
    kinds = []
    with open(path, "rb") as f:
        f.seek(0, 2)
        end = f.tell()
        at = 0
        while at + 8 <= end and len(kinds) < 64:
            f.seek(at)
            head = f.read(16)
            size, kind = int.from_bytes(head[:4], "big"), head[4:8].decode("latin-1")
            if size == 1:
                size = int.from_bytes(head[8:16], "big")
            elif size == 0:
                size = end - at
            if size < 8:
                break
            kinds.append(kind)
            at += size
    return kinds


def mediainfo(binary, path):
    code, out = run([binary, "--Output=JSON", str(path)])
    if code != 0:
        return None
    try:
        return json.loads(out)
    except json.JSONDecodeError:
        return None


def check(source, output, summary, mediainfo_bin=None, selected=None):
    problems, packets_ok, packets_total = [], 0, 0

    def problem(field, **data):
        problems.append({"field": field, **data})

    layout = top_level(output)
    if layout[:3] != ["ftyp", "moov", "mdat"] or len(layout) != 3:
        problem("layout", output=layout)
    src, out = ffprobe(source), ffprobe(output)
    if out is None or "mp4" not in out.get("format", {}).get("format_name", ""):
        problem("output_probe")
        return {"ok": False, "problems": problems, "packets_ok": 0, "packets_total": 0}
    kept = summary["streams"]
    # Independently of the summary: every source stream of a codec this
    # remux copies is in the output, unless it cannot be (encrypted, no
    # frames, nothing presented).
    reasons = {d["stream_index"]: d.get("message") for d in summary.get("dropped_streams", [])}
    kept_indexes = {k["stream_index"] for k in kept}
    for s in src["streams"]:
        if (s.get("codec_type") in ("video", "audio") and s.get("codec_name") in COPYABLE
                and s.get("index") not in kept_indexes
                and (selected is None or s.get("index") in selected)
                and reasons.get(s.get("index")) not in ACCEPTED_DROPS):
            problem("stream_not_kept", stream=s.get("index"), codec=s.get("codec_name"),
                    reason=reasons.get(s.get("index")))
    if len(out["streams"]) != len(kept):
        problem("stream_count", source=len(kept), output=len(out["streams"]))
    for i, stream in enumerate(kept):
        s_index = stream["stream_index"]
        s = next((x for x in src["streams"] if x.get("index") == s_index), None)
        o = out["streams"][i] if i < len(out["streams"]) else None
        if s is None or o is None:
            problem("stream_missing", stream=s_index)
            continue
        for key in STREAM_KEYS:
            if s.get(key) != o.get(key):
                problem(f"stream.{key}", stream=s_index, source=s.get(key), output=o.get(key))
        sbase, spk = framemd5(source, s_index)
        obase, opk = framemd5(output, i)
        # H.264 rewritten from Annex B start codes to NAL unit lengths: the
        # packets change form, the decoded pictures must not.
        annex_b = bool(stream.get("annex_b"))
        if annex_b:
            # Times from the copied packets (the decoder's time base is
            # coarser), checksums from the decoded pictures.
            _, sdec = framemd5(source, s_index, copy=False)
            _, odec = framemd5(output, i, copy=False)
            if len(sdec) != len(spk) or len(odec) != len(opk):
                problem("decoded_count", stream=s_index, source=len(sdec), output=len(odec))
            for packets, decoded in ((spk, sdec), (opk, odec)):
                for p, d in zip(sorted(packets, key=lambda f: f["pts"]), sorted(decoded, key=lambda f: f["pts"])):
                    p["md5"] = d["md5"]
        if sbase is None or obase is None:
            problem("framemd5_failed", stream=s_index)
            packets_total += max(len(spk), len(opk))
            continue
        audio = s.get("codec_type") == "audio"
        packets_total += max(len(spk), len(opk))
        bad = 0
        for a, b in zip(spk, opk):
            same = a["md5"] == b["md5"] and (annex_b or a["size"] == b["size"])
            # Audio: within half the frame's own duration (frames last what
            # they decode to, while the source's times may jitter), which
            # ffmpeg reads from the packet (the Opus TOC; one AAC frame). The
            # longer of the two: ffmpeg 9 shortens the source's last packet
            # by the Matroska discard padding.
            frame = max(a["duration"] * sbase, b["duration"] * obase) if audio else 0
            tolerance = Fraction(1, 1000) + frame / 2
            close = abs(a["pts"] * sbase - b["pts"] * obase) <= tolerance
            if same and close:
                packets_ok += 1
            else:
                bad += 1
                if bad <= 3:
                    problem("packet", stream=s_index, source=a, output=b,
                            source_time=float(a["pts"] * sbase), output_time=float(b["pts"] * obase))
        if len(spk) != len(opk):
            problem("packet_count", stream=s_index, source=len(spk), output=len(opk))
        # Every packet is presented by the edit list (ffprobe's start time
        # and duration of the output stream), audio priming aside.
        start, length = o.get("start_time"), o.get("duration")
        if start is not None and length is not None and opk:
            start, end = Fraction(start), Fraction(start) + Fraction(length)
            # Half a millisecond for the edit's rounding to the movie
            # timescale.
            slack = Fraction(1, 2000)
            outside = [b for b in opk if b["pts"] * obase >= end + slack
                       or (not audio and b["pts"] * obase < start - slack)]
            if outside:
                problem("not_presented", stream=s_index, count=len(outside),
                        first=float(outside[0]["pts"] * obase), start=float(start), end=float(end))
        if bad > 3:
            problem("packets_more", stream=s_index, count=bad - 3)
    if mediainfo_bin:
        info = mediainfo(mediainfo_bin, output)
        tracks = (info or {}).get("media", {}).get("track", [])
        general = next((t for t in tracks if t.get("@type") == "General"), {})
        media = [t for t in tracks if t.get("@type") in ("Video", "Audio")]
        if general.get("Format") != "MPEG-4" or len(media) != len(kept):
            problem("mediainfo", format=general.get("Format"), tracks=len(media))
    return {"ok": not problems, "problems": problems, "packets_ok": packets_ok,
            "packets_total": packets_total}


# The fixtures of scripts/gen-fixtures-mkv.sh that have nothing MP4 can
# carry, with the error message `coreav remux` must give.
FIXTURE_REFUSALS = {
    "mkv_lace_fixed.mkv": "unsupported: PCM audio is not copied to MP4",
    "mkv_mp3.mkv": "unsupported: MP3 is not copied to MP4 by this remux",
    "mkv_pcm.mkv": "unsupported: PCM audio is not copied to MP4",
    "webm_vp8_vorbis.webm": ("unsupported: VP8 in MP4 does not play in Safari; keep the WebM, which plays in "
                             "Chrome, Firefox and Safari, or re-encode to VP9, H.264 or AV1 outside CoreAV"),
}
# A copy of webm_vp8_vorbis.webm with a smaller PixelWidth and PixelHeight
# (docs/design/policy-matroska.md), refused the same way.
FIXTURE_REFUSALS["size_vp8_understated.webm"] = FIXTURE_REFUSALS["webm_vp8_vorbis.webm"]


def remux(coreav, source, output):
    """Runs `coreav remux`; returns (status, summary or error document)."""
    try:
        done = subprocess.run([coreav, "remux", "--force", str(source), str(output)], capture_output=True,
                              timeout=TIMEOUT, check=False)
    except subprocess.TimeoutExpired:
        return "timeout", None
    stream = done.stdout if done.returncode == 0 else done.stderr
    try:
        document = json.loads(stream)
    except json.JSONDecodeError:
        document = None
    if done.returncode == 0 and document is not None:
        return "remuxed", document
    if done.returncode == 1 and document is not None:
        return "refused", document
    return f"exit-{done.returncode}", document


def fixtures(args):
    """Remuxes every fixture: the refusals must be exactly FIXTURE_REFUSALS,
    and every other output must pass every check."""
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    results, wrong = {}, {}
    sources = sorted(p for d in args.media for p in pathlib.Path(d).iterdir()
                     if p.suffix in (".mkv", ".webm"))
    for source in sources:
        output = out / f"{source.name}.mp4"
        status, document = remux(args.coreav, source, output)
        expected = FIXTURE_REFUSALS.get(source.name)
        if status == "refused" and expected is not None and document.get("message") == expected:
            continue
        if status != "remuxed" or expected is not None:
            wrong[source.name] = {"status": status, "document": document, "expected": expected}
            continue
        results[source.name] = check(source, output, document, args.mediainfo)
    ok = sum(r["ok"] for r in results.values())
    packets = [sum(r["packets_ok"] for r in results.values()), sum(r["packets_total"] for r in results.values())]
    print(json.dumps({"files": [ok, len(results)], "packets": packets, "unexpected": wrong,
                      "failed": {k: v["problems"][:3] for k, v in results.items() if not v["ok"]}}, indent=1))
    return 0 if ok == len(results) and results and not wrong else 1


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    one = sub.add_parser("check")
    one.add_argument("source")
    one.add_argument("output")
    one.add_argument("summary")
    one.add_argument("--mediainfo")
    batch = sub.add_parser("fixtures")
    batch.add_argument("--coreav", required=True)
    batch.add_argument("--media", action="append", required=True)
    batch.add_argument("--out", required=True)
    batch.add_argument("--mediainfo")
    args = parser.parse_args()
    if args.command == "check":
        summary = json.loads(pathlib.Path(args.summary).read_text())
        result = check(args.source, args.output, summary, args.mediainfo)
        print(json.dumps(result, indent=1))
        return 0 if result["ok"] else 1
    return fixtures(args)


if __name__ == "__main__":
    sys.exit(main())
