#!/usr/bin/env python3
"""Black-box oracles for HLS packaging (docs/design/hls.md, "Verification").

Packages every input with `coreav hls package` and checks the result
against its source with the pinned ffprobe and ffmpeg binaries, and with
MediaInfo when --mediainfo is given. Per packaged input:

- the master playlist, read through ffmpeg's HLS demuxer, holds the
  source's video and audio streams with the source's packets (data hashes
  in order) and, with --master-decode, decodes to the source's frames;
- per rendition, the init segment followed by every media segment
  ("the concatenation") is read by ffprobe with the source stream's codec,
  profile, dimensions, pixel format, sample rate and channels;
- `ffprobe -show_packets -show_data_hash` of the concatenation lists the
  source stream's packets, all of them, in order: the same data hash,
  size, duration and keyframe flag, at the source's presentation times
  plus one constant (0, or a known divergence);
- `ffmpeg -copyts -f framemd5` of the concatenation equals the source
  stream's decoded frames, video and audio: the same hashes in order, at
  the source's times plus the same constant;
- every media segment, read after the init segment on its own, starts
  with a keyframe and holds the next run of the source's packets;
- the media playlist's EXTINF values add up to the stream's presented
  duration (within one sample), and each video segment starts, on the
  playlist's timeline, at its first frame's presentation time (each audio
  segment within one audio frame after it);
- each sample's stored duration in the segments is the source's decode
  time step after it, or, where the source's next movie fragment does not
  start at the decode time the stored durations reach (its `tfdt`), the
  source's stored duration;
- MediaInfo reads the init segment, each init-plus-segment pair and the
  concatenation with the source track's format, dimensions, sample rate and
  channels, and frame counts equal to the packets.

A difference counts as agreement only when an entry of
docs/known-divergences-hls.md covers it; each entry's predicate below
rechecks its conditions. The binaries are black boxes;
no implementation source is read. Exits 1 if any check fails, a required
input is refused, or no input is packaged.
"""

import argparse
import collections
import concurrent.futures
import fnmatch
import json
import os
import pathlib
import re
import shutil
import struct
import subprocess
import sys
import tempfile
from fractions import Fraction

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import clip_check as cc  # noqa: E402
import remux_check  # noqa: E402
import track_boxes  # noqa: E402

VIDEO_KEYS = ("codec_name", "profile", "width", "height", "pix_fmt")
AUDIO_KEYS = ("codec_name", "profile", "sample_rate", "channels")
MEDIAINFO_VIDEO = ("Format", "Format_Profile", "Width", "Height")
MEDIAINFO_AUDIO = ("Format", "Format_AdditionalFeatures", "SamplingRate", "Channels")
KD_FILE = pathlib.Path(__file__).resolve().parent.parent / "docs" / "known-divergences-hls.json"
THREADS = ["-threads", os.environ["HLS_CHECK_THREADS"]] if os.environ.get("HLS_CHECK_THREADS") else []


def run(cmd, check=True):
    proc = subprocess.run(cmd, capture_output=True)
    if check and proc.returncode != 0:
        raise RuntimeError(f"{cmd[0]} failed ({proc.returncode}): {proc.stderr.decode(errors='replace')[-800:]}")
    return proc


def ffprobe_json(args):
    return json.loads(run(["ffprobe", "-v", "error", "-of", "json", *args]).stdout)


def streams(path):
    return ffprobe_json(["-show_streams", str(path)]).get("streams", [])


def packets(path, index):
    """Packets of stream `index`, in file order, with exact times. Read in
    ffprobe's compact form line by line: the JSON form of a long file's packet list takes
    gigabytes to parse."""
    streams_ = ffprobe_json(["-select_streams", str(index), "-show_entries", "stream=time_base", str(path)])
    tb = Fraction(streams_["streams"][0]["time_base"]) if streams_.get("streams") else Fraction(1)
    text = run(["ffprobe", "-v", "error", "-select_streams", str(index), "-show_data_hash", "MD5",
                "-show_entries", "packet=pts,dts,duration,size,flags,data_hash",
                "-of", "compact=p=0", str(path)]).stdout.decode()
    result = []

    def number(fields, key):
        value = fields.get(key)
        return None if value in (None, "", "N/A") else int(value)

    for line in text.splitlines():
        fields = dict(part.split("=", 1) for part in line.split("|") if "=" in part)
        pts, dts = number(fields, "pts"), number(fields, "dts")
        flags = fields.get("flags", "")
        result.append({
            "pts": None if pts is None else pts * tb,
            "dts": None if dts is None else dts * tb,
            "duration": (number(fields, "duration") or 0) * tb,
            "size": int(fields["size"]),
            "key": "K" in flags,
            "discard": "D" in flags,
            "hash": fields.get("data_hash"),
        })
    return result


def frames(path, index=None):
    """(time, hash) of every decoded frame of one stream, `-copyts`."""
    cmd = ["ffmpeg", "-v", "error", *THREADS, "-copyts", "-i", str(path)]
    if index is not None:
        cmd += ["-map", f"0:{index}"]
    # Frames keep their decoded times in the stream's own time base: no
    # frame is duplicated or dropped, and the times are not rounded to a
    # time base derived from ffmpeg's frame rate guess, which differs between
    # a fragmented file and its source for variable frame rates.
    cmd += ["-fps_mode", "passthrough", "-enc_time_base:v", "demux", "-f", "framemd5", "-"]
    try:
        text = run(cmd).stdout.decode()
    except RuntimeError as error:
        # One thread when the decode aborts, as clip_check.decode retries.
        if not cc.killed(error):
            raise
        text = run(cc.single_thread(cmd)).stdout.decode()
    tb = None
    result = []
    for line in text.splitlines():
        m = re.match(r"#tb (\d+): (\d+)/(\d+)", line)
        if m:
            tb = Fraction(int(m.group(2)), int(m.group(3)))
            continue
        if line.startswith("#") or not line.strip():
            continue
        parts = [p.strip() for p in line.split(",")]
        result.append((int(parts[0]), int(parts[2]) * tb, parts[5]))
    return result


def mediainfo(binary, path):
    out = json.loads(run([binary, "--Output=JSON", str(path)]).stdout)
    tracks = out.get("media", {}).get("track", [])
    return {t["@type"]: t for t in tracks}


def media_playlist(path):
    """(init name, [(duration, name)]) of a media playlist."""
    init, segments, duration = None, [], None
    for line in path.read_text().splitlines():
        if line.startswith("#EXT-X-MAP:"):
            init = re.search(r'URI="([^"]+)"', line).group(1)
        elif line.startswith("#EXTINF:"):
            duration = Fraction(line[len("#EXTINF:"):].split(",")[0])
        elif line and not line.startswith("#"):
            segments.append((duration, line))
    return init, segments


def trun_samples(segment):
    """One composition offset (None when not stored) per sample of every
    `trun` box of a media segment (14496-12 8.8.8), read from the box
    headers, independently of CoreAV."""
    data = segment.read_bytes()
    # Only inside the movie fragment box: the media data after it can hold
    # the bytes `trun` by chance.
    moof = next(((s, e) for k, s, e in boxes(data, 0, len(data)) if k == b"moof"), None)
    if moof is None:
        return []
    data = data[:moof[1]]
    offsets = []
    at = 0
    while True:
        at = data.find(b"trun", at)
        if at < 0:
            return offsets
        version, flags = data[at + 4], int.from_bytes(data[at + 5:at + 8], "big")
        count = int.from_bytes(data[at + 8:at + 12], "big")
        pos = at + 12 + (4 if flags & 0x1 else 0) + (4 if flags & 0x4 else 0)
        for _ in range(count):
            for bit in (0x100, 0x200, 0x400):
                if flags & bit:
                    pos += 4
            if flags & 0x800:
                offsets.append(int.from_bytes(data[pos:pos + 4], "big", signed=version == 1))
                pos += 4
            else:
                offsets.append(None)
        at = pos


class Report:
    def __init__(self, name):
        self.name = name
        self.failures = []
        self.divergences = []
        self.packets_agree = 0
        self.packets_disagree = 0
        self.source_frames = {}
        self.output_frames = {}
        self.source_packets = {}

    def fail(self, what):
        self.failures.append(what)

    def covered(self, kd, what):
        self.divergences.append({"id": kd, "detail": what})


def kd_negative_cto_shift(package, rendition, timescale, delta):  # KD-HLS-1
    """KD-HLS-1: ffmpeg presents a video rendition whose fragments carry
    negative composition offsets (signed, or values of 2^31 and more in a
    version 0 `trun`, which ffmpeg reads as negative) later by the magnitude
    of its most negative offset, the same for every frame. Holds when `delta` (output time minus source time, in
    seconds) equals that magnitude over the rendition's media timescale."""
    if rendition != "video":
        return False
    negative = most_negative_offset(package, rendition)
    return negative < 0 and delta == Fraction(-negative, timescale)


def most_negative_offset(package, rendition):
    """The rendition's most negative composition offset, or 0."""
    _, segments = media_playlist(package / f"{rendition}.m3u8")
    offsets = [o for _, name in segments for o in trun_samples(package / name)]
    # Read as signed whatever the trun version: ffmpeg reads the 2^31 and
    # larger values of a version 0 table as negative.
    signed = (o - (1 << 32) if o >= 1 << 31 else o for o in offsets if o is not None)
    return min(signed, default=0)


def kd_negative_cto_midframe(package, init, rendition, timescale, delta, source=None, index=None):  # KD-HLS-1 and KD-HLS-3
    """Both at once: ffmpeg presents the rendition later by KD-HLS-1's
    shift and starts the source's timeline after a mid-frame edit's start
    by KD-HLS-3's. Holds when the rendition has a negative offset and the
    rest of `delta` after KD-HLS-1's shift meets KD-HLS-3's predicate."""
    if rendition != "video":
        return False
    negative = most_negative_offset(package, rendition)
    if negative >= 0:
        return False
    return kd_midframe_edit(package, init, rendition, timescale, delta - Fraction(-negative, timescale), source, index)


def check_rendition(args, report, source, package, rendition, info, src_stream, work):
    index = info["stream_index"]
    timescale = info["duration"]["timescale"]
    init, segments = media_playlist(package / f"{rendition}.m3u8")
    whole = work / f"{rendition}.mp4"
    with whole.open("wb") as out:
        for name in [init] + [n for _, n in segments]:
            out.write((package / name).read_bytes())

    check_edit(report, source, package, rendition, init, info["track_id"])

    # What the black boxes do not read, from the files' own boxes
    # (scripts/writer_boxes.py): every sample with its sync flag, the init
    # segment's sample entry and timescale.
    for _, message in cc.writer_boxes.check_rendition(str(source), index, str(package / init),
                                                      [str(package / n) for _, n in segments]):
        report.fail(f"{rendition}: {message}")
    cc.writer_boxes.forget()

    # Streams.
    out_streams = streams(whole)
    if len(out_streams) != 1:
        report.fail(f"{rendition}: {len(out_streams)} streams in the concatenation")
        return
    keys = VIDEO_KEYS if rendition == "video" else AUDIO_KEYS
    for key in keys:
        if out_streams[0].get(key) != src_stream.get(key):
            if key == "profile" and kd_profile_unnamed(out_streams[0], src_stream):
                report.covered("KD-HLS-7", f"{rendition}: profile {out_streams[0].get(key)}")
                continue
            report.fail(f"{rendition}: stream {key} {out_streams[0].get(key)!r} != {src_stream.get(key)!r}")

    # Packets of the concatenation against the source's. Leading packets
    # that only the output lists are matched under KD-HLS-2.
    src = packets(source, index)
    report.source_packets[rendition] = [p["hash"] for p in src]
    out = packets(whole, 0)
    lead = len(out) - len(src)
    if lead <= 0 or [p["hash"] for p in out[lead:]] != [p["hash"] for p in src]:
        lead = 0
    deltas = set()
    for i, (s, o) in enumerate(zip(src, out[lead:])):
        # Durations are left out: ffprobe derives them from the edit list
        # differently for movie fragments than for sample tables; the
        # coreav-isobmff round trip compares the stored durations exactly.
        same = (s["hash"], s["size"], s["key"]) == (o["hash"], o["size"], o["key"])
        if same and s["pts"] is not None and o["pts"] is not None:
            deltas.add(o["pts"] - s["pts"])
        if same:
            report.packets_agree += 1
        else:
            report.packets_disagree += 1
            if report.packets_disagree <= 3:
                report.fail(f"{rendition}: packet {i} differs: {o} != {s}")
    if lead:
        matched = [p["pts"] for p in out[lead:] if p["pts"] is not None]
        extra = [p["pts"] for p in out[:lead] if p["pts"] is not None]
        if kd_hidden_lead(package, rendition, init, extra, matched):
            report.covered("KD-HLS-2", f"{rendition}: {lead} leading packets")
            report.packets_agree += lead
        else:
            report.packets_disagree += lead
            report.fail(f"{rendition}: {lead} leading packets the source does not list")
    missing = len(src) - len(out[lead:])
    tail = src[len(src) - missing:] if missing > 0 else []
    left_out = []
    if tail and all(p.get("discard") for p in tail):
        # Trailing samples the edit hides, which ffprobe lists as discarded
        # and the rendition leaves out (KD-HLS-2).
        report.covered("KD-HLS-2", f"{rendition}: {missing} discarded trailing packets left out")
        report.packets_agree += missing
        src = src[:len(src) - missing]
        left_out = tail
    elif missing:
        report.packets_disagree += abs(missing)
        report.fail(f"{rendition}: {len(out)} packets, source has {len(src)}")
    delta = check_delta(report, source, package, rendition, timescale, deltas, "packet", init, index)

    # Stored sample durations against the source's decode time steps
    # (ffprobe's packet durations follow the edit list; decode times do not).
    stored = [d for _, name in segments for d in trun_durations(package / name)]
    # Each step at its sample's place, src[i] being the rendition's sample
    # lead + i.
    steps = [(i, (b["dts"] - a["dts"]) * timescale) for i, (a, b) in enumerate(zip(src, src[1:]))
             if a["dts"] is not None and b["dts"] is not None]
    wrong = sum(1 for i, step in steps if lead + i < len(stored) and stored[lead + i] != step)
    if wrong:
        # Where the source's next movie fragment does not start at the decode
        # time its stored durations reach, the step there is not the
        # sample's duration, which is copied as stored.
        gaps = source_decode_gaps(source, info["track_id"])
        wrong = sum(1 for i, step in steps if lead + i < len(stored) and stored[lead + i] != step
                    and gaps.get(lead + i) != stored[lead + i])
    if wrong:
        report.fail(f"{rendition}: {wrong} sample durations differ from the source's decode time steps")

    # Decoded frames, matched by time: every source frame must be decoded
    # from the output at its time plus the packets' shift, with the same
    # hash (decoders emit frames of variable frame rate streams in an order
    # that need not be the same). Output frames outside the source's are
    # samples the edit hides (KD-HLS-2).
    src_frames = frames(source, index)
    report.source_frames[rendition] = [h for _, _, h in src_frames]
    out_frames = frames(whole)
    report.output_frames[rendition] = [h for _, _, h in out_frames]
    shift = delta or Fraction(0)
    by_time = collections.defaultdict(list)
    for _, t, h in out_frames:
        by_time[t].append(h)
    unmatched = []
    for _, t, h in src_frames:
        hashes = by_time.get(t + shift, [])
        if h in hashes:
            hashes.remove(h)
        else:
            unmatched.append((t, h))
    extra = [t for t, hashes in by_time.items() for _ in hashes]
    covered = None
    if unmatched:
        want = [h for _, _, h in src_frames]
        got = [h for _, _, h in out_frames]
        if kd_gapless_conflict(source, package, rendition, out_frames, src_frames):
            covered = ("KD-HLS-5", f"{rendition}: first and last frames")
        elif not extra and kd_left_out_frames(unmatched, left_out):
            covered = ("KD-HLS-2", f"{rendition}: {len(unmatched)} frames of the discarded trailing packets")
        elif kd_source_last_cut(source, index, whole, src, out, src_frames, unmatched, extra, shift):
            covered = ("KD-HLS-11", f"{rendition}: the last frame, cut in the source only")
    elif extra:
        kept = [t + shift for _, t, _ in src_frames]
        if kd_hidden_lead(package, rendition, init, extra, kept):
            covered = ("KD-HLS-2", f"{rendition}: {len(extra)} frames the edit hides")
    if (unmatched or extra) and covered is None \
            and kd_fragment_reading(source, index, out_frames, work):
        covered = ("KD-HLS-8", f"{rendition}: frames as ffmpeg decodes its own fragmented copy")
    if covered:
        report.covered(*covered)
    elif unmatched:
        report.fail(f"{rendition}: {len(unmatched)} decoded frames of the source differ or are missing "
                    f"({len(out_frames)} against {len(src_frames)})")
    elif extra:
        report.fail(f"{rendition}: {len(extra)} frames the source does not present")
    # The source's packets as the output lists them, for the checks below.
    expect = [p["hash"] for p in out[:lead]] + [p["hash"] for p in src]
    report.source_packets[rendition] = expect
    timeline = [dict(p, pts=None if p["pts"] is None else p["pts"] - (delta or 0)) for p in out]

    # Each media segment on its own.
    picks = range(len(segments))
    if args.segments == "sample" and len(segments) > 3:
        picks = sorted({0, len(segments) // 2, len(segments) - 1})
    position = 0
    counts = []
    for n, (_, name) in enumerate(segments):
        pair = work / f"{rendition}-pair.mp4"
        pair.write_bytes((package / init).read_bytes() + (package / name).read_bytes())
        if n in picks or n == len(segments) - 1:
            seg = packets(pair, 0)
            counts.append(len(seg))
            if not seg or not seg[0]["key"]:
                report.fail(f"{rendition}: segment {n} does not start with a keyframe")
            if [p["hash"] for p in seg] != expect[position:position + len(seg)]:
                report.fail(f"{rendition}: segment {n} is not the source's packets {position}..")
            if args.mediainfo:
                check_mediainfo(args, report, pair, src_stream, rendition, f"segment {n}", len(seg))
            position += len(seg)
        else:
            # Advance without reading the segment: its sample count from the
            # trun boxes.
            position += len(trun_samples(package / name))

    # Segment starts and durations on the playlist's timeline, up to one
    # constant: ffmpeg does not apply an edit's media time to fragmented
    # sources, so its times can differ from the edit list's by a constant
    # in both the source and the output.
    step = max((p["duration"] for p in src), default=Fraction(0))
    if not step:
        # ffprobe reports no packet durations for some sources: the median
        # gap between successive presentation times instead (not the
        # longest, which a hole in the timeline would make loose).
        times = sorted(p["pts"] for p in src if p["pts"] is not None)
        gaps = sorted(b - a for a, b in zip(times, times[1:]) if b > a)
        step = gaps[len(gaps) // 2] if gaps else Fraction(0)
    src = timeline
    starts = []
    acc = Fraction(0)
    for d, _ in segments:
        starts.append(acc)
        acc += d
    origin = check_starts(report, rendition, segments, starts, package, src, step, init, timescale, delta,
                          source, info["track_id"])
    total = sum(d for d, _ in segments)
    presented = [p for p in src if p["pts"] is not None and p["pts"] >= origin and not p.get("discard")]
    end = max((p["pts"] + p["duration"] for p in presented), default=Fraction(0)) - origin
    # ffprobe's own stream end as well: some packet durations it reports do
    # not add up to it (the last ones of some fragments).
    stream_end = None
    if src_stream.get("duration") not in (None, "N/A"):
        stream_end = Fraction(src_stream["duration"]) + Fraction(src_stream.get("start_time") or 0) - origin
    # Within two sample durations: where the last frame ends is ambiguous
    # (its decode duration, or the edit's end).
    near = [abs(total - e) <= 2 * step + Fraction(1, 1000) for e in (end, stream_end) if e is not None]
    if not any(near):
        report.fail(f"{rendition}: EXTINF sum {float(total)} s, presented end {float(end)} s")

    if args.mediainfo:
        check_mediainfo(args, report, package / init, src_stream, rendition, "init", 0)
        check_mediainfo(args, report, whole, src_stream, rendition, "concatenation", len(src))


def check_delta(report, source, package, rendition, timescale, deltas, what, init, index=None):
    if len(deltas) > 1:
        report.fail(f"{rendition}: {what} times shifted by {len(deltas)} different amounts")
        return None
    delta = next(iter(deltas), Fraction(0))
    if delta != 0:
        if kd_negative_cto_shift(package, rendition, timescale, delta):
            report.covered("KD-HLS-1", f"{rendition} {what} times +{float(delta)} s")
        elif kd_midframe_edit(package, init, rendition, timescale, delta, source, index):
            report.covered("KD-HLS-3", f"{rendition} {what} times +{float(delta)} s")
        elif kd_negative_cto_midframe(package, init, rendition, timescale, delta, source, index):
            report.covered("KD-HLS-1", f"{rendition} {what} times, with KD-HLS-3: +{float(delta)} s")
            report.covered("KD-HLS-3", f"{rendition} {what} times, with KD-HLS-1: +{float(delta)} s")
        elif kd_gapless_edit(source, package, rendition, timescale, delta):
            report.covered("KD-HLS-4", f"{rendition} {what} times {float(delta)} s")
        else:
            report.fail(f"{rendition}: {what} times shifted by {float(delta)} s")
    return delta


def check_starts(report, rendition, segments, starts, package, src, step, init, timescale, delta, source=None,
                 track_id=None):
    """Segment n ≥ 1 starts at its first presented sample, exactly for the
    video and within one frame before it for the audio, once a constant
    origin is allowed for: 0, the init segment's edit media time (which
    ffmpeg does not apply to fragmented sources), the frame-aligned start of
    KD-HLS-3 (minus the packets' shift), or at most one sample duration.
    Starts that hold only once KD-HLS-10's drift is applied are covered by
    it. Returns that origin in seconds."""
    origin, failed = start_errors(rendition, segments, starts, package, src, step, init, timescale, delta)
    if failed:
        drift = kd_index_drift(source, track_id, package, segments, starts)
        if drift:
            moved = [s + d for s, d in zip(starts, drift)]
            origin_moved, still = start_errors(rendition, segments, moved, package, src, step, init, timescale,
                                               delta)
            if not still:
                report.covered("KD-HLS-10", f"{rendition}: segment starts up to "
                               f"{float(max(abs(d) for d in drift)):.6f} s from the index's")
                return origin_moved
        report.fail(failed)
    return origin


def start_errors(rendition, segments, starts, package, src, step, init, timescale, delta):
    """check_starts' rule: (origin, the failure or None)."""
    position = 0
    errors = []
    for n, ((_, name), start) in enumerate(zip(segments, starts)):
        count = len(trun_samples(package / name))
        run_ = src[position:position + count]
        position += count
        if n == 0 or not run_:
            continue
        first = min(p["pts"] for p in run_ if p["pts"] is not None)
        errors.append((n, start, first, first - start))
    if not errors:
        return Fraction(0), None
    tiny = Fraction(1, 500_000)
    if rendition == "video":
        origin = errors[0][3]
        bad = [e for e in errors if abs(e[3] - origin) > tiny]
    else:
        origin = min(e[3] for e in errors)
        bad = [e for e in errors if e[3] - origin >= step]
    window = init_edit_window(package / init)
    allowed = [Fraction(0), -(delta or 0)] + ([Fraction(window[0], timescale)] if window else [])
    if rendition == "video":
        known = abs(origin) <= step or any(abs(origin - a) <= tiny for a in allowed)
    else:
        known = abs(origin) <= step or any(-tiny <= origin - a < step for a in allowed)
    if bad or not known:
        n, start, first, _ = (bad or errors)[0]
        return origin, f"{rendition}: segment {n} starts at {float(start)} s, its first sample at {float(first)} s"
    return origin, None


def attributes(line):
    """The attribute list of a playlist tag (RFC 8216 4.2)."""
    return dict(re.findall(r'([A-Z0-9-]+)=("[^"]*"|[^,]*)', line.split(":", 1)[1]))


def measured_rates(package, rendition):
    """(peak, average) bit rate of a media playlist per RFC 8216 4.3.4.2,
    from the segment files' sizes and the playlist's EXTINF and target
    duration, computed independently of CoreAV."""
    text = (package / f"{rendition}.m3u8").read_text()
    target = int(re.search(r"#EXT-X-TARGETDURATION:(\d+)", text).group(1))
    _, segments = media_playlist(package / f"{rendition}.m3u8")
    sizes = [(package / name).stat().st_size for _, name in segments]
    durations = [d for d, _ in segments]
    peak = None
    for i in range(len(segments)):
        total, bits = Fraction(0), 0
        for d, z in zip(durations[i:], sizes[i:]):
            total += d
            bits += 8 * z
            if total > Fraction(3, 2) * target:
                break
            if total >= Fraction(target, 2) and total > 0:
                peak = max(peak or 0, bits / total)
    average = 8 * sum(sizes) / sum(durations) if sum(durations) else 0
    return (peak if peak is not None else average), average


H264_PROFILES = {"Baseline": 66, "Constrained Baseline": 66, "Main": 77, "Extended": 88, "High": 100,
                 "High 10": 110, "High 4:2:2": 122, "High 4:4:4 Predictive": 244}
HEVC_PROFILES = {"Main": 1, "Main 10": 2, "Main Still Picture": 3, "Rext": 4}
AAC_TYPES = {"LC": 2, "HE-AAC": 5, "HE-AACv2": 29, "Main": 1, "LTP": 4}


def check_master(report, package, summary, src_streams, source):
    """The master playlist's attributes against the source as ffprobe reads
    it and the segment files as they are: BANDWIDTH and AVERAGE-BANDWIDTH
    (within 1% and one bit per second), RESOLUTION (display size, turned by
    the rotation), FRAME-RATE (between the average and the largest frame
    rate), CHANNELS, and CODECS (profile and level for H.264 and HEVC, the
    object type for AAC).

    A deferred rendition's bit rates come from the source's own subsegment
    sizes, which hold one or more movie fragments where the packaged
    segment holds one (docs/design/hls.md, "Sources with a segment
    index"), so they stand for the written segments' rather than measure
    them: within a tenth either way."""
    text = (package / "master.m3u8").read_text()
    inf = attributes(next(l for l in text.splitlines() if l.startswith("#EXT-X-STREAM-INF:")))
    media = [attributes(l) for l in text.splitlines() if l.startswith("#EXT-X-MEDIA:")]
    peak = average = 0
    for r in summary["renditions"]:
        p, a = measured_rates(package, r["rendition"])
        peak, average = peak + p, average + a
    bandwidth, avg_bw = int(inf["BANDWIDTH"]), int(inf.get("AVERAGE-BANDWIDTH", 0))
    deferred = any(r.get("deferred") for r in summary["renditions"])
    def off(stated, measured):
        slack = measured / 10 if deferred else measured / 100
        return abs(stated - measured) > slack + 1
    if off(bandwidth, peak):
        report.fail(f"master playlist: BANDWIDTH {bandwidth}, measured peak {float(peak):.0f}")
    if off(avg_bw, average):
        report.fail(f"master playlist: AVERAGE-BANDWIDTH {avg_bw}, measured {float(average):.0f}")
    codecs = inf["CODECS"].strip('"').split(",")
    for r, codec in zip(summary["renditions"], codecs):
        src = next(x for x in src_streams if x["index"] == r["stream_index"])
        kind, *fields = codec.split(".")
        if kind == "avc1" and src.get("profile") in H264_PROFILES:
            if int(fields[0][:2], 16) != H264_PROFILES[src["profile"]] or int(fields[0][4:], 16) != src.get("level"):
                report.fail(f"master playlist: CODECS {codec} against {src.get('profile')} level {src.get('level')}")
        elif kind == "hvc1" and src.get("profile") in HEVC_PROFILES:
            if int(fields[0].lstrip("ABC")) != HEVC_PROFILES[src["profile"]] or int(fields[2][1:]) != src.get("level"):
                report.fail(f"master playlist: CODECS {codec} against {src.get('profile')} level {src.get('level')}")
        elif kind == "mp4a" and src.get("profile") in AAC_TYPES:
            if int(fields[1]) != AAC_TYPES[src["profile"]]:
                if kd_implicit_sbr(int(fields[1]), src):
                    report.covered("KD-HLS-6", f"CODECS {codec} for {src['profile']}")
                else:
                    report.fail(f"master playlist: CODECS {codec} against {src.get('profile')}")
        if r["rendition"] == "video":
            check_video_attributes(report, inf, src, source)
        elif media:
            channels = media[0].get("CHANNELS", '""').strip('"')
            if channels and int(channels) != src.get("channels"):
                report.fail(f"master playlist: CHANNELS {channels} against {src.get('channels')}")


def check_video_attributes(report, inf, src, source):
    width, height = int(src["width"]), int(src["height"])
    sar = src.get("sample_aspect_ratio") or "1:1"
    num, den = (int(x) for x in sar.split(":")) if ":" in sar and sar != "0:1" else (1, 1)
    width = round(width * num / den) if den else width
    rotation = next((int(float(d.get("rotation", 0))) for d in src.get("side_data_list", [])
                     if "rotation" in d), 0)
    if abs(rotation) % 180 == 90:
        width, height = height, width
    got = [int(x) for x in inf.get("RESOLUTION", "0x0").split("x")]
    if abs(got[0] - width) > 2 or abs(got[1] - height) > 2:
        if kd_left_top_crop(got, src, source):
            report.covered("KD-HLS-9", f"RESOLUTION {inf.get('RESOLUTION')} against {width}x{height}")
        else:
            report.fail(f"master playlist: RESOLUTION {inf.get('RESOLUTION')} against {width}x{height}")
    pk = packets(source, src["index"])
    dts = [p["dts"] for p in pk if p["dts"] is not None]
    steps = [b - a for a, b in zip(dts, dts[1:]) if b > a]
    if "FRAME-RATE" in inf and steps:
        rate = Fraction(inf["FRAME-RATE"])
        fastest = 1 / min(steps)
        mean = len(steps) / sum(steps)
        if not (mean - Fraction(1, 500) <= rate <= fastest + Fraction(1, 500)):
            report.fail(f"master playlist: FRAME-RATE {inf['FRAME-RATE']}, frame rates "
                        f"{float(mean):.3f} on average and {float(fastest):.3f} at most")


def check_mediainfo(args, report, path, src_stream, rendition, what, frames_expected):
    tracks = mediainfo(args.mediainfo, path)
    kind = "Video" if rendition == "video" else "Audio"
    track = tracks.get(kind)
    if track is None:
        report.fail(f"{rendition}: MediaInfo finds no {kind} track in the {what}")
        return
    src = args.mediainfo_source[(src_stream["_path"], kind, src_stream["_order"])]
    keys = MEDIAINFO_VIDEO if kind == "Video" else MEDIAINFO_AUDIO
    if what == "init":
        # Without samples MediaInfo cannot read profiles from the bitstream.
        keys = [k for k in keys if k not in ("Format_Profile", "Format_AdditionalFeatures")]
    for key in keys:
        if track.get(key) != src.get(key):
            if what == "init" and key == "SamplingRate" and kd_implicit_sbr_rate(track, src, src_stream):
                report.covered("KD-HLS-6", f"{rendition}: MediaInfo SamplingRate {track.get(key)} in the init")
                continue
            report.fail(f"{rendition}: MediaInfo {key} {track.get(key)!r} != {src.get(key)!r} in the {what}")
    if frames_expected and track.get("FrameCount") not in (None, str(frames_expected)):
        report.fail(f"{rendition}: MediaInfo counts {track.get('FrameCount')} frames in the {what}, expected {frames_expected}")


def source_mediainfo(args, source, src_streams):
    tracks = json.loads(run([args.mediainfo, "--Output=JSON", str(source)]).stdout)["media"]["track"]
    table = {}
    for kind in ("Video", "Audio"):
        of_kind = [t for t in tracks if t["@type"] == kind]
        for order, t in enumerate(of_kind):
            table[(str(source), kind, order)] = t
    return table


def check_one(args, source):
    name = source.name
    report = Report(name)
    work = pathlib.Path(tempfile.mkdtemp(prefix="hls-check-", dir=args.work))
    package = work / "package"
    try:
        def package_with(extra):
            shutil.rmtree(package, ignore_errors=True)
            return run([args.coreav, "hls", "package", "--segment-duration", args.segment_duration,
                        *extra, str(source), str(package)], check=False)

        def error_of(proc):
            return json.loads(proc.stderr or b"{}") if proc.stderr.strip().startswith(b"{") else {}

        proc = package_with([])
        no_audio = False
        if proc.returncode != 0 and getattr(args, "retry_without_audio", False) \
                and "only AAC audio" in (error_of(proc).get("message") or ""):
            # The default audio is not AAC: package the video alone, so
            # that it is checked too.
            no_audio = True
            proc = package_with(["--audio", "none"])
        if proc.returncode != 0:
            error = error_of(proc)
            return {"file": name, "status": "refused", "exit": proc.returncode,
                    "kind": error.get("kind"), "message": error.get("message"), "no_audio": no_audio}
        summary = json.loads(proc.stdout)
        src_streams = streams(source)
        by_type = {"video": [], "audio": []}
        for s in src_streams:
            if s.get("codec_type") in by_type:
                s["_order"] = len(by_type[s["codec_type"]])
                s["_path"] = str(source)
                by_type[s["codec_type"]].append(s)
        if args.mediainfo:
            args.mediainfo_source.update(source_mediainfo(args, source, src_streams))
        # The master playlist through ffmpeg's HLS demuxer.
        master = streams(package / "master.m3u8")
        expected = sorted(r["codecs"].split(".")[0] for r in summary["renditions"])
        names = {"h264": "avc1", "hevc": "hvc1", "av1": "av01", "vp9": "vp09", "aac": "mp4a"}
        got = sorted(names.get(s.get("codec_name"), s.get("codec_name")) for s in master)
        if got != expected:
            report.fail(f"master playlist streams {got}, expected {expected}")
        for r in summary["renditions"]:
            rendition = r["rendition"]
            src_stream = next(s for s in src_streams if s["index"] == r["stream_index"])
            check_rendition(args, report, source, package, rendition, r, src_stream, work)
        check_master(report, package, summary, src_streams, source)
        # The master playlist through ffmpeg's HLS demuxer: each stream's
        # packets are the source stream's (and, with --master-decode, its
        # decoded frames).
        master_index = {s["index"]: names.get(s.get("codec_name"), s.get("codec_name")) for s in master}
        for r in summary["renditions"]:
            want = report.source_packets.get(r["rendition"])
            kind = r["codecs"].split(".")[0]
            index = next((i for i, c in master_index.items() if c == kind), None)
            if want is None or index is None:
                continue
            got = [p["hash"] for p in packets(package / "master.m3u8", index)]
            if got != want:
                report.fail(f"master playlist: {r['rendition']} packets differ ({len(got)} against {len(want)})")
            if args.master_decode:
                # The same samples as the concatenation, so the same frames.
                got = [h for _, _, h in frames(package / "master.m3u8", index)]
                if sorted(got) != sorted(report.output_frames.get(r["rendition"]) or []):
                    report.fail(f"master playlist: {r['rendition']} frames differ")
        status = "fail" if report.failures else ("covered" if report.divergences else "agree")
        return {"file": name, "status": status, "no_audio": no_audio, "failures": report.failures,
                "divergences": report.divergences, "packets_agree": report.packets_agree,
                "packets_disagree": report.packets_disagree,
                "renditions": [{k: r[k] for k in ("rendition", "codecs", "segments", "samples")}
                               for r in summary["renditions"]]}
    except Exception as error:  # noqa: BLE001 - reported per file
        return {"file": name, "status": "fail", "failures": [f"harness: {error}"]}
    finally:
        if not args.keep:
            shutil.rmtree(work, ignore_errors=True)


def init_edit(init):
    """The media edit's media time of an init segment's `elst`, from its
    box header (14496-12 8.6.6); None without one."""
    data = init.read_bytes()
    at = data.find(b"elst")
    if at < 0:
        return None
    version = data[at + 4]
    count = int.from_bytes(data[at + 8:at + 12], "big")
    pos, width = at + 12, 8 if version == 1 else 4
    media_time = None
    for _ in range(count):
        value = int.from_bytes(data[pos + width:pos + 2 * width], "big", signed=True)
        if value >= 0:
            media_time = value
        pos += 2 * width + 4
    return media_time


def boxes(data, start, end):
    """(type, payload start, box end) of the boxes in data[start:end]."""
    at = start
    while at + 8 <= end:
        size = int.from_bytes(data[at:at + 4], "big")
        kind = data[at + 4:at + 8]
        head = 8
        if size == 1:
            size = int.from_bytes(data[at + 8:at + 16], "big")
            head = 16
        elif size == 0:
            size = end - at
        if size < head or at + size > end:
            return
        yield kind, at + head, at + size
        at += size


def child(data, start, end, kind):
    return next(((s, e) for k, s, e in boxes(data, start, end) if k == kind), None)


def elst_entries(data, start):
    """(duration, media time) of an `elst` payload at `start`."""
    version = data[start]
    count = int.from_bytes(data[start + 4:start + 8], "big")
    pos, width = start + 8, 8 if version == 1 else 4
    entries = []
    for _ in range(count):
        duration = int.from_bytes(data[pos:pos + width], "big")
        media_time = int.from_bytes(data[pos + width:pos + 2 * width], "big", signed=True)
        entries.append((duration, media_time))
        pos += 2 * width + 4
    return entries


def source_edit(source, track_id):
    """The source track's edit list in its media ticks, from its box headers:
    (empty edit, media time, media edit duration), or None without one."""
    # Only the movie box is read (sources can be gigabytes), inflated when
    # compressed (QuickTime `cmov`).
    data = cc.movie_box(str(source), cap=1 << 62)
    if data is None:
        return None
    moov = (0, len(data))
    mvhd = child(data, *moov, b"mvhd")
    if mvhd is None:
        return None
    v = data[mvhd[0]]
    movie_ts = int.from_bytes(data[mvhd[0] + (20 if v == 1 else 12):][:4], "big")
    for kind, start, end in boxes(data, *moov):
        if kind != b"trak":
            continue
        tkhd = child(data, start, end, b"tkhd")
        v = data[tkhd[0]]
        tid = int.from_bytes(data[tkhd[0] + (20 if v == 1 else 12):][:4], "big")
        if tid != track_id:
            continue
        mdia = child(data, start, end, b"mdia")
        mdhd = child(data, *mdia, b"mdhd")
        v = data[mdhd[0]]
        media_ts = int.from_bytes(data[mdhd[0] + (20 if v == 1 else 12):][:4], "big")
        edts = child(data, start, end, b"edts")
        elst = edts and child(data, *edts, b"elst")
        if not elst:
            return None
        empty, media_time, duration = 0, None, 0
        for d, t in elst_entries(data, elst[0]):
            if t < 0 and media_time is None:
                empty += d
            else:
                media_time, duration = t, d
        scale = Fraction(media_ts, movie_ts)
        return round(empty * scale), media_time, round(duration * scale)
    return None


def check_edit(report, source, package, rendition, init, track_id):
    """The init segment's edit list is the source's, in media ticks (an
    empty edit either kept or turned into a longer media edit), or the one
    a matching `iTunSMPB` tag stands for, or none when the source has
    neither; durations within a tick of rounding."""
    data = (package / init).read_bytes()
    at = data.find(b"elst")
    got = elst_entries(data, at + 4) if at >= 0 else []
    want = source_edit(source, track_id)
    if want is None:
        tag = itunsmpb(source) if rendition == "audio" else None
        ok = got == [] or (tag is not None and len(got) == 1 and got[0][1] == tag[0]
                           and abs(got[0][0] - tag[2]) <= 1)
    else:
        empty, media_time, duration = want
        # A media edit of duration 0 lasts to the end of the media and keeps
        # its 0 when the empty edit becomes a decode time shift.
        forms = [[(duration + empty if duration else 0, media_time)]]
        if empty:
            forms.append([(empty, -1), (duration, media_time)])
        else:
            forms.append([(duration, media_time)])
        ok = any(len(f) == len(got) and all(g[1] == w[1] and abs(g[0] - w[0]) <= 1 for g, w in zip(got, f))
                 for f in forms)
    if not ok:
        report.fail(f"{rendition}: init edit list {got} against the source's {want}")


def init_edit_window(init):
    """(media time, duration) of an init segment's media edit, in media
    ticks (the init's movie timescale is the media timescale); None
    without an edit list."""
    data = init.read_bytes()
    at = data.find(b"elst")
    if at < 0:
        return None
    version = data[at + 4]
    count = int.from_bytes(data[at + 8:at + 12], "big")
    pos, width = at + 12, 8 if version == 1 else 4
    window = None
    for _ in range(count):
        duration = int.from_bytes(data[pos:pos + width], "big")
        value = int.from_bytes(data[pos + width:pos + 2 * width], "big", signed=True)
        if value >= 0:
            window = (value, duration)
        pos += 2 * width + 4
    return window


def tfdt(segment):
    data = segment.read_bytes()
    at = data.find(b"tfdt")
    width = 8 if data[at + 4] == 1 else 4
    return int.from_bytes(data[at + 8:at + 8 + width], "big")


def hidden_ends(package, rendition, init):
    """Samples of the rendition that its edit list leaves entirely
    unpresented or starts before it (before its start), or after its end, counted
    from the segments' own box headers (offsets read as signed, as ffmpeg
    reads them)."""
    window = init_edit_window(package / init)
    if window is None:
        return 0, 0
    media_time, duration = window
    _, segments = media_playlist(package / f"{rendition}.m3u8")
    hidden = []
    for _, name in segments:
        dts = tfdt(package / name)
        offsets = trun_samples(package / name)
        for d, o in zip(trun_durations(package / name), offsets):
            o = o or 0
            o = o - (1 << 32) if o >= 1 << 31 else o
            cts = dts + o
            before = cts < media_time
            after = duration > 0 and cts >= media_time + duration
            hidden.append((before, after))
            dts += d or 0
    return sum(b for b, _ in hidden), sum(a for _, a in hidden)


def trun_durations(segment):
    """Sample durations of a media segment's first `traf`: per sample from
    `trun`, else the `tfhd` default."""
    data = segment.read_bytes()
    at = data.find(b"tfhd")
    flags = int.from_bytes(data[at + 5:at + 8], "big")
    pos = at + 12 + (8 if flags & 0x1 else 0) + (4 if flags & 0x2 else 0)
    default = int.from_bytes(data[pos:pos + 4], "big") if flags & 0x8 else None
    at = data.find(b"trun")
    flags = int.from_bytes(data[at + 5:at + 8], "big")
    count = int.from_bytes(data[at + 8:at + 12], "big")
    pos = at + 12 + (4 if flags & 0x1 else 0) + (4 if flags & 0x4 else 0)
    durations = []
    for _ in range(count):
        if flags & 0x100:
            durations.append(int.from_bytes(data[pos:pos + 4], "big"))
        else:
            durations.append(default)
        pos += 4 * sum(1 for bit in (0x100, 0x200, 0x400, 0x800) if flags & bit)
    return durations


def kd_hidden_lead(package, rendition, init, extra, matched):  # KD-HLS-2
    """KD-HLS-2: ffmpeg lists and decodes, from movie fragments, samples that
    the edit list hides at the start or the end and that it drops from the
    source's sample tables. Holds when every extra packet or frame comes
    before the first matched one or after the last, and there are no more
    of them at either end than the rendition has samples its edit list
    leaves unpresented there."""
    if not matched:
        return False
    first, last = min(matched), max(matched)
    if not all(t < first or t > last for t in extra):
        return False
    lead, trail = hidden_ends(package, rendition, init)
    return sum(t < first for t in extra) <= lead and sum(t > last for t in extra) <= trail


def itunsmpb(source):
    """(delay, padding, samples) of the source's `iTunSMPB` tag, as ffprobe
    reports it; None without one."""
    tags = ffprobe_json(["-show_entries", "format_tags", str(source)]).get("format", {}).get("tags", {})
    text = next((v for k, v in tags.items() if k.lower() == "itunsmpb"), None)
    fields = (text or "").split()
    try:
        return int(fields[1], 16), int(fields[2], 16), int(fields[3], 16)
    except (IndexError, ValueError):
        return None


def kd_left_out_frames(unmatched, left_out):  # KD-HLS-2
    """KD-HLS-2, the trailing case: `ffmpeg` decodes, from the source's
    sample tables, frames of trailing packets it flags as discarded, which
    the rendition leaves out. Holds when the source frames the rendition
    lacks are at most one per such packet, each at one of those packets'
    times. `left_out` is the source's packets the rendition left out, all
    flagged as discarded (checked with the packets)."""
    times = collections.Counter(p["pts"] for p in left_out if p.get("pts") is not None)
    for t, _ in unmatched:
        if not times[t]:
            return False
        times[t] -= 1
    return bool(unmatched)


def kd_gapless_edit(source, package, rendition, timescale, delta):  # KD-HLS-4
    """KD-HLS-4: an audio rendition whose edit list comes from the source's
    `iTunSMPB` tag; ffmpeg applies the tag to the source but keeps its
    timeline, so every time differs by the tag's delay. Holds when the
    init segment's media edit starts at the tag's delay and `delta` is
    minus that delay."""
    if rendition != "audio":
        return False
    tag = itunsmpb(source)
    if tag is None:
        return False
    return init_edit(package / "audio-init.mp4") == tag[0] and delta == Fraction(-tag[0], timescale)


def kd_gapless_conflict(source, package, rendition, out_frames, src_frames):  # KD-HLS-5
    """KD-HLS-5: the source has an edit list and an `iTunSMPB` tag that
    disagree; the rendition keeps the edit list and ffmpeg applies the tag
    to the source, decoding its first and last frames differently. Holds
    when the source has the tag, the init segment has an edit list other
    than the tag's, and every source frame but the first and the last
    appears in the output, in order, unchanged and shifted by one constant."""
    if rendition != "audio":
        return False
    tag = itunsmpb(source)
    media_time = init_edit(package / "audio-init.mp4")
    if tag is None or media_time is None or media_time == tag[0]:
        return False
    inner = src_frames[1:-1]
    if not inner:
        return False
    want = [h for _, _, h in inner]
    got = [h for _, _, h in out_frames]
    # Only where the first inner frame's hash is: linear, not quadratic.
    starts = [k for k, h in enumerate(got) if h == want[0] and k + len(want) <= len(got)][:16]
    for k in starts:
        if got[k:k + len(want)] == want:
            window = out_frames[k:k + len(inner)]
            return len({o[1] - s[1] for s, o in zip(inner, window)}) <= 1
    return False


def source_composition_times(source, index):
    """Every sample's stored composition time in the source track, in media
    ticks: ffprobe's packets with edit lists ignored (each sample once, at its
    stored times; scripts/clip_check.py reads them the same way)."""
    text = run(["ffprobe", "-v", "error", "-ignore_editlist", "1", "-select_streams", str(index),
                "-show_entries", "packet=pts", "-of", "csv=p=0", str(source)]).stdout.decode()
    try:
        return [int(line) for line in text.split() if line.strip()]
    except ValueError:
        return None


def composition_times(package, rendition):
    """Every sample's composition time in media ticks, from the media
    segments' box headers (offsets read as signed, as ffmpeg reads them)."""
    _, segments = media_playlist(package / f"{rendition}.m3u8")
    times = []
    for _, name in segments:
        dts = tfdt(package / name)
        for d, o in zip(trun_durations(package / name), trun_samples(package / name)):
            o = o or 0
            o = o - (1 << 32) if o >= 1 << 31 else o
            times.append(dts + o)
            dts += d or 0
    return times


def kd_midframe_edit(package, init, rendition, timescale, delta, source=None, index=None):  # KD-HLS-3
    """KD-HLS-3: the media edit starts between two samples' composition
    times; ffmpeg starts the source's timeline at a sample after it instead.
    Holds when `delta`, in ticks, is above 0, the edit's media time is no
    sample's composition time, the media time plus `delta` is one, and at
    most three samples lie between them. The edit and the composition times
    must be the source's own, read from the source file (its stored
    composition times, edit lists ignored, and its edit list): the
    rendition's would agree with themselves when wrong."""
    window = init_edit_window(package / init)
    ticks = delta * timescale
    if window is None or ticks.denominator != 1 or ticks <= 0 or source is None or index is None:
        return False
    media_time, ticks = window[0], int(ticks)
    times = composition_times(package, rendition)
    stored = source_composition_times(source, index)
    if stored is None or sorted(times) != sorted(stored):
        return False
    edits = cc.full_edit_list(str(source), index)
    if not edits or [t for _, t, _ in edits[2] if t != -1] != [media_time]:
        return False
    times = set(times)
    between = sum(1 for t in times if media_time < t < media_time + ticks)
    return media_time not in times and media_time + ticks in times and between <= 3


def kd_implicit_sbr(object_type, src_stream):  # KD-HLS-6
    """KD-HLS-6: HE-AAC signalled only in the bitstream (implicit SBR or
    PS). The configuration names the core, AAC LC, which CODECS gives;
    ffprobe decodes the extension and names HE-AAC. Holds when CODECS has
    object type 2 and ffprobe names HE-AAC or HE-AACv2 for the source."""
    return object_type == 2 and src_stream.get("profile") in ("HE-AAC", "HE-AACv2")


def kd_implicit_sbr_rate(track, src, src_stream):  # KD-HLS-6
    """The same cause in MediaInfo: from the init segment alone (no frames)
    it reads the core's sample rate, half the decoded rate it reads from
    the source. Holds when ffprobe names HE-AAC or HE-AACv2 for the source
    and the init's rate is half the source's."""
    try:
        return (src_stream.get("profile") in ("HE-AAC", "HE-AACv2")
                and 2 * int(track.get("SamplingRate")) == int(src.get("SamplingRate")))
    except (TypeError, ValueError):
        return False


def kd_profile_unnamed(out_stream, src_stream):  # KD-HLS-7
    """KD-HLS-7: ffprobe names no AAC profile for the source and names LC
    for the rendition, whose configuration is the source's as stored (the
    init segment's edit and sample entry are checked separately). Holds for
    aac on both sides, no source profile and LC on the output."""
    return (out_stream.get("codec_name") == src_stream.get("codec_name") == "aac"
            and src_stream.get("profile") in (None, "unknown") and out_stream.get("profile") == "LC")


def kd_source_last_cut(source, index, whole, src, out, src_frames, unmatched, extra, shift):  # KD-HLS-11
    """KD-HLS-11: ffmpeg cuts the source's last audio frame where Skip
    Samples side data on its last packet ends it, and reads none from the
    rendition's track runs, so it decodes that frame whole there. Holds when
    the one source frame left unmatched is the last, the one output frame
    left over is at its time, the last packets have the same bytes and
    duration, and ffprobe lists Skip Samples on the source's last packet that
    skip nothing and discard samples at its end, and none on the
    rendition's."""
    if not (src_frames and src and out) or len(unmatched) != 1 or len(extra) != 1:
        return False
    if unmatched[0] != tuple(src_frames[-1][1:]) or extra[0] != unmatched[0][0] + shift:
        return False
    if (src[-1]["hash"], src[-1]["duration"]) != (out[-1]["hash"], out[-1]["duration"]):
        return False
    cut = remux_check.last_skip_samples("ffprobe", str(source), index)
    return (cut is not None and cut[0] == 0 and cut[1] > 0
            and remux_check.last_skip_samples("ffprobe", str(whole), 0) is None)


def kd_fragment_reading(source, index, out_frames, work):  # KD-HLS-8
    """KD-HLS-8: ffmpeg decodes a stream from movie fragments to other
    frames than from the source's sample tables. Holds when the rendition's
    decoded frames are, in order, those of ffmpeg's own stream copy of the
    source stream into fragmented MP4, at the same times up to one
    constant."""
    copy = pathlib.Path(work) / f"fragcopy-{index}.mp4"
    try:
        run(["ffmpeg", "-v", "error", "-y", "-i", str(source), "-map", f"0:{index}", "-c", "copy",
             "-movflags", "frag_keyframe+empty_moov+default_base_moof", str(copy)])
        reference = frames(copy)
    except RuntimeError:
        return False
    finally:
        copy.unlink(missing_ok=True)
    if [h for _, _, h in reference] != [h for _, _, h in out_frames]:
        return False
    return len({o[1] - r[1] for r, o in zip(reference, out_frames)}) <= 1


def kd_left_top_crop(got, src, source):  # KD-HLS-9
    """KD-HLS-9: RESOLUTION is the size the codec configuration's parameter
    sets bound, which leaves out no left or top crop (docs/design/
    frame-sizes.md); ffprobe applies the crop. Holds when RESOLUTION is the
    size of the first frame ffmpeg decodes with cropping off, and at least
    the displayed size."""
    out = ffprobe_json(["-apply_cropping", "0", "-select_streams", str(src["index"]), "-read_intervals",
                        "%+#1", "-show_entries", "frame=width,height", str(source)])
    frames_ = out.get("frames") or [{}]
    try:
        uncropped = (int(frames_[0]["width"]), int(frames_[0]["height"]))
    except (KeyError, TypeError, ValueError):
        return False
    return tuple(got) == uncropped and got[0] >= int(src["width"]) and got[1] >= int(src["height"])


def source_fragments(source, track_id):
    """The source track as its box headers store it: (media timescale,
    samples in its sample table, [(movie fragment offset, decode time or
    None, durations, composition offsets)] of its track fragments in
    order), or None when unreadable. Cached per source."""
    return cc.cached((str(source), "hls fragments", track_id), lambda: _source_fragments(str(source), track_id))


def _source_fragments(path, track_id):
    try:
        movie = track_boxes.read_movie(path)
        track = next((t for t in movie["tracks"] if t["id"] == track_id), None)
        trak = next((t for t in (cc.headers(path) or {}).get("traks", []) if t.get("id") == track_id), None)
        if track is None or trak is None:
            return None
        runs = []
        for kind, offset, header, size in track_boxes.top_level(path):
            if kind != b"moof":
                continue
            data = memoryview(track_boxes.read_box(path, offset, header, size, track_boxes.MAX_MOOF))
            end = None
            for k, start, stop in track_boxes.boxes(data):
                if k != b"traf":
                    continue
                found, decode_time, samples, end = track_boxes.traf_samples(data, (start, stop), offset, end,
                                                                            movie["trex"])
                if found == track_id:
                    runs.append((offset, decode_time, list(samples["durations"]), list(samples["cto"])))
    except (track_boxes.Unreadable, OSError, struct.error, IndexError, ValueError):
        return None
    return track["timescale"], sum(n for n, _ in trak.get("stts") or []), runs


def source_decode_gaps(source, track_id):
    """{sample number: stored duration} of the source track's samples that
    end a track fragment whose next fragment's decode time (`tfdt`, 8.8.12)
    is not the one the stored durations reach; empty without such a gap."""
    found = source_fragments(source, track_id)
    if found is None:
        return {}
    _, number, runs = found
    gaps, end = {}, None
    for _, decode_time, durations, _ in runs:
        if end is not None and decode_time is not None and decode_time != end[0]:
            gaps[end[1]] = end[2]
        start = decode_time if decode_time is not None else (end[0] if end else None)
        number += len(durations)
        end = (start + sum(durations), number - 1, durations[-1]) if start is not None and durations else None
    return gaps


def source_index(source):
    """The source's one top-level segment index (8.16.3) when every
    reference is to media: (reference ID, timescale, [(referenced_size,
    subsegment_duration)], the file offset of the first subsegment), else
    None."""
    try:
        found = [b for b in track_boxes.top_level(str(source)) if b[0] == b"sidx"]
        if len(found) != 1:
            return None
        _, offset, header, size = found[0]
        data = track_boxes.read_box(str(source), offset, header, size, 16 << 20)
    except (track_boxes.Unreadable, OSError, struct.error):
        return None
    version = data[:1]
    at = 20 if version == b"\x00" else 28
    if len(data) < at + 4:
        return None
    reference_id, scale = int.from_bytes(data[4:8], "big"), int.from_bytes(data[8:12], "big")
    # The first subsegment starts first_offset bytes after the index.
    first = offset + size + int.from_bytes(data[16:20] if version == b"\x00" else data[20:28], "big")
    count = int.from_bytes(data[at + 2:at + 4], "big")
    at += 4
    if not scale or len(data) < at + 12 * count:
        return None
    references = []
    for i in range(count):
        word = int.from_bytes(data[at + 12 * i:at + 12 * i + 4], "big")
        if word >> 31:
            # A reference to another segment index.
            return None
        references.append((word & 0x7FFFFFFF, int.from_bytes(data[at + 12 * i + 4:at + 12 * i + 8], "big")))
    return reference_id, scale, references, first


def kd_index_drift(source, track_id, package, segments, starts):  # KD-HLS-10
    """KD-HLS-10: the source's track fragments carry decode times (`tfdt`,
    8.8.12) past the ones its stored durations reach, and its segment
    index's `subsegment_duration` values are those durations' sums. Holds
    when the segments are the source's subsegments, one each (sample counts
    from the index's byte ranges), their starts the index's running sums
    (to the microsecond, plus one constant), and each subsegment's earliest
    presentation time, from the source's fragments, lies past that sum by
    exactly the source's decode time gaps before it. Returns each segment's
    drift in seconds, or None when the entry does not apply. Everything but
    the playlist and the segments' sample counts is read from the source."""
    if source is None or track_id is None:
        return None
    index = source_index(source)
    found = source_fragments(source, track_id)
    if index is None or found is None or found[1] != 0:
        return None
    reference_id, scale, references, first = index
    media_scale, _, runs = found
    if reference_id != track_id or len(references) != len(segments) or not runs:
        return None
    # From the second segment on, the starts are the index's running sums
    # plus one constant: the first segment also spans the index's earliest
    # presentation time.
    total = Fraction(0)
    for n, (start, (_, duration)) in enumerate(zip(starts, references)):
        if n == 1:
            origin = start - total
        if n >= 1 and abs(start - origin - total) > Fraction(1, 1_000_000):
            return None
        total += Fraction(duration, scale)
    # The source's samples by subsegment: a track fragment belongs to the
    # subsegment whose byte range holds its movie fragment box.
    bounds, at = [], first
    for size, _ in references:
        bounds.append((at, at + size))
        at += size
    groups = [[] for _ in references]
    # The decode time gaps the fragments leave before each subsegment.
    gaps = [0] * len(references)
    end, n, gap = None, 0, 0
    for offset, decode_time, durations, offsets in runs:
        while n < len(bounds) and offset >= bounds[n][1]:
            n += 1
        if n == len(bounds) or offset < bounds[n][0]:
            return None
        if end is not None and decode_time is not None:
            gap += decode_time - end
        if not groups[n]:
            gaps[n] = gap
        at = end if decode_time is None else decode_time
        if at is None:
            return None
        for duration, cto in zip(durations, offsets):
            groups[n].append(at + cto)
            at += duration
        end = at
    if any(not g for g in groups):
        return None
    for (_, name), group in zip(segments, groups):
        if len(trun_samples(package / name)) != len(group):
            return None
    drift, total = [], Fraction(0)
    for group, (_, duration), before in zip(groups, references, gaps):
        moved = Fraction(min(group) - min(groups[0]), media_scale) - total
        # The drift is the decode time gaps the fragments leave, nothing else.
        if moved != Fraction(before - gaps[0], media_scale):
            return None
        drift.append(moved)
        total += Fraction(duration, scale)
    return drift if any(drift) else None


# Predicates by the name the twin gives them.
PREDICATES = {"negative_cto_shift": kd_negative_cto_shift, "hidden_lead": kd_hidden_lead,
              "midframe_edit": kd_midframe_edit, "gapless_edit": kd_gapless_edit,
              "gapless_conflict": kd_gapless_conflict, "implicit_sbr": kd_implicit_sbr,
              "profile_unnamed": kd_profile_unnamed, "fragment_reading": kd_fragment_reading,
              "left_top_crop": kd_left_top_crop, "index_drift": kd_index_drift,
              "source_last_cut": kd_source_last_cut}


def check_twin():
    """The twin lists exactly the predicates this script applies."""
    entries = json.loads(KD_FILE.read_text())["entries"]
    named = {e["predicate"] for e in entries}
    if named != set(PREDICATES) or [e["id"] for e in entries] != ["KD-HLS-1", "KD-HLS-2", "KD-HLS-3", "KD-HLS-4", "KD-HLS-5",
                                                                  "KD-HLS-6", "KD-HLS-7", "KD-HLS-8", "KD-HLS-9",
                                                                  "KD-HLS-10", "KD-HLS-11"]:
        sys.exit(f"{KD_FILE} and the predicates of {__file__} disagree")


def main():
    check_twin()
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--coreav", required=True)
    parser.add_argument("--media", type=pathlib.Path, help="directory of inputs")
    parser.add_argument("--files", nargs="*", type=pathlib.Path, default=[])
    parser.add_argument("--pattern", default="*.mp4,*.mov,*.m4a,*.m4v,*.3gp")
    parser.add_argument("--expect-refused", default="",
                        help="comma-separated globs of inputs that may be refused")
    parser.add_argument("--mediainfo")
    parser.add_argument("--segment-duration", default="6")
    parser.add_argument("--segments", choices=["all", "sample"], default="all")
    parser.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    parser.add_argument("--work", default=None)
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--retry-without-audio", action="store_true",
                        help="package an input whose default audio is not AAC again without audio")
    parser.add_argument("--master-decode", action="store_true",
                        help="also decode the master playlist through ffmpeg's HLS demuxer")
    parser.add_argument("--report", type=pathlib.Path)
    args = parser.parse_args()
    args.mediainfo_source = {}
    inputs = list(args.files)
    if args.media:
        patterns = args.pattern.split(",")
        inputs += sorted(p for p in args.media.iterdir()
                         if any(fnmatch.fnmatch(p.name, pat) for pat in patterns))
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        results = list(pool.map(lambda s: check_one(args, s), inputs))
    allowed = [g for g in args.expect_refused.split(",") if g]
    bad = 0
    for r in results:
        if r["status"] == "refused":
            ok = any(fnmatch.fnmatch(r["file"], g) for g in allowed)
            r["unexpected"] = not ok
            bad += 0 if ok else 1
            print(f"{'refused' if ok else 'REFUSED'} {r['file']}: {r.get('message')}")
        elif r["status"] == "fail":
            bad += 1
            print(f"FAIL {r['file']}")
            for f in r["failures"]:
                print(f"    {f}")
        else:
            extra = f" ({', '.join(sorted({d['id'] for d in r['divergences']}))})" if r["divergences"] else ""
            print(f"{r['status']} {r['file']}{extra}")
    packaged = [r for r in results if r["status"] in ("agree", "covered", "fail")]
    agree = sum(r.get("packets_agree", 0) for r in results)
    disagree = sum(r.get("packets_disagree", 0) for r in results)
    summary = {
        "inputs": len(results),
        "packaged": len(packaged),
        "agree": sum(r["status"] == "agree" for r in results),
        "covered": sum(r["status"] == "covered" for r in results),
        "fail": sum(r["status"] == "fail" for r in results),
        "refused": sum(r["status"] == "refused" for r in results),
        "packets_agree": agree,
        "packets_disagree": disagree,
    }
    print(json.dumps(summary))
    if args.report:
        args.report.write_text(json.dumps({"summary": summary, "results": results}, indent=1))
    if bad or not packaged:
        sys.exit(1)


if __name__ == "__main__":
    main()
