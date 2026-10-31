#!/usr/bin/env python3
"""R3 on the fixtures (docs/design/compat-cli.md, "The 0.1.0 bars"): the
outputs `coreav ffmpeg` wrote for the `ffmpeg` commands of `commands-0.1.0`,
judged with the oracle of each command's job at that oracle's own rules
(those rules set aside the fixtures UNMODELLED names).

    copy_check.py --records FILE [--ffmpeg PATH] [--ffprobe PATH]
                  [--mediainfo PATH] [--jobs N] [--report FILE]

FILE is the `records.json` the `the_0_1_0_commands_on_the_fixtures` test of
coreav-cli writes (`COREAV_FFMPEG_OUT`): each command run on a fixture, its
outcome and the summary its writer's own command would print. Every
written output is checked twice:

- selection: the pinned `ffmpeg` runs the same command on the same file, and
  its output must have the same streams as CoreAV's (count, and each
  stream's type and codec), in order. A command `ffmpeg` fails is reported
  apart and checked by its oracle alone;
- the job's oracle, from an entry built from the summary as
  `tools/oracle/writer_corpus.py` builds it for the native commands:
  `scripts/remux_check.py` (MP4 and MOV sources),
  `scripts/mkv_remux_check.py` (Matroska and WebM; the streams outside the
  selection are not expected in the output), `clip_check.py`,
  `split_check.py`, `extract_check.py` and `scripts/join_check.py` (a join
  also against `ffmpeg -f concat` on the same inputs), each with its known
  divergences. A clip's requested range must also be the one the command
  names, read here from its arguments and the input's start time.

Outputs with the same bytes from the same source (the spellings of one
command) are judged once. Prints the counts per entry; exits 1 if any
output fails. ffmpeg, ffprobe and MediaInfo are black boxes.
"""
import argparse
import collections
import concurrent.futures
import fractions
import hashlib
import json
import re
import pathlib
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(ROOT / "tools" / "oracle"))

import track_boxes  # noqa: E402
import writer_corpus  # noqa: E402

TIMEOUT = 600
MATROSKA = (".mkv", ".webm")
# Fixtures whose picture size changes inside the track
# (docs/design/frame-sizes.md): the clip and split oracles do not model a
# decoder that reinitializes mid-stream, and their own fixture runs leave
# them out, so their clips and segments are set aside here too.
UNMODELLED = {"clip": ("size_inband_sps.mp4", "size_vp9_resize.mp4"),
              "split": ("size_inband_sps.mp4", "size_vp9_resize.mp4"),
              "join": ("size_inband_sps.mp4", "size_vp9_resize.mp4")}


def outputs(record):
    """The files a written record produced, in order."""
    output = pathlib.Path(record["output"])
    if record["job"] == "split":
        return [seg["output"]["path"] for seg in record["detail"]["segments"]]
    return [str(output)]


def digest(paths):
    h = hashlib.sha256()
    for path in paths:
        h.update(pathlib.Path(path).read_bytes())
    return h.hexdigest()


def streams(ffprobe, path):
    done = subprocess.run([ffprobe, "-v", "error", "-of", "json", "-show_streams", str(path)],
                          capture_output=True, timeout=TIMEOUT, check=False)
    if done.returncode != 0:
        return None
    return json.loads(done.stdout).get("streams", [])


MAPPING = re.compile(r"^\s*Stream #0:(\d+)(?:\[[^]]*\])? -> #0:(\d+) \(copy\)", re.M)


def selected(record):
    """The input streams CoreAV wrote, in output order, from the summary."""
    summary = record["detail"]
    if "stream" in summary:
        return [summary["stream"]["stream_index"]]
    if "inputs" in summary:
        return [s["stream_index"] for s in summary["inputs"][0]["streams"]]
    if "segments" in summary:
        listed = [s for seg in summary["segments"] for s in seg["streams"]]
        return sorted({s["stream_index"] for s in listed}, key=lambda i: [s["stream_index"] for s in listed].index(i))
    return [s["stream_index"] for s in summary["streams"]]


def selection(record, ffmpeg):
    """None when the pinned ffmpeg maps the same input streams to the same
    output streams (its "Stream mapping", the streams it selects, not the
    tracks its muxer adds or those it ends up writing no packet of),
    "ffmpeg fails" when it cannot run the command, else the difference."""
    with tempfile.TemporaryDirectory(prefix="copy-check-") as tmp:
        target = pathlib.Path(tmp) / pathlib.Path(record["output"]).name
        args = [record.get("input", record["source"]) if a == "INPUT" else str(target) if a == "OUTPUT" else a for a in record["args"]]
        done = subprocess.run([ffmpeg, "-nostdin", "-hide_banner", "-v", "info"] + args, capture_output=True,
                              timeout=TIMEOUT, check=False)
    mapped = sorted((int(o), int(i)) for i, o in MAPPING.findall(done.stderr.decode(errors="replace")))
    if done.returncode != 0 or not mapped:
        return "ffmpeg fails"
    theirs, ours = [i for _, i in mapped], selected(record)
    if theirs != ours:
        return f"streams: coreav {ours}, ffmpeg {theirs}"
    return None


def seconds(text):
    """A time in ffmpeg's duration syntax, exactly."""
    if ":" in text:
        clock, _, fraction = text.partition(".")
        *hours_minutes, whole = clock.split(":")
        hours, minutes = ([0, 0] + [int(x) for x in hours_minutes])[-2:]
        return hours * 3600 + minutes * 60 + fractions.Fraction(whole + "." + fraction if fraction else whole)
    for unit, scale in (("ms", 1000), ("us", 1_000_000), ("s", 1)):
        if text.endswith(unit):
            return fractions.Fraction(text[:-len(unit)]) / scale
    return fractions.Fraction(text)


def requested(args, origin):
    """The range on the input's timeline that a clip command names, as the
    pinned ffmpeg reads it (black box): every time counts from the input's
    start time, origin; an input -ss moves the output's time 0, from which
    an output -ss and -to count; an output -t counts from the output -ss;
    -t wins over -to. The start is at least 0, the end None when open."""
    at = args.index("-i")

    def last(group, name):
        values = [value for option, value in zip(group, group[1:]) if option == name]
        return seconds(values[-1]) if values else None

    before, after = args[:at], args[at + 2:]
    shift = origin + (last(before, "-ss") or 0)
    start = shift + (last(after, "-ss") or 0)
    ends = []
    if last(before, "-t") is not None:
        ends.append(shift + last(before, "-t"))
    elif last(before, "-to") is not None:
        ends.append(origin + last(before, "-to"))
    if last(after, "-t") is not None:
        ends.append(start + last(after, "-t"))
    elif last(after, "-to") is not None:
        ends.append(shift + last(after, "-to"))
    return max(start, 0), min(ends) if ends else None


def start_time(ffprobe, path):
    """The input's start time as ffprobe prints it, 0 when it has none."""
    done = subprocess.run([ffprobe, "-v", "error", "-show_entries", "format=start_time", "-of", "csv=p=0", str(path)],
                          capture_output=True, timeout=TIMEOUT, check=True, text=True)
    text = done.stdout.strip()
    return fractions.Fraction(text) if text not in ("", "N/A") else fractions.Fraction(0)


def range_problems(record, ffprobe):
    """The difference between the range a clip command names and the one
    CoreAV's clip was asked for, if any."""
    summary = record["detail"]
    ratio = lambda r: fractions.Fraction(r["num"], r["den"]) if r else None  # noqa: E731
    theirs = requested(record["args"], start_time(ffprobe, record["source"]))
    ours = (ratio(summary["start"]["requested"]), ratio(summary["end"].get("requested")))
    problems = [] if ours == theirs else ["requested range: coreav %s, the command names %s" % (ours, theirs)]
    # A clip without video is not snapped to a sync sample: it starts where
    # the command says.
    snapped = writer_corpus.ratio(summary["start"]["snapped"])
    if not any(s["kind"] == "video" for s in summary["streams"]) and fractions.Fraction(
            snapped["num"], snapped["den"]) != theirs[0]:
        problems.append(f"start: the audio clip starts at {snapped}, the command names {theirs[0]}")
    return problems


def micros(value):
    """A time in whole microseconds, halves away from zero, as ffmpeg's
    demuxers count a file's start time and duration."""
    exact = abs(fractions.Fraction(value)) * 1_000_000
    whole = int(exact + fractions.Fraction(1, 2))
    return whole if value >= 0 else -whole


def placement(offsets, formats):
    """The problems of a join's placement: each input's offset (from the
    first input's) against where ffmpeg's concat demuxer places that file,
    after the files before it, each as long as its format duration, less its
    own start time; `formats` holds each input's (start_time, duration) in
    microseconds. Any difference is a problem."""
    problems, end = [], 0
    for index, (offset, (start, duration)) in enumerate(zip(offsets, formats)):
        expected = end - start + formats[0][0]
        ours = micros(offset - offsets[0])
        if ours != expected:
            problems.append(f"timing: input {index} placed at {ours} us, ffmpeg concat {expected} us")
        end += duration
    return problems


# KD-FF9-FIXTURE-SIDX-DURATION (docs/known-divergences-ffprobe9.md): on a
# fragmented file with a segment index and no edit list whose video starts
# presenting after 0, ffmpeg makes the file that much longer than its boxes
# do, so its concat demuxer places the next file that much later.
SIDX_DURATION = "KD-FF9-FIXTURE-SIDX-DURATION"


def presentation(movie):
    """(end, video start, video samples) of a fragmented movie none of whose
    tracks has an edit list, from its boxes, times in seconds; None for any
    other movie. With no edit list a track's media timeline is its
    presentation timeline (ISO/IEC 14496-12 8.6.6), and a sample presents
    from its composition time, its decode time (the fragment's tfdt,
    8.8.12, then the durations before it, 8.8.8) plus its composition
    offset (8.6.1.3), for its duration. The end is the latest over every
    track; the video start is the first video track's earliest."""
    if not movie["fragmented"] or any(t["edits"] is not None for t in movie["tracks"]):
        return None
    end, video = fractions.Fraction(0), None
    for track in movie["tracks"]:
        samples = track["samples"]
        if samples["gap"] or not track["timescale"]:
            return None
        dts, first = samples["base_dts"], None
        for duration, cto in zip(samples["durations"], samples["cto"]):
            composition = fractions.Fraction(dts + cto, track["timescale"])
            first = composition if first is None else min(first, composition)
            end = max(end, composition + fractions.Fraction(duration, track["timescale"]))
            dts += duration
        if track["handler"] == "vide" and video is None and first is not None:
            video = (first, len(samples["sizes"]))
    if video is None:
        return None
    return end, video[0], video[1]


def composition_gaps(formats, found):
    """Per input, the time by which ffmpeg's format duration exceeds the end
    of its presentation by its boxes ([presentation]) when that excess is
    exactly the video's first composition time
    (SIDX_DURATION), else 0; `formats` as [placement] takes them."""
    gaps = []
    for (_, duration), item in zip(formats, found):
        gap = fractions.Fraction(0)
        if item is not None and item[1] > 0 and duration - micros(item[0]) == micros(item[1]):
            gap = item[1]
        gaps.append(gap)
    return gaps


def without_gaps(times, gaps, counts):
    """The packet times of a concat output, each input's packets moved back
    by the gaps of the inputs before it; `counts` holds each input's video
    packet count."""
    out, shift, at = [], fractions.Fraction(0), 0
    for gap, count in zip(gaps, counts):
        out.extend((pts - shift, dts - shift) for pts, dts in times[at:at + count])
        at += count
        shift += gap
    return out + times[at:]


def packet_times(ffprobe, path):
    """The first video stream's (pts, dts) in seconds, from its first
    packet's pts, edit lists applied."""
    done = subprocess.run([ffprobe, "-v", "error", "-of", "json", "-select_streams", "v:0", "-show_entries",
                           "stream=time_base:packet=pts,dts", str(path)], capture_output=True, timeout=TIMEOUT,
                          check=True)
    data = json.loads(done.stdout)
    base = fractions.Fraction(data["streams"][0]["time_base"])
    packets = data.get("packets", [])
    first = packets[0]["pts"] if packets else 0
    return [((p["pts"] - first) * base, (p["dts"] - first) * base) for p in packets]


def concat_problems(record, ffprobe, ffmpeg):
    """A join's timing against ffmpeg's concat demuxer, where any
    disagreement fails (join_check.py only reports its own concat timing
    comparison): every input placed where the demuxer places it
    ([placement], from each input's format start_time and duration by the
    pinned ffprobe), and the video packets' times, from the first packet's,
    those of the pinned ffmpeg's output of the same command. The audio
    packets are not compared one for one: where a file's audio runs past
    its edit, ffmpeg's muxer moves the next file's first packet to one tick
    after the overlapping one (black box: durations of 1 and 135 ticks at
    the seam of a fixture listed twice), so its times there are not the
    demuxer's placement, which the first check covers for every stream.
    Where ffmpeg makes an input longer than its boxes by its video's first
    composition time (SIDX_DURATION), both checks use the end by
    the boxes: the placement takes it as the input's duration, and
    ffmpeg's packets after that input move back by the excess, each input
    holding the video packets the pinned ffprobe lists for it."""
    formats = []
    for item in record["detail"]["inputs"]:
        done = subprocess.run([ffprobe, "-v", "error", "-of", "json", "-show_entries", "format=start_time,duration",
                               item["path"]], capture_output=True, timeout=TIMEOUT, check=True)
        fmt = json.loads(done.stdout)["format"]
        formats.append((micros(fractions.Fraction(fmt["start_time"])), micros(fractions.Fraction(fmt["duration"]))))
    offsets = [fractions.Fraction(r["num"], r["den"]) for r in (writer_corpus.ratio(item["offset"]) for item in record["detail"]["inputs"])]
    found = []
    for item in record["detail"]["inputs"]:
        try:
            indexed = any(kind == b"sidx" for kind, *_ in track_boxes.top_level(item["path"]))
            found.append(presentation(track_boxes.read_movie(item["path"])) if indexed else None)
        except track_boxes.Unreadable:
            found.append(None)
    gaps = composition_gaps(formats, found)
    problems = placement(offsets, [(start, duration - micros(gap)) for (start, duration), gap in zip(formats, gaps)])
    with tempfile.TemporaryDirectory(prefix="copy-check-concat-") as tmp:
        reference = pathlib.Path(tmp) / "concat.mp4"
        args = [record["input"] if a == "INPUT" else str(reference) if a == "OUTPUT" else a for a in record["args"]]
        done = subprocess.run([ffmpeg, "-nostdin", "-v", "error"] + args, capture_output=True, timeout=TIMEOUT,
                              check=False)
        if done.returncode != 0:
            return problems + [f"timing: ffmpeg fails on the command ({done.stderr.decode(errors='replace')[-120:]})"]
        theirs = packet_times(ffprobe, reference)
    if any(gaps):
        counts = [len(packet_times(ffprobe, item["path"])) for item in record["detail"]["inputs"]]
        theirs = without_gaps(theirs, gaps, counts)
    ours = packet_times(ffprobe, record["output"])
    if ours != theirs:
        differ = next((i for i, (a, b) in enumerate(zip(ours, theirs)) if a != b), min(len(ours), len(theirs)))
        problems.append(f"timing: video packet {differ} of {len(ours)} at other times than ffmpeg concat's "
                        f"({len(theirs)} packets)")
    return problems


def entry(record, src_movie, source_streams):
    """The oracle's entry for one written record, as writer_corpus builds it."""
    summary, source, output = record["detail"], record["source"], record["output"]
    job = record["job"]
    if job == "clip":
        kinds = {s["kind"]: s for s in summary["streams"]}
        requested = summary["end"].get("requested")
        return {"source": source, "output": output, "label": record["id"],
                "end": {"num": requested["num"], "den": requested["den"]} if requested else None,
                "snapped_start": writer_corpus.ratio(summary["start"]["snapped"]),
                "written_end": writer_corpus.ratio(summary["end"]["written"]),
                "video": writer_corpus.kept_track(src_movie, kinds["video"]) if "video" in kinds else None,
                "audio": writer_corpus.kept_track(src_movie, kinds["audio"]) if "audio" in kinds else None}
    if job == "join":
        inputs = []
        for item in summary["inputs"]:
            kinds = {s["kind"]: s["stream_index"] for s in item["streams"]}
            inputs.append({"path": item["path"], "offset": writer_corpus.ratio(item["offset"]),
                           "video_start": writer_corpus.ratio(item["video_start"]),
                           "video_end": writer_corpus.ratio(item["video_end"]), "extended": item["extended"],
                           "video_stream": kinds["video"], "audio_stream": kinds.get("audio")})
        return {"label": record["id"], "source": None, "source_offset": None, "output": output,
                "duration": writer_corpus.ratio(summary["duration"]), "video_edits": summary["edits"]["video"],
                "audio_edits": summary["edits"]["audio"], "inputs": inputs}
    if job == "split":
        first = summary["segments"][0]["streams"]
        segments = []
        for seg in summary["segments"]:
            kinds = {s["kind"]: s for s in seg["streams"]}
            segments.append({"output": seg["output"]["path"], "start": writer_corpus.ratio(seg["start"]),
                             "video_start": writer_corpus.ratio(seg["video_start"]),
                             "end": writer_corpus.ratio(seg["end"]),
                             "video_first": kinds["video"]["first_sample"],
                             "video_samples": kinds["video"]["samples"],
                             "audio_first": kinds["audio"]["first_sample"] if "audio" in kinds else 0,
                             "audio_samples": kinds["audio"]["samples"] if "audio" in kinds else 0})
        return {"source": source, "label": record["id"],
                "video_stream": next(s["stream_index"] for s in first if s["kind"] == "video"),
                "audio_stream": next((s["stream_index"] for seg in summary["segments"] for s in seg["streams"]
                                      if s["kind"] == "audio"), None),
                "segments": segments}
    if "stream" in summary:
        return {"source": source, "stream_index": summary["stream"]["stream_index"], "output": output}
    kept = [s["stream_index"] for s in summary["streams"]]
    return {"source": source, "output": output,
            "dropped_streams": [s["index"] for s in source_streams if s["index"] not in kept],
            "streams": [{"stream_index": s["stream_index"], "track_id": s["track_id"],
                         "source_track_id": s.get("source_track_id")} for s in summary["streams"]],
            "keep": None}


def oracle(record, tools):
    """The problems the job's oracle finds in one written record."""
    import clip_check
    import extract_check
    import join_check
    import mkv_remux_check
    import remux_check
    import split_check
    ffprobe, ffmpeg, mediainfo = tools["ffprobe"], tools["ffmpeg"], tools["mediainfo"]
    source = record["source"]
    if source.endswith(MATROSKA):
        summary = record["detail"]
        selected = [s["stream_index"] for s in summary["streams"]]
        result = mkv_remux_check.check(source, record["output"], summary, mediainfo, selected=selected)
        return [json.dumps(p) for p in result["problems"]]
    src_movie = track_boxes.read_movie(source) if record["job"] in ("clip", "split") else None
    if record["job"] == "join":
        problems = join_check.check(entry(record, None, []), ffprobe, ffmpeg, mediainfo, concat=True)[0]
        return [str(p) for p in problems] + concat_problems(record, ffprobe, ffmpeg)
    item = entry(record, src_movie, streams(ffprobe, source) or [])
    if record["job"] == "clip":
        problems = range_problems(record, ffprobe) + list(clip_check.check(item, ffprobe, ffmpeg, mediainfo)[0])
    elif record["job"] == "split":
        problems = split_check.check(item, ffprobe, ffmpeg, mediainfo)[0]
    elif "stream_index" in item:
        problems = extract_check.check(item, ffprobe, ffmpeg, mediainfo)[0]
    else:
        problems = [p["message"] for p in remux_check.check(item, ffprobe, ffmpeg, mediainfo)[0]]
    return [str(p) for p in problems]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--records", required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--mediainfo")
    parser.add_argument("--jobs", type=int, default=None)
    parser.add_argument("--report")
    args = parser.parse_args()
    records = json.loads(pathlib.Path(args.records).read_text(encoding="utf-8"))
    tools = {"ffmpeg": args.ffmpeg, "ffprobe": args.ffprobe, "mediainfo": args.mediainfo}
    counts = collections.defaultdict(collections.Counter)
    written = []
    for record in records:
        counts[record["id"]][record["status"]] += 1
        if record["status"] == "written":
            written.append(record)
    # One oracle run per distinct output of a source; the selection check
    # runs per command, since each spelling is its own ffmpeg command.
    groups = collections.defaultdict(list)
    for record in written:
        groups[(record["source"], record["job"], digest(outputs(record)))].append(record)
    failures = []
    with concurrent.futures.ProcessPoolExecutor(max_workers=args.jobs) as pool:
        judged = {key: pool.submit(oracle, group[0], tools) for key, group in groups.items()
                  if pathlib.Path(key[0]).name not in UNMODELLED.get(key[1], ())}
        mapped = {id(r): pool.submit(selection, r, tools["ffmpeg"]) for r in written}
        for key, group in groups.items():
            try:
                problems = judged[key].result() if key in judged else None
            except Exception as error:  # noqa: BLE001
                problems = [f"harness: {type(error).__name__}: {error}"]
            for record in group:
                difference = mapped[id(record)].result()
                c = counts[record["id"]]
                if difference == "ffmpeg fails":
                    c["ffmpeg fails"] += 1
                    difference = None
                if problems is None and not difference:
                    c["set aside"] += 1
                elif problems or difference:
                    c["failed"] += 1
                    failures.append({"id": record["id"], "args": record["args"], "source": record["source"],
                                     "selection": difference, "problems": (problems or [])[:5]})
                else:
                    c["passed"] += 1
    for id_, c in sorted(counts.items()):
        print(id_, dict(sorted(c.items())))
    for f in failures:
        print("FAIL", f["id"], " ".join(f["args"]), pathlib.Path(f["source"]).name,
              f["selection"] or "", "; ".join(f["problems"])[:600])
    if args.report:
        pathlib.Path(args.report).write_text(json.dumps({"counts": counts, "failures": failures}, indent=1) + "\n",
                                             encoding="utf-8")
    if not written:
        print("no output was written", file=sys.stderr)
        return 1
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
