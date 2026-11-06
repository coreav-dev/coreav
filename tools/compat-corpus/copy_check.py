#!/usr/bin/env python3
"""R3 on the fixtures (docs/design/compat-cli.md, "The 0.1.0 bars"): the
outputs `coreav ffmpeg` wrote for the `ffmpeg` commands of `commands-0.1.0`,
judged with the oracle of each command's job at that oracle's own rules
(those rules set aside the fixtures UNMODELLED names).

    copy_check.py --records FILE [--ffmpeg PATH] [--ffprobe PATH]
                  [--mediainfo PATH] [--jobs N] [--report FILE] [--timings]

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
  selection are not expected in the output), `clip_check.py` for native
  gapless clips, the bounded AVC/AAC input-seek model here (complete pinned
  output and independent ISO checks),
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
import os
import re
import pathlib
import subprocess
import struct
import sys
import tempfile
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(ROOT / "tools" / "oracle"))

import track_boxes  # noqa: E402
import writer_corpus  # noqa: E402
import writer_boxes  # noqa: E402
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT  # noqa: E402

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


def nearest(value):
    """The nearest integer, halves away from zero."""
    exact = abs(fractions.Fraction(value))
    whole = int(exact + fractions.Fraction(1, 2))
    return whole if value >= 0 else -whole


def micros(value):
    """A time in whole microseconds, halves away from zero, as ffmpeg's
    demuxers count a file's start time and duration."""
    return nearest(fractions.Fraction(value) * 1_000_000)


def placement(offsets, formats, timescale=None):
    """The problems of a join's placement: each input's offset (from the
    first input's) against where ffmpeg's concat demuxer places that file,
    after the files before it, each as long as its format duration, less its
    own start time; `formats` holds each input's (start_time, duration) in
    microseconds. With the video's `timescale`, the offsets are compared in
    its ticks, the demuxer's rounded to the nearest one, halves away from
    zero, where the muxer can place the video (black box: a file of 4.032 s
    before a 1/12800 video places the next file's video 51610 ticks later).
    Any difference is a problem."""
    problems, end = [], 0
    for index, (offset, (start, duration)) in enumerate(zip(offsets, formats)):
        expected = end - start + formats[0][0]
        ours = micros(offset - offsets[0])
        if timescale is not None:
            ours = (offset - offsets[0]) * timescale
            expected = nearest(fractions.Fraction(expected * timescale, 1_000_000))
            unit = "ticks"
        else:
            unit = "us"
        if ours != expected:
            problems.append(f"timing: input {index} placed at {ours} {unit}, ffmpeg concat {expected} {unit}")
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
    timescale = record["detail"]["inputs"][0]["offset"]["timescale"]
    problems = placement(offsets, [(start, duration - micros(gap)) for (start, duration), gap in zip(formats, gaps)],
                         timescale)
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
                "audio_edits": summary["edits"]["audio"], "inputs": inputs, "placement": "concat"}
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


INPUT_SEEK_MAX_SAMPLES = 200_000
INPUT_SEEK_MAX_RAW = 1 << 30
INPUT_SEEK_ROLL = bytes.fromhex('01000000726f6c6c0000000200000001ffff')
INPUT_SEEK_ENTRIES = ('stream:stream_tags=language,handler_name:packet=stream_index,pts,dts,duration,size,flags,'
                      'data_hash:frame=stream_index,best_effort_timestamp,pkt_dts,duration:format=start_time,'
                      'duration:format_tags=')


def input_seek_args(record):
    if record['job'] != 'clip' or record['source'].endswith(MATROSKA):
        return None
    args = record['args']
    if args.count('-i') != 1 or args[-1:] != ['OUTPUT']:
        return None
    at = args.index('-i')
    if args[at + 1:at + 2] != ['INPUT']:
        return None
    values, coverage = {}, set()
    codecs = {'-c': 'va', '-codec': 'va', '-c:v': 'v', '-vcodec': 'v', '-c:a': 'a', '-acodec': 'a'}
    for tokens, allowed in ((args[:at], {'-ss'}), (args[at + 2:-1], {*codecs, '-t', '-to'})):
        index = 0
        while index < len(tokens):
            name = tokens[index]
            if name in ('-y', '-hide_banner'):
                index += 1
                continue
            if name not in allowed or index + 1 == len(tokens):
                return None
            value = tokens[index + 1]
            if name in codecs:
                if value != 'copy':
                    return None
                coverage.update(codecs[name])
            elif name in values:
                return None
            else:
                values[name] = value
            index += 2
    bounds = set(values) & {'-t', '-to'}
    if '-ss' not in values or len(bounds) != 1 or coverage != {'v', 'a'}:
        return None
    try:
        return seconds(values['-ss']), seconds(values[bounds.pop()])
    except (ValueError, ZeroDivisionError):
        return None


def input_seek_operation(record):
    operation = input_seek_args(record)
    if operation is None:
        return None
    try:
        tracks = track_boxes.read_movie(record['source'])['tracks']
    except (track_boxes.Unreadable, OSError, struct.error):
        return operation
    if not any(t['handler'] == 'vide' and (t.get('entry') or {}).get('type') == 'avc1' for t in tracks):
        return None
    for track in tracks:
        entry = track.get('entry') or {}
        if track['handler'] == 'soun' and entry.get('type') == 'mp4a':
            config = writer_boxes.aac_config(entry)
            if not config or config[0] >> 3 == 2:
                return operation
    return None


def input_seek_facts(path):
    layout = track_boxes.top_level(path)
    moov = [b for b in layout if b[0] == b'moov']
    mdat = [b for b in layout if b[0] == b'mdat']
    if len(moov) != 1 or not mdat or not layout:
        raise ValueError('one movie and bounded media data required')
    if layout[-1][1] + layout[-1][3] != pathlib.Path(path).stat().st_size:
        raise ValueError('incomplete top-level layout')
    with open(path, 'rb') as handle:
        for _, offset, header, size in layout:
            handle.seek(offset)
            raw = handle.read(header)
            declared = struct.unpack_from('>Q' if header == 16 else '>I', raw, 8 if header == 16 else 0)[0]
            if declared and declared != size:
                raise ValueError('declared box extent exceeds the file')
    data = memoryview(track_boxes.read_box(path, *moov[0][1:], track_boxes.MAX_MOOV))

    def one(parent, kind, minimum=0):
        found = track_boxes.children(data, parent, kind)
        if len(found) != 1 or found[0][1] - found[0][0] < minimum:
            raise ValueError('missing or duplicated required box')
        return found[0]

    tracks = []
    root = (0, len(data))
    mvhd = one(root, b'mvhd', 1)
    if data[mvhd[0]] > 1 or mvhd[1] - mvhd[0] < (112 if data[mvhd[0]] else 100):
        raise ValueError('invalid movie header')
    for trak in track_boxes.children(data, root, b'trak'):
        tkhd = one(trak, b'tkhd', 1)
        if data[tkhd[0]] > 1 or tkhd[1] - tkhd[0] < (96 if data[tkhd[0]] else 84):
            raise ValueError('invalid track header')
        mdia = one(trak, b'mdia')
        mdhd = one(mdia, b'mdhd', 1)
        if data[mdhd[0]] > 1 or mdhd[1] - mdhd[0] < (36 if data[mdhd[0]] else 24):
            raise ValueError('invalid media header')
        stbl = one(one(mdia, b'minf'), b'stbl')
        for kind in (b'stsd', b'stts', b'ctts', b'stss', b'stsz', b'stz2', b'stsc', b'stco', b'co64'):
            tables = track_boxes.children(data, stbl, kind)
            if len(tables) > 1 or any(end - start < 4 or data[start] not in ((0, 1) if kind == b'ctts' else (0,))
                                      for start, end in tables):
                raise ValueError('duplicate sample table or unsupported table version')
        if len(track_boxes.children(data, stbl, b'stco') + track_boxes.children(data, stbl, b'co64')) != 1:
            raise ValueError('one chunk-offset table required')
        stsd = one(stbl, b'stsd', 8)
        stsc = one(stbl, b'stsc', 8)
        count = track_boxes.u32(data, stsc[0] + 4)
        if count > INPUT_SEEK_MAX_SAMPLES or stsc[0] + 8 + count * 12 != stsc[1]:
            raise ValueError('sample-to-chunk table exceeds its bounds')
        sizes = track_boxes.children(data, stbl, b'stsz') + track_boxes.children(data, stbl, b'stz2')
        if (len(sizes) != 1 or sizes[0][1] - sizes[0][0] < 12
                or track_boxes.u32(data, sizes[0][0] + 8) > INPUT_SEEK_MAX_SAMPLES):
            raise ValueError('input-seek sample budget exceeded')
        rates = []
        elst = track_boxes.child(data, track_boxes.child(data, trak, b'edts'), b'elst')
        if elst is not None:
            width = 20 if data[elst[0]] == 1 else 12
            entries = track_boxes.u32(data, elst[0] + 4)
            if data[elst[0]] > 1 or elst[0] + 8 + width * entries != elst[1]:
                raise ValueError('invalid edit table extent')
            rates = [track_boxes.u32(data, elst[0] + 8 + i * width + width - 4) for i in range(entries)]
        groups = {}
        for kind in (b'sgpd', b'sbgp'):
            found = track_boxes.children(data, stbl, kind)
            if len(found) > 1:
                raise ValueError('multiple sample-group boxes')
            groups[kind] = bytes(data[slice(*found[0])]) if found else None
        tracks.append(dict(volume=track_boxes.u16(data, tkhd[0] + (48 if data[tkhd[0]] else 36)),
                           descriptions=track_boxes.u32(data, stsd[0] + 4), rates=rates,
                           indices=[track_boxes.u32(data, stsc[0] + 16 + i * 12) for i in range(count)],
                           groups=groups))
    brand = track_boxes.read_box(path, *layout[0][1:], track_boxes.MAX_MOOV)[:4] if layout[0][0] == b'ftyp' else None
    media = (mdat[0][1] + mdat[0][2], mdat[0][1] + mdat[0][3]) if len(mdat) == 1 else None
    return dict(tracks=tracks, brand=brand, media=media,
                fragmented=any(b[0] == b'moof' for b in layout) or track_boxes.child(data, root, b'mvex') is not None)


def input_seek_times(track):
    samples = track['samples']
    if samples['base_dts'] != 0 or samples['gap'] or not samples['sizes']:
        raise ValueError('noncontiguous or empty sample timeline')
    if any(d <= 0 for d in samples['durations']) or any(s <= 0 for s in samples['sizes']):
        raise ValueError('nonpositive sample duration or size')
    if sum(samples['sizes']) > INPUT_SEEK_MAX_RAW:
        raise ValueError('input-seek sample byte budget exceeded')
    dts, now = [], 0
    for duration in samples['durations']:
        dts.append(now)
        now += duration
    compositions = [d + c for d, c in zip(dts, samples['cto'])]
    if any(c < 0 for c in samples['cto']) or len(set(compositions)) != len(compositions):
        raise ValueError('negative or duplicate composition time')
    return dts, compositions


def input_seek_plan(record, operation, movie, facts):
    seek, span = operation
    tracks = movie['tracks']
    if seek < 0 or span <= 0 or facts['fragmented'] or len(tracks) != 2:
        raise ValueError('outside the bounded two-track input-seek model')
    positions = {t['handler']: i for i, t in enumerate(tracks)}
    if set(positions) != {'vide', 'soun'} or movie['movie_timescale'] <= 0:
        raise ValueError('one video and one audio track required')
    runs = []
    for kind, handler in (('video', 'vide'), ('audio', 'soun')):
        index = positions[handler]
        track, extra = tracks[index], facts['tracks'][index]
        entry = track['entry']
        config = writer_boxes.aac_config(entry) if kind == 'audio' else None
        if (kind == 'video' and (entry['type'] != 'avc1' or track['timescale'] < 10_000)
                or kind == 'audio' and (not config or config[0] >> 3 != 2)):
            raise ValueError('AVC and AAC-LC source required')
        if (extra['descriptions'] != 1 or set(extra['indices']) != {1}
                or extra['volume'] != (0 if kind == 'video' else 256)):
            raise ValueError('unsupported source sample description or volume')
        edits = track['edits']
        if not edits or len(edits) != 1 or edits[0][1] < 0 or extra['rates'] != [65536]:
            raise ValueError('one complete unit-rate source edit required')
        clock, media = track['timescale'], edits[0][1]
        if clock <= 0 or (seek * clock).denominator != 1 or (span * clock).denominator != 1:
            raise ValueError('inexact media-clock bound')
        dts, cts = input_seek_times(track)
        samples = track['samples']
        last = max(c + d for c, d in zip(cts, samples['durations']))
        if edits[0][0] * clock != (last - media) * movie['movie_timescale']:
            raise ValueError('incomplete source presentation edit')
        if kind == 'video':
            if cts[0] != media or len(set(samples['durations'])) != 1:
                raise ValueError('nonzero origin or variable video cadence')
            keys = [i for i, c in enumerate(cts) if samples['sync'][i] and c <= media + seek * clock]
            if not keys or any(cts[a] >= cts[b] for a, b in zip(keys, keys[1:])):
                raise ValueError('no decode access point')
            first = max(keys, key=lambda i: cts[i])
            size = writer_boxes._length_size(entry)
            packet = next(track_boxes.iter_sample_bytes(record['source'], samples['offsets'], samples['sizes'], first, 1))
            if size is None or writer_boxes.access('h264', packet, size, writer_boxes.h264_max_ref_frames(entry)) != 'must':
                raise ValueError('unproved decode access point')
            video_origin = fractions.Fraction(dts[first] - media, clock)
        else:
            if any(samples['cto']) or (video_origin * clock).denominator != 1:
                raise ValueError('unqualified audio decode origin')
            first = max((i for i, d in enumerate(dts) if d <= media + video_origin * clock), default=0)
            expected = b'\0' * 4 + b'roll' + struct.pack('>III', 1, len(dts), 1)
            if extra['groups'] not in ({b'sgpd': None, b'sbgp': None}, {b'sgpd': INPUT_SEEK_ROLL, b'sbgp': expected}):
                raise ValueError('unqualified source audio roll')
        end = next((i for i in range(first, len(dts)) if dts[i] >= media + (seek + span) * clock), len(dts))
        if end <= first:
            raise ValueError('empty retained run')
        if kind == 'video' and any(c < cts[first] for c in cts[first:end]):
            raise ValueError('leading pictures before the retained access point')
        runs.append(dict(kind=kind, source_index=index, first=first, count=end - first,
                         media_time=int(media + seek * clock - dts[first])))
    return runs


def input_seek_iso(record, operation, movie, plan, output, written):
    if (written['brand'] != b'isom' or written['fragmented'] or len(output['tracks']) != 2
            or [t['handler'] for t in output['tracks']] != ['vide', 'soun']):
        raise ValueError('invalid output layout or track selection')
    problems, extents, durations, presented = [], [], [], 0
    streams = record['detail']['streams']
    if len(streams) != 2 or {s['kind'] for s in streams} != {'video', 'audio'}:
        raise ValueError('invalid summary selection')
    for run, track, extra in zip(plan, output['tracks'], written['tracks']):
        kind = run['kind']
        src = movie['tracks'][run['source_index']]
        dts, cts = input_seek_times(track)
        samples = track['samples']
        found = writer_boxes.compare_samples(kind, record['source'], src, [(record['output'], samples)], run['first'], run['count'])
        found += writer_boxes.compare_entry(kind, src, track) + writer_boxes.compare_timescale(kind, src, track)
        found += writer_boxes.compare_media_duration(kind, track, sum(samples['durations']))
        problems.extend(message for _, message in found)
        if extra['descriptions'] != 1 or set(extra['indices']) != {1} or extra['rates'] != [65536]:
            raise ValueError('invalid output sample descriptions or edit rate')
        span = max(c + d for c, d in zip(cts, samples['durations'])) - run['media_time']
        duration = fractions.Fraction(span * output['movie_timescale'], track['timescale'])
        if (span <= 0 or run['media_time'] < 0 or duration.denominator != 1
                or track['edits'] != [(int(duration), run['media_time'], 1)]
                or track['tkhd_duration'] != duration or extra['volume'] != (0 if kind == 'video' else 256)):
            raise ValueError('output edit does not present the complete retained span')
        durations.append(duration)
        if kind == 'video':
            presented = sum(c >= run['media_time'] for c in cts)
        else:
            expected = b'\0' * 4 + b'roll' + struct.pack('>III', 1, len(dts), 1)
            if extra['groups'] != {b'sgpd': INPUT_SEEK_ROLL, b'sbgp': expected}:
                raise ValueError('invalid audio roll membership')
        summary = next(s for s in streams if s['kind'] == kind)
        for key, value in (('stream_index', run['source_index']), ('first_sample', run['first']), ('samples', run['count'])):
            if summary[key] != value:
                raise ValueError('summary does not describe the independently selected samples')
        extents.extend(zip(samples['offsets'], (o + s for o, s in zip(samples['offsets'], samples['sizes']))))
    extents.sort()
    if (not extents or (extents[0][0], extents[-1][1]) != written['media']
            or any(a[1] != b[0] for a, b in zip(extents, extents[1:]))):
        raise ValueError('sample extents do not tile the media box exactly')
    if output['movie_duration'] != max(durations):
        raise ValueError('movie duration is not its longest track')
    def ratio(value):
        exact = writer_corpus.ratio(value)
        return fractions.Fraction(exact['num'], exact['den'])
    summary = record['detail']
    seek, bound = operation
    if (ratio(summary['start']['snapped']) != seek
            or ratio(summary['end']['written']) != seek + durations[0] / output['movie_timescale']
            or fractions.Fraction(summary['start']['requested']['num'], summary['start']['requested']['den']) != seek
            or fractions.Fraction(summary['end']['requested']['num'], summary['end']['requested']['den']) != seek + bound):
        raise ValueError('summary presentation interval differs from the file and command')
    return problems, presented


def input_seek_snapshot(ffprobe, path):
    result = subprocess.run([ffprobe, '-v', 'error', '-show_streams', '-show_packets', '-show_frames', '-show_format',
                             '-show_data_hash', 'sha256', '-show_entries', INPUT_SEEK_ENTRIES, '-of', 'json', str(path)],
                            capture_output=True, timeout=TIMEOUT, check=False)
    if result.returncode or result.stderr or len(result.stdout) > INPUT_SEEK_MAX_RAW:
        raise ValueError('incomplete pinned snapshot')
    return json.loads(result.stdout)


def input_seek_pin(record, tools, presented, directory):
    import clip_check
    reference = directory / pathlib.Path(record['output']).name
    if pathlib.Path(record['input']).resolve() != pathlib.Path(record['source']).resolve():
        raise ValueError('copy command input differs from the independently checked source')
    args = [record['input'] if a == 'INPUT' else str(reference) if a == 'OUTPUT' else a for a in record['args']]
    result = subprocess.run([tools['ffmpeg'], '-nostdin', '-v', 'error', *AUDIO_DECODE_INPUT, *args],
                            capture_output=True, timeout=TIMEOUT, check=False)
    if result.returncode or result.stderr or not reference.is_file():
        raise ValueError('pinned unchanged copy command failed')
    ours = input_seek_snapshot(tools['ffprobe'], record['output'])
    theirs = input_seek_snapshot(tools['ffprobe'], reference)
    if ours != theirs:
        return ['input-seek complete packet, stream, frame or format records differ from the pin']
    frames = [f for f in theirs.get('packets_and_frames', []) if f['type'] == 'frame' and f['stream_index'] == 0]
    streams = theirs.get('streams', [])
    if (len(streams) != 2 or streams[0].get('codec_name') != 'h264' or streams[1].get('codec_name') != 'aac'
            or streams[1].get('profile') != 'LC' or len(frames) != presented):
        raise ValueError('pinned decoded selection differs from the ISO presentation')
    width, height = int(streams[0]['width']), int(streams[0]['height'])
    pixels = len(frames) * (width * height + 2 * ((width + 1) // 2) * ((height + 1) // 2))
    audio = int(streams[1]['nb_frames']) * 1024 * int(streams[1]['channels']) * 4
    if not 0 < pixels <= INPUT_SEEK_MAX_RAW or not 0 < audio <= INPUT_SEEK_MAX_RAW:
        raise ValueError('input-seek raw decode budget exceeded')
    for kind in ('video', 'audio'):
        raws = []
        for label, path in (('output', record['output']), ('reference', reference)):
            raw = directory / (kind + '-' + label)
            if kind == 'video':
                command = [tools['ffmpeg'], '-nostdin', '-v', 'error', *AUDIO_DECODE_INPUT, '-i', str(path),
                           '-map', '0:v:0', '-an', '-fps_mode', 'passthrough', '-pix_fmt', 'yuv420p', '-f', 'rawvideo', str(raw)]
            else:
                command = [tools['ffmpeg'], '-nostdin', '-v', 'error', *AUDIO_DECODE_INPUT, '-i', str(path),
                           '-map', '0:a:0', '-vn', '-c:a', 'pcm_f32le', '-f', 'f32le', str(raw)]
            result = subprocess.run(command, capture_output=True, timeout=TIMEOUT, check=False)
            if result.returncode or result.stderr or not raw.is_file() or not 0 < raw.stat().st_size <= INPUT_SEEK_MAX_RAW:
                raise ValueError('incomplete or over-budget input-seek decode')
            if kind == 'video' and raw.stat().st_size != pixels:
                raise ValueError('decoded video does not contain every presented frame')
            raws.append(raw)
        if raws[0].stat().st_size != raws[1].stat().st_size or not clip_check.same_bytes(raws[0], 0, raws[1]):
            return [f'input-seek {kind} decoded bytes differ from the pin']
    if tools['mediainfo']:
        ours = clip_check.mediainfo_tracks(tools['mediainfo'], record['output'])
        theirs = clip_check.mediainfo_tracks(tools['mediainfo'], reference)
        if any([t.get('Format') for t in rows if t.get('@type') == 'General'] != ['MPEG-4'] for rows in (ours, theirs)):
            return ['input-seek MediaInfo container format differs']
        for kind, fields in (('Video', clip_check.MEDIAINFO_VIDEO), ('Audio', clip_check.MEDIAINFO_AUDIO)):
            a, b = ([t for t in rows if t.get('@type') == kind] for rows in (ours, theirs))
            if len(a) != 1 or len(b) != 1 or any(a[0].get(k) != b[0].get(k) for k in fields):
                return [f'input-seek {kind} MediaInfo fields differ from the pin']
    return []


def input_seek_problems(record, operation, tools):
    try:
        source_facts = input_seek_facts(record['source'])
        source = track_boxes.read_movie(record['source'])
        plan = input_seek_plan(record, operation, source, source_facts)
        written = input_seek_facts(record['output'])
        output = track_boxes.read_movie(record['output'])
        problems, presented = input_seek_iso(record, operation, source, plan, output, written)
        if problems:
            return problems
        with tempfile.TemporaryDirectory(prefix='copy-check-input-seek-') as tmp:
            return input_seek_pin(record, tools, presented, pathlib.Path(tmp))
    except (ValueError, track_boxes.Unreadable, struct.error) as error:
        return [f'input-seek model: {error}']


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
    operation = input_seek_operation(record)
    if operation is not None:
        return input_seek_problems(record, operation, tools)
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


def timed_check(timings, sequence, check, record, *args):
    if not timings:
        return check(record, *args)
    started = time.monotonic()
    completed = False
    try:
        result = check(record, *args)
        completed = True
        return result
    finally:
        event = {"event": "copy_check_call", "sequence": sequence, "check": check.__name__,
                 "job": record["job"], "seconds": time.monotonic() - started, "completed": completed}
        os.write(sys.stdout.fileno(), (json.dumps(event) + "\n").encode())


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--records", required=True)
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--mediainfo")
    parser.add_argument("--jobs", type=int, default=None)
    parser.add_argument("--report")
    parser.add_argument("--timings", action="store_true", help="log elapsed time per oracle and selection check")
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
        operation = input_seek_operation(record)
        key = tuple(record["args"]) if operation is not None else None
        groups[(record["source"], record["job"], digest(outputs(record)), key)].append(record)
    failures = []
    sequence = os.getpid()
    with concurrent.futures.ProcessPoolExecutor(max_workers=args.jobs) as pool:
        judged = {key: pool.submit(timed_check, args.timings, sequence, oracle, group[0], tools)
                  for key, group in groups.items()
                  if pathlib.Path(key[0]).name not in UNMODELLED.get(key[1], ())}
        mapped = {id(r): pool.submit(timed_check, args.timings, sequence, selection, r, tools["ffmpeg"])
                  for r in written}
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
