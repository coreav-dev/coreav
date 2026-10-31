#!/usr/bin/env python3
"""Black-box oracles for keyframe-aligned clips.

Reads a manifest of clips (from the `fixtures_round_trip` test of
coreav-isobmff-write, COREAV_CLIP_OUT, or from the corpus harness) and
checks every output against its source with the pinned ffprobe and ffmpeg
binaries, and with MediaInfo when --mediainfo is given:

- ffprobe reads the output as an MP4 file (major brand `isom`) holding the
  kept video stream and, if the clip has one, the kept audio stream, each
  with the source stream's codec, profile, dimensions, rotation, sample
  rate and channel layout;
- the output's stream durations, read from its box headers independently
  of CoreAV, are its own edits capped by its media durations (CoreAV's
  stream duration rule), and the video edit is the clip, from
  the snapped start to the written end;
- `ffprobe -show_packets -show_data_hash` lists, for each output stream,
  the source stream's packets of the kept run, found from the byte
  position of its first sample: the same data hash, size and keyframe
  flag, at the source's times minus the clip's start (exactly for the
  video, within one tick for the audio, whose edit is rounded to whole
  ticks). Every packet is counted towards the packet bar: agreeing, or
  disagreeing (a mismatch, a source packet of the run the output lacks, or
  an output packet without a source packet);
- `ffmpeg -copyts -f framemd5` of the output's video equals the source's
  decoded frames presented inside the clip, at their times minus the
  clip's start;
- the output's decoded audio equals the source's, sample for sample, from
  the first sample the output presents to its last (decoded with ffmpeg to
  32-bit float PCM, aligned within one sample, since the clip's start is
  rarely a whole audio sample);
- MediaInfo reports an MPEG-4 file whose tracks have the source's format,
  dimensions, sample rate and channels, and durations equal to the
  output's track header durations.

A difference counts as agreement only when an entry of
docs/known-divergences-clip.md covers it: each entry's
predicate below rechecks its conditions from the files' box headers and
the oracles' own output. Covered differences are reported apart from
direct agreement and never hide another difference of the same clip.

The binaries are black boxes; no implementation source
is read. Exits 1 if any check fails or the manifest is empty.
"""

import argparse
from bisect import bisect_left
import fnmatch
import json
import os
import pathlib
import re
import struct
import subprocess
import sys
import tempfile
import zlib
from fractions import Fraction

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import track_boxes  # noqa: E402
import writer_boxes  # noqa: E402

VIDEO_KEYS = ("codec_type", "codec_name", "profile", "width", "height", "pix_fmt")
AUDIO_KEYS = ("codec_type", "codec_name", "profile", "sample_rate", "channels", "channel_layout")
PACKET_KEYS = ("data_hash", "size", "pts", "dts", "flags", "pos", "duration")
MEDIAINFO_VIDEO = ("Format", "Width", "Height", "Rotation")
MEDIAINFO_AUDIO = ("Format", "Format_AdditionalFeatures", "SamplingRate", "Channels")
KD_FILE = pathlib.Path(__file__).resolve().parent.parent / "docs" / "known-divergences-clip.json"
CHUNK = 4 << 20
# Decoder threads per ffmpeg video run (`CLIP_CHECK_THREADS`), so that
# parallel checks share the machine; ffmpeg's default otherwise. Audio
# decodes on one thread: with frame threads, the pinned ffmpeg's FLAC and
# ALAC decoders apply a start skip longer than the first frame several
# frames late, so the decoded samples depend on the thread count.
THREADS = ["-threads", os.environ["CLIP_CHECK_THREADS"]] if os.environ.get("CLIP_CHECK_THREADS") else []
AUDIO_THREADS = ["-threads", "1"]

# Results for a source file, shared by its clips: (path, what) -> value.
# Decoded audio lives in files under WORK, removed by clear_cache().
_CACHE = {}
WORK = None


def _work():
    global WORK
    if WORK is None:
        WORK = tempfile.mkdtemp(prefix="clip-check-")
    return WORK


def cached(key, compute):
    if key not in _CACHE:
        _CACHE[key] = compute()
    return _CACHE[key]


def clear_cache():
    """Forgets every cached source result and removes decoded audio."""
    for value in _CACHE.values():
        if isinstance(value, tuple) and len(value) == 3 and isinstance(value[2], str):
            pathlib.Path(value[2]).unlink(missing_ok=True)
    _CACHE.clear()
    writer_boxes.forget()


def run(args):
    result = subprocess.run(args, capture_output=True, check=False)
    if result.returncode != 0:
        raise RuntimeError(f"{args[0]} failed ({result.returncode}): {result.stderr.decode(errors='replace')[:400]}")
    return result.stdout


def decode(args):
    """`run` for an ffmpeg decode, retried with one thread when the decode
    is killed by a signal. ffmpeg 7.0.2's frame-threaded AV1 decoding
    aborts (`Assertion pkt failed`) when a stream's last packet lies
    outside its edit, in sources as in clips; one thread decodes the same
    frames. The retry applies whether the call set a thread count or left
    ffmpeg's default, which is frame-threaded."""
    try:
        return run(args)
    except RuntimeError as error:
        if not killed(error):
            raise
        return run(single_thread(args))


def killed(error):
    """Whether a `run` error is a tool killed by a signal (an abort)."""
    return re.search(r" failed \(-\d+\)", str(error)) is not None


def single_thread(args):
    """An ffmpeg command with one decoding thread: its thread count
    replaced, or, without one, set before the first input."""
    if "-threads" in args:
        at = args.index("-threads")
        return [*args[:at], "-threads", "1", *args[at + 2:]]
    return [args[0], "-threads", "1", *args[1:]]


def ffprobe_json(ffprobe, *args):
    return json.loads(run([ffprobe, "-v", "error", "-of", "json", *args]))


def fraction(value):
    return Fraction(value["num"], value["den"])


def rotation(stream):
    for side in stream.get("side_data_list") or []:
        if "rotation" in side:
            return side["rotation"]
    return 0


def extradata(ffprobe, path, index):
    """The codec configuration of a stream, as ffprobe dumps it."""
    data = ffprobe_json(ffprobe, "-select_streams", str(index), "-show_entries", "stream=extradata",
                        "-show_data", path)
    streams = data.get("streams") or []
    return streams[0].get("extradata") if streams else None


def packets(ffprobe, path, index, raw=False):
    """The packets of a stream; with `raw`, edit lists ignored, so each
    sample is listed once at its stored decode and composition times."""
    data = ffprobe_json(
        ffprobe,
        *(["-ignore_editlist", "1"] if raw else []),
        "-select_streams",
        str(index),
        "-show_packets",
        "-show_data_hash",
        "SHA256",
        "-show_entries",
        "packet=" + ",".join(PACKET_KEYS),
        path,
    )
    return data.get("packets", [])


def framemd5(ffmpeg, path, index, video=False, raw=False):
    """(time base, [(pts, size, hash, duration)]) of the decoded frames of a
    stream, on the file's own presentation times (`-copyts`). Video frames
    keep the stream's own time base and every frame (`-enc_time_base demux`,
    `-fps_mode passthrough`), so their times are exact rather than rounded
    to a frame rate."""
    exact = ["-enc_time_base:v", "demux", "-fps_mode", "passthrough"] if video else []
    ignore = ["-ignore_editlist", "1"] if raw else []
    out = decode([ffmpeg, "-v", "error", *(THREADS if video else AUDIO_THREADS), "-copyts", *ignore, "-i", path, "-map", f"0:{index}", *exact,
                  "-f", "framemd5", "-"])
    base, frames = None, []
    for line in out.decode().splitlines():
        if line.startswith("#tb"):
            num, den = line.split(":", 1)[1].strip().split("/")
            base = Fraction(int(num), int(den))
        elif line and not line.startswith("#"):
            parts = [p.strip() for p in line.split(",")]
            if len(parts) == 6:
                frames.append((int(parts[2]), int(parts[4]), parts[5], int(parts[3])))
    return base, frames


def pcm(ffmpeg, path, index, channels, raw=False):
    """(decoded frames [(start, samples)] in seconds and samples, channels,
    path of the float32 PCM) of an audio stream, decoded with ffmpeg on the
    file's own times. The PCM holds the frames back to back, so a gap
    between two frames' times is not in it; the frame list maps times to
    positions."""
    base, frames = framemd5(ffmpeg, path, index, raw=raw)
    if base is None or not frames:
        return None
    # framemd5 hashes 16-bit samples: size / 2 / channels samples a frame.
    spans = [(pts * base, size // (2 * channels)) for pts, size, _, _ in frames]
    out = tempfile.mkstemp(suffix=".f32", dir=_work())
    os.close(out[0])
    os.unlink(out[1])
    decode([ffmpeg, "-v", "error", "-y", *AUDIO_THREADS, "-copyts", *(["-ignore_editlist", "1"] if raw else []),
            "-i", path, "-map", f"0:{index}", "-c:a", "pcm_f32le", "-f", "f32le", out[1]])
    return spans, channels, out[1]


def same_bytes(path_a, offset_a, path_b, length=None, offset_b=0):
    """Whether `path_b` (from `offset_b`, `length` bytes or the rest of it)
    equals `path_a` from `offset_a`."""
    size_b = os.path.getsize(path_b) - offset_b if length is None else length
    if offset_a < 0 or offset_a + size_b > os.path.getsize(path_a) or size_b < 0:
        return False
    with open(path_a, "rb") as a, open(path_b, "rb") as b:
        a.seek(offset_a)
        b.seek(offset_b)
        left = size_b
        while left > 0:
            n = min(CHUNK, left)
            if a.read(n) != b.read(n):
                return False
            left -= n
    return True


def position(spans, t, rate):
    """Index in the PCM of the sample presented at time `t`, or None."""
    at = 0
    for start, samples in spans:
        if start <= t < start + Fraction(samples, rate):
            return at + (t - start) * rate
        at += samples
    return None


def mediainfo_tracks(mediainfo, path):
    data = json.loads(run([mediainfo, "--Output=JSON", path]))
    return data["media"]["track"]


# Box headers, read independently of CoreAV (ISO/IEC 14496-12 4.2): the
# movie header, and per track the track header duration, the edit list and
# the media header (8.2.2, 8.3.2, 8.6.6, 8.4.2).


def _children(data):
    at = 0
    while at + 8 <= len(data):
        size, kind = struct.unpack(">I4s", data[at:at + 8])
        header = 8
        if size == 1 and at + 16 <= len(data):
            size, header = struct.unpack(">Q", data[at + 8:at + 16])[0], 16
        elif size == 0:
            size = len(data) - at
        if size < header or at + size > len(data):
            return
        yield kind.decode("latin1"), data[at + header:at + size]
        at += size


def _top_level(path, wanted, cap=64 << 20):
    with open(path, "rb") as f:
        total = f.seek(0, 2)
        at = 0
        while at + 8 <= total:
            f.seek(at)
            head = f.read(16)
            size, kind = struct.unpack(">I4s", head[:8])
            header = 8
            if size == 1:
                size, header = struct.unpack(">Q", head[8:16])[0], 16
            elif size == 0:
                size = total - at
            if size < header:
                return None
            if kind == wanted:
                if size > cap:
                    return None
                f.seek(at + header)
                return f.read(size - header)
            at += size
    return None


def movie_box(path, cap=64 << 20):
    """The payload of a file's movie box, inflated when it is a compressed
    QuickTime movie (`cmov`, track_boxes.movie_payload), or None."""
    moov = _top_level(path, b"moov", cap)
    if moov is None:
        return None
    try:
        return bytes(track_boxes.movie_payload(memoryview(moov)))
    except (track_boxes.Unreadable, zlib.error, struct.error, IndexError, ValueError):
        return None


def headers(path):
    """{'brand', 'mvhd': (timescale, duration), 'traks': [...]}, or None."""
    moov = movie_box(path)
    ftyp = _top_level(path, b"ftyp") or b""
    if moov is None:
        return None
    out = {"brand": ftyp[:4].decode("latin1"), "mvhd": None, "traks": []}
    try:
        for kind, body in _children(moov):
            if kind == "mvhd":
                out["mvhd"] = struct.unpack(">IQ", body[20:32]) if body[0] else struct.unpack(">II", body[12:20])
            if kind == "mvex":
                for k2, b2 in _children(body):
                    if k2 == "trex" and len(b2) >= 16:
                        track, _, duration = struct.unpack(">III", b2[4:16])
                        out.setdefault("trex", {})[track] = duration
            if kind != "trak":
                continue
            trak = {"elst": None}
            for k2, b2 in _children(body):
                if k2 == "tkhd":
                    trak["tkhd"] = struct.unpack(">Q", b2[28:36])[0] if b2[0] else struct.unpack(">I", b2[20:24])[0]
                    trak["id"] = struct.unpack(">I", b2[20:24] if b2[0] else b2[12:16])[0]
                if k2 == "edts":
                    for k3, b3 in _children(b2):
                        if k3 == "elst":
                            version, count = b3[0], struct.unpack(">I", b3[4:8])[0]
                            width, layout = (20, ">Qqi") if version else (12, ">Iii")
                            trak["elst"] = [
                                list(struct.unpack(layout, b3[8 + i * width:8 + (i + 1) * width])[:2])
                                for i in range(min(count, 8))
                            ]
                if k2 == "mdia":
                    for k3, b3 in _children(b2):
                        if k3 == "mdhd":
                            trak["mdhd"] = (struct.unpack(">IQ", b3[20:32]) if b3[0]
                                            else struct.unpack(">II", b3[12:20]))
                        if k3 == "hdlr":
                            trak["handler"] = b3[8:12].decode("latin1")
                        if k3 == "minf":
                            trak["stts_sum"] = _stts_sum(b3)
                            trak["stts"] = _stts_entries(b3)
                            trak["ctts_unsigned_large"] = _ctts_unsigned_large(b3)
                            trak["ctts_reads_negative"] = _ctts_reads_negative(b3)
                            trak["stss"] = _stss_count(b3)
                            trak["ctts_min"] = _ctts_min(b3)
            out["traks"].append(trak)
    except (struct.error, IndexError):
        return None
    return out


def _ctts_unsigned_large(minf):
    """Whether `ctts` (8.6.1.3) is version 0 with an offset of at least
    2^31, a value a version 1 reader would take as negative."""
    for kind, body in _children(minf):
        if kind != "stbl":
            continue
        for k2, b2 in _children(body):
            if k2 == "ctts":
                if b2[0] != 0:
                    return False
                count = struct.unpack(">I", b2[4:8])[0]
                return any(struct.unpack(">I", b2[12 + 8 * i:16 + 8 * i])[0] >= 1 << 31
                           for i in range(count))
    return False


def _ctts_reads_negative(minf):
    """Whether `ctts` holds an offset that a reader taking 32-bit offsets as
    signed sees as negative: any version 1 value below 0, or a version 0
    value of at least 2^31."""
    for kind, body in _children(minf):
        if kind != "stbl":
            continue
        for k2, b2 in _children(body):
            if k2 == "ctts":
                count = struct.unpack(">I", b2[4:8])[0]
                return any(struct.unpack(">i", b2[12 + 8 * i:16 + 8 * i])[0] < 0 for i in range(count))
    return False


def _ctts_min(minf):
    """The smallest offset of `ctts` read as signed, or None without one."""
    for kind, body in _children(minf):
        if kind != "stbl":
            continue
        for k2, b2 in _children(body):
            if k2 == "ctts":
                count = struct.unpack(">I", b2[4:8])[0]
                values = [struct.unpack(">i", b2[12 + 8 * i:16 + 8 * i])[0] for i in range(count)]
                return min(values) if values else None
    return None


def _stss_count(minf):
    """Number of entries of `stss` (8.6.2), or None without one: every
    sample is then a sync sample."""
    for kind, body in _children(minf):
        if kind != "stbl":
            continue
        for k2, b2 in _children(body):
            if k2 == "stss":
                return struct.unpack(">I", b2[4:8])[0]
    return None


def fragment_samples(path, track_id, default_duration, cap=100_000, limit=None):
    """(decode time, duration) of each sample of a track's movie fragments
    (ISO/IEC 14496-12 8.8): `tfdt` gives a fragment's first decode time, the
    run's durations or the `tfhd` and `trex` defaults the rest. [] without
    fragments of the track; None when unreadable or past `cap` samples.
    With `limit`, it stops once it has that many samples, and `cap` is at
    least `limit`."""
    cap = cap if limit is None else max(cap, limit)
    samples = []
    try:
        with open(path, "rb") as f:
            total, at = f.seek(0, 2), 0
            while at + 8 <= total:
                f.seek(at)
                head = f.read(16)
                size, kind = struct.unpack(">I4s", head[:8])
                header = 8
                if size == 1:
                    size, header = struct.unpack(">Q", head[8:16])[0], 16
                elif size == 0:
                    size = total - at
                if size < header:
                    return None
                if kind == b"moof":
                    if size > 64 << 20:
                        return None
                    f.seek(at + header)
                    for k1, traf in _children(f.read(size - header)):
                        if k1 != "traf":
                            continue
                        boxes = list(_children(traf))
                        tfhd = next((b for k, b in boxes if k == "tfhd"), None)
                        if tfhd is None or struct.unpack(">I", tfhd[4:8])[0] != track_id:
                            continue
                        flags, pos, duration = int.from_bytes(tfhd[1:4], "big"), 8, default_duration
                        pos += 8 if flags & 0x01 else 0
                        pos += 4 if flags & 0x02 else 0
                        if flags & 0x08:
                            duration = struct.unpack(">I", tfhd[pos:pos + 4])[0]
                        tfdt = next((b for k, b in boxes if k == "tfdt"), None)
                        t = (struct.unpack(">Q", tfdt[4:12])[0] if tfdt[0] else struct.unpack(">I", tfdt[4:8])[0]) \
                            if tfdt is not None else (samples[-1][0] + samples[-1][1] if samples else 0)
                        for k, trun in boxes:
                            if k != "trun":
                                continue
                            tflags, count = int.from_bytes(trun[1:4], "big"), struct.unpack(">I", trun[4:8])[0]
                            pos = 8 + (4 if tflags & 0x01 else 0) + (4 if tflags & 0x04 else 0)
                            width = 4 * bin(tflags & 0xF00).count("1")
                            for i in range(count):
                                d = struct.unpack(">I", trun[pos + i * width:pos + i * width + 4])[0] \
                                    if tflags & 0x100 else duration
                                if limit is not None and len(samples) >= limit:
                                    return samples
                                if d is None or len(samples) >= cap:
                                    return None
                                samples.append((t, d))
                                t += d
                at += size
    except (struct.error, IndexError, OSError):
        return None
    return samples


def _stts_sum(minf):
    """Sum of the decode durations of `stts` (8.6.1.2), or None."""
    entries = _stts_entries(minf)
    return None if entries is None else sum(n * delta for n, delta in entries)


def _stts_entries(minf):
    """The (sample count, decode duration) entries of `stts` (8.6.1.2), or
    None."""
    for kind, body in _children(minf):
        if kind != "stbl":
            continue
        for k2, b2 in _children(body):
            if k2 == "stts":
                count = struct.unpack(">I", b2[4:8])[0]
                return [struct.unpack(">II", b2[8 + 8 * i:16 + 8 * i]) for i in range(count)]
    return None


def stored_durations(path, headers_, index, needed=None):
    """The decode durations a track stores, as `stts` entries: its sample
    table's, then those of its movie fragments (fragment_samples, with the
    `trex` default), up to the first `needed` samples where given; None when
    unreadable."""
    traks = (headers_ or {}).get("traks") or []
    if not 0 <= index < len(traks):
        return None
    trak, trex = traks[index], (headers_ or {}).get("trex") or {}
    entries = list(trak.get("stts") or [])
    if trak.get("id") in trex:
        in_table = sum(n for n, _ in entries)
        limit = None if needed is None else max(needed - in_table, 0)
        fragments = cached((path, "fragments", trak["id"], limit),
                           lambda: fragment_samples(path, trak["id"], trex[trak["id"]], limit=limit))
        if fragments is None:
            return None
        entries += [(1, d) for _, d in fragments]
    return entries


def stts_window(entries, first, count):
    """The `stts` entries (sample count, decode duration) of samples first to
    first + count - 1 (0-based), adjacent equal durations merged; None when
    the table has fewer samples."""
    window, at = [], 0
    for n, delta in entries or []:
        lo, hi = max(at, first), min(at + n, first + count)
        if hi > lo:
            window.append((hi - lo, delta))
        at += n
    return merged_stts(window) if at >= first + count else None


def merged_stts(entries):
    """`stts` entries with adjacent equal durations merged and empty ones
    dropped, so two tables of the same durations compare equal."""
    out = []
    for n, delta in entries or []:
        if n and out and out[-1][1] == delta:
            out[-1] = (out[-1][0] + n, delta)
        elif n:
            out.append((n, delta))
    return out


def stts_window_sum(entries, first, count):
    """Sum of the stored decode durations of samples first to first + count
    - 1 (0-based), or None when the table has fewer."""
    window = stts_window(entries, first, count)
    return None if window is None else sum(n * delta for n, delta in window)


def edit_span(trak, movie_timescale):
    """(empty, segment) of a track's edits in seconds; (0, None) without."""
    if not trak.get("elst"):
        return Fraction(0), None
    empty = sum(Fraction(d, movie_timescale) for d, t in trak["elst"] if t == -1)
    segment = sum(Fraction(d, movie_timescale) for d, t in trak["elst"] if t != -1)
    return empty, segment


# Several edits (docs/design/edit-list-writers.md, common decision 3): the
# edit arithmetic is done here, from ISO/IEC 14496-12, on the file's own
# boxes; ffmpeg supplies the samples and frames with edit lists ignored.

MAX_EDITS_BYTES = 64 << 20


def full_edit_list(path, index):
    """(movie timescale, media timescale, entries) of the track at position
    `index`: the movie header's timescale (8.2.2), the media header's (8.4.2)
    and every entry of the track's last edit list (8.6.6) as stored,
    `(segment_duration, media_time, media_rate)` with the rate as its
    16.16 value; entries empty without an edit list. None when unreadable."""
    moov = movie_box(path, MAX_EDITS_BYTES)
    if moov is None:
        return None
    movie_ts, traks = None, []
    try:
        for kind, body in _children(moov):
            if kind == "mvhd":
                movie_ts = struct.unpack(">I", body[20:24] if body[0] else body[12:16])[0]
            elif kind == "trak":
                traks.append(body)
        if not 0 <= index < len(traks) or not movie_ts:
            return None
        media_ts, entries = None, []
        for kind, body in _children(traks[index]):
            if kind == "edts":
                for k2, b2 in _children(body):
                    if k2 == "elst":
                        version, count = b2[0], struct.unpack(">I", b2[4:8])[0]
                        width, layout = (20, ">Qqi") if version else (12, ">Iii")
                        if 8 + count * width > len(b2):
                            return None
                        entries = [struct.unpack(layout, b2[8 + i * width:8 + (i + 1) * width])
                                   for i in range(count)]
            elif kind == "mdia":
                for k2, b2 in _children(body):
                    if k2 == "mdhd":
                        media_ts = struct.unpack(">I", b2[20:24] if b2[0] else b2[12:16])[0]
        return (movie_ts, media_ts, entries) if media_ts else None
    except (struct.error, IndexError):
        return None


def several_edits(entries):
    """Whether an edit list is more than one media edit after at most one
    empty edit."""
    simple = (len(entries) == 1 and entries[0][1] != -1) or (
        len(entries) == 2 and entries[0][1] == -1 and entries[1][1] != -1)
    return bool(entries) and not simple


def iso_windows(movie_ts, media_ts, entries):
    """Each media edit's media interval and where it starts on the
    presentation timeline, `(media start, media end, presented at)` in media
    ticks (ISO/IEC 14496-12 8.6.6: the entries play one after another, an
    empty edit presenting nothing for its duration)."""
    windows, at = [], Fraction(0)
    for duration, media_time, _rate in entries:
        length = Fraction(duration * media_ts, movie_ts)
        if media_time != -1:
            windows.append((Fraction(media_time), Fraction(media_time) + length, at))
        at += length
    return windows


def iso_presented(frames, windows):
    """`(presentation time, content)` of every frame each window presents:
    those whose composition interval `[cts, cts + duration)` overlaps the
    window's media interval (a frame of no duration when the window holds
    its start), at their composition time less the window's media start
    plus its presentation start; in media ticks, sorted."""
    order = sorted(frames, key=lambda f: f[0])
    starts = [f[0] for f in order]
    longest = max((f[1] for f in order), default=0)
    shown = []
    for m0, m1, at in windows:
        lo = bisect_left(starts, m0 - longest)
        hi = bisect_left(starts, m1)
        for cts, duration, content in order[lo:hi]:
            if (duration > 0 and cts < m1 and cts + duration > m0) or (duration == 0 and m0 <= cts < m1):
                shown.append((cts - m0 + at, content))
    shown.sort(key=lambda s: s[0])
    return shown


def compare_frames_several(src, out, src_windows, out_windows, zero, end, found):
    """The frames the output's edits present equal those the source's edits
    present inside `[zero, end)` (media ticks of the source), at the same
    times less `zero`; frames decoded with edit lists ignored."""
    (sb, sf), (ob, of) = src, out
    if sb is None or ob is None:
        found.append(("video:framemd5", "framemd5 without a time base"))
        return
    src_frames = [(Fraction(pts), Fraction(duration), (size, h)) for pts, size, h, duration in sf]
    out_frames = [(Fraction(pts), Fraction(duration), (size, h)) for pts, size, h, duration in of]
    expected = [(t - zero, c) for t, c in iso_presented(src_frames, src_windows) if zero <= t < end]
    actual = iso_presented(out_frames, out_windows)
    if not expected or expected != actual:
        first = next((i for i, (a, b) in enumerate(zip(expected, actual)) if a != b), min(len(expected), len(actual)))
        found.append(("video:framemd5", f"video frames the edits present differ at frame {first} "
                      f"({len(expected)} in the source's clip, {len(actual)} in the output)"))


def ffmpeg_model(packets_raw, frames_raw, edits):
    """ffmpeg's edit-applied presentation of a video track as the measured
    model of docs/design/edit-lists.md ("What `ffprobe` lists", rules 1 to
    5) predicts it from the track's stored samples: `(view, empties, rules)`
    with `view` mapping each presented frame's time in seconds to its
    content, `empties` the spans in seconds of the empty edits after a media
    edit, which ffmpeg presents media during (rule 2) and the model does not
    predict, and `rules` the names of the rules that move a time away from
    ISO/IEC 14496-12's. None when the model does not apply (no media edit,
    or a time base other than the media timescale)."""
    movie_ts, media_ts, entries = edits
    base, frames = frames_raw
    if base != Fraction(1, media_ts) or not any(m != -1 for _, m, _ in entries):
        return None
    content = {pts: (size, h) for pts, size, h, _ in frames}
    samples = [(p["pts"], p["dts"]) for p in packets_raw
               if isinstance(p.get("pts"), int) and isinstance(p.get("dts"), int)]
    view, empties, rules = {}, [], set()
    at, first = 0, None
    for duration, media_time, _rate in entries:
        # Rule 1: whole media ticks, halves up.
        exact = Fraction(duration * media_ts, movie_ts)
        length = int(exact + Fraction(1, 2))
        if length != exact:
            rules.add("durations rounded to whole ticks")
        if media_time == -1:
            if first is not None:
                empties.append((Fraction(at, media_ts), Fraction(at + length, media_ts)))
                rules.add("an empty edit between media edits listed as media")
            at += length
            continue
        lead = next((s for s in samples if s[0] >= media_time), None)
        if lead is None:
            at += length
            continue
        if first is None:
            # Rule 4: the first composition time at or after the media time
            # lands on the edit's start.
            snap = min(c for c, _ in samples if c >= media_time)
            first = lead[1] - snap
            offset = media_time - snap
            if offset:
                rules.add("the first edit snapped to its first composition time")
        else:
            # Rule 5: the decode time of the edit's first sample is anchored.
            offset = first + media_time - lead[1]
            if offset:
                rules.add("times anchored on decode times")
        for c, _ in samples:
            if media_time <= c < media_time + length and c in content:
                view[Fraction(c - media_time + at + offset, media_ts)] = content[c]
        at += length
    return view, empties, rules


def emitted(frames_view):
    """A file's edit-applied frames in the order ffmpeg emits them:
    `(view, clamped)`, `view` mapping each time in seconds to its content
    and `clamped` the `(time, content)` of each frame that comes with the
    same time as the frame before it. When the decoder emits a frame before
    frames presented earlier, ffmpeg's output moves their times up to that
    frame's ("Non-monotonic DTS ... changing to"), so their own times are
    lost."""
    base, frames = frames_view
    view, clamped, last = {}, [], None
    for pts, size, h, _ in frames:
        t = pts * base
        if t == last:
            clamped.append((t, (size, h)))
        else:
            view[t] = (size, h)
        last = t
    return view, clamped


def second_view_video(src, out, src_model, out_model, zero, end, found):
    """The second view of common decision 3 for video: ffmpeg's
    edit-applied frames of the output against the source's from the clip's
    start, by content and time. The source's frames are taken up to the
    clip's end, or to the output's last frame when ffmpeg places frames past
    the end in it. When they differ, each file's view is compared with
    ffmpeg's measured edit model (`ffmpeg_model`): if both follow it, the
    difference is named by the model's rules that apply; otherwise it is
    unexplained. Frames whose times ffmpeg's output clamped (`emitted`)
    follow the model when it presents their content no later than the time
    they were clamped to."""
    if src[0] is None or out[0] is None:
        found.append(("video:second", "framemd5 without a time base"))
        return
    (src_view, src_clamped), (out_view, out_clamped) = emitted(src), emitted(out)
    last = max(out_view, default=Fraction(0))

    def inside(t):
        return 0 <= t < end - zero or 0 <= t <= last
    expected = {t - zero: c for t, c in src_view.items() if inside(t - zero)}
    expected_clamped = [(t - zero, c) for t, c in src_clamped if inside(t - zero)]
    differ = sorted(t for t in set(out_view) | set(expected) if out_view.get(t) != expected.get(t))
    if not differ and expected_clamped == out_clamped:
        return

    def departs(view, clamped, model):
        """The first time at which a file's view departs from the model,
        outside the empty edits it does not predict; None if it follows
        it."""
        predicted, empties, _ = model
        late = {}
        for t, c in clamped:
            late.setdefault(c, []).append(t)
        for t in sorted(set(view) | set(predicted)):
            if view.get(t) == predicted.get(t) or any(a <= t < b for a, b in empties):
                continue
            if t not in view and any(t <= u for u in late.get(predicted[t], [])):
                late[predicted[t]].remove(next(u for u in late[predicted[t]] if t <= u))
                continue
            return t
        return next((min(ts) for ts in late.values() if ts), None)

    where = f"{max(len(differ), 1)} frames differ, first at {float(differ[0] if differ else 0):.3f} s"
    if src_model is None or out_model is None:
        found.append(("video:second", f"{where}: unexplained (ffmpeg's edit model does not apply)"))
        return
    off = [(name, departs(view, clamped, model)) for name, view, clamped, model in
           (("source", src_view, src_clamped, src_model), ("output", out_view, out_clamped, out_model))]
    off = [(name, t) for name, t in off if t is not None]
    if off:
        found.append(("video:second", f"{where}: unexplained ("
                      + ", ".join(f"the {name}'s view departs from ffmpeg's edit model at {float(t):.3f} s"
                                  for name, t in off) + ")"))
        return
    rules = src_model[2] | out_model[2]
    if src_clamped or out_clamped:
        rules.add("frames emitted out of presentation order, their times clamped by ffmpeg's output")
    found.append(("video:second", f"{where}: ffmpeg's edit model in both files ({'; '.join(sorted(rules))})"))


def check_audio_several(src, out, src_windows, out_windows, first_dts, zero, timescale, found, end=None,
                        media_end=None):
    """Audio with several edits: the output's decoded audio equals the
    source's from the output's second frame on (the first follows a decoder
    restart), both decoded with edit lists ignored; and each output edit
    presents the media the source's edits present at the same time, within
    one tick (the edits are rounded to the movie timescale). `first_dts` is
    the source decode time of the output's first sample and `zero` the
    clip's start, in audio media ticks."""
    if src is None or out is None:
        found.append(("audio:pcm", "audio did not decode"))
        return
    (s_spans, s_channels, s_raw), (o_spans, o_channels, o_raw) = src, out
    if s_channels != o_channels or len(o_spans) < 2:
        found.append(("audio:pcm", f"audio: {s_channels} source and {o_channels} output channels"))
        return
    frame = 4 * o_channels
    # The output's second frame, at its time plus the run's first decode
    # time in the source; spans are (start in seconds, samples).
    target = o_spans[1][0] + Fraction(first_dts, timescale)
    offset, seen = None, 0
    for start, samples in s_spans:
        if abs(start - target) * timescale < Fraction(1, 2):
            offset = seen
            break
        seen += samples
    if offset is None or not same_bytes(s_raw, offset * frame, o_raw, offset_b=o_spans[0][1] * frame):
        found.append(("audio:pcm", "audio: the output's decoded audio differs from the source's run"))
    for m0, m1, at in out_windows:
        sm0, sm1, s_at = m0 + first_dts, m1 + first_dts, at + zero
        holder = next((w for w in src_windows if w[0] - 1 <= sm0 and sm1 <= w[1] + 1), None)
        if holder is None or abs(sm0 - holder[0] + holder[2] - s_at) > 1:
            found.append(("audio:edits", f"audio: the output's edit at {float(at)} ticks presents other media "
                          "than the source's edits"))
            break
    # Coverage: what the output's edits present, as spans of its timeline,
    # equals what the source's present from the clip's start to the
    # output's last presented time, within one tick at each boundary (the
    # edits are rounded to the movie timescale).
    def merged(spans):
        out = []
        for a, b in sorted(spans):
            if out and a <= out[-1][1] + 1:
                out[-1] = (out[-1][0], max(out[-1][1], b))
            elif b > a:
                out.append((a, b))
        return out
    shown = merged([(at, at + m1 - m0) for m0, m1, at in out_windows])
    if shown:
        # Up to the clip's end; a source window shows no audio past the
        # source's last sample (`media_end`, its media time).
        until = end - zero if end is not None else shown[-1][1]
        cap = (lambda m0, m1: min(m1, media_end)) if media_end is not None else (lambda m0, m1: m1)
        expected = merged([(max(at - zero, 0), min(at + cap(m0, m1) - m0 - zero, until))
                           for m0, m1, at in src_windows
                           if cap(m0, m1) > m0 and at + cap(m0, m1) - m0 > zero and at - zero < until])
        if len(expected) != len(shown) or any(abs(a - c) > 1 or abs(b - d) > 1
                                              for (a, b), (c, d) in zip(expected, shown)):
            found.append(("audio:edits", f"audio: the output presents {[(float(a), float(b)) for a, b in shown][:4]} "
                          f"ticks, the source {[(float(a), float(b)) for a, b in expected][:4]} from the clip's start"))


# Comparisons. Each returns (agreeing, disagreeing) packet counts and appends
# (field, message) differences.


def compare_packets(kind, src, out, kept, shift, found, evidence=None):
    """Pairs the output's packets with the source's kept run, which starts at
    the byte position of its first sample. `shift` is the clip's start in
    the stream's time base."""
    # Video times are exact, unless the clip's start is not a whole number
    # of ticks (a source's empty edit converted from the movie timescale):
    # ffprobe then rounds the source's times to whole ticks. Audio times are
    # within one tick, the audio edit being rounded to whole ticks.
    tolerance = 0 if kind == "video" and shift.denominator == 1 else 1
    start = next((i for i, p in enumerate(src) if p.get("pos") == str(kept["first_pos"])), None)
    if start is None:
        found.append((f"{kind}:packets", f"{kind}: no source packet at byte {kept['first_pos']}"))
        return 0, len(out)
    run_packets = src[start:start + kept["samples"]]
    if evidence is not None:
        evidence[f"{kind}_run"] = (run_packets, out, shift)
        evidence[f"{kind}_run_start"] = start
    agree = disagree = 0
    j = 0
    missing = []
    mismatched = []
    for i, s in enumerate(run_packets):
        o = out[j] if j < len(out) else None
        if o is None or o.get("data_hash") != s.get("data_hash"):
            # A source packet of the run that the output does not list.
            missing.append(i)
            disagree += 1
            continue
        j += 1
        same = (o.get("size") == s.get("size") and (o.get("flags", "")[:1] == s.get("flags", "")[:1]))
        for key in ("pts", "dts"):
            a, b = s.get(key), o.get(key)
            if not isinstance(a, int) or not isinstance(b, int) or abs(Fraction(a) - (b + shift)) > tolerance:
                same = False
        if same:
            agree += 1
        else:
            disagree += 1
            mismatched.append(i)
    extra = len(out) - j
    disagree += extra
    if missing:
        found.append((f"{kind}:missing", f"{kind}: {len(missing)} packets of the run not in the output, "
                      f"at {missing[:5]} of {len(run_packets)}"))
    if mismatched:
        found.append((f"{kind}:packets", f"{kind}: packets differ at {mismatched[:5]} of {len(run_packets)}"))
    if evidence is not None:
        evidence[f"{kind}_mismatched"] = len(mismatched)
    if extra:
        found.append((f"{kind}:extra", f"{kind}: {extra} output packets with no source packet"))
    return agree, disagree


def compare_video_frames(src, out, zero, end, found, evidence=None):
    (sb, sf), (ob, of) = src, out
    if sb is None or ob is None:
        found.append(("video:framemd5", "framemd5 without a time base"))
        return
    # When the clip's start is not a whole number of source ticks, ffmpeg
    # rounds the source's times to whole ticks: they are compared within a
    # tick, and the window is taken on the rounded start.
    rounded = zero / sb != int(zero / sb)
    if rounded:
        zero_r = round(zero / sb) * sb
        shift_r = zero_r - zero
        end, zero = end + shift_r, zero_r
    expected = [(pts * sb - zero, size, h) for pts, size, h, _ in sf if zero <= pts * sb < end]
    # A frame presented exactly at the clip's end has no presented duration
    # (a last sample of decode duration 0, say); ffmpeg may still show it,
    # so the output may hold the source's frames at that time as well.
    at_end = [(pts * sb - zero, size, h) for pts, size, h, _ in sf if pts * sb == end]
    actual = [(pts * ob, size, h) for pts, size, h, _ in of]
    if rounded:
        # Times within a tick: the output's times rounded to the source's.
        actual = [(round(t / sb) * sb, size, h) for t, size, h in actual]
    if evidence is not None:
        evidence["video_frames"] = (expected, actual)
        evidence["video_framemd5"] = (src, out, zero, end)
    if not expected or (expected != actual and expected + at_end != actual):
        first = next((i for i, (a, b) in enumerate(zip(expected, actual)) if a != b), min(len(expected), len(actual)))
        found.append(("video:framemd5", f"video framemd5 differs at frame {first} "
                      f"({len(expected)} source frames in the clip, {len(actual)} output frames)"))


def check_audio_pcm(src, out, zero, rate, found):
    if src is None or out is None:
        found.append(("audio:pcm", "audio did not decode"))
        return
    (s_spans, s_channels, s_raw), (o_spans, o_channels, o_raw) = src, out
    o_size = os.path.getsize(o_raw)
    if s_channels != o_channels or not o_size:
        found.append(("audio:pcm", f"audio: {s_channels} source and {o_channels} output channels"))
        return
    frame = 4 * o_channels
    n = o_size // frame
    # The output's first sample is presented at its first frame's time; the
    # source presents the same audio at that time plus the clip's start.
    exact = position(s_spans, zero + o_spans[0][0], rate)
    if exact is None:
        found.append(("audio:pcm", "audio: the clip's start is not in the source's decoded audio"))
        return
    for offset in sorted({int(exact) - 1, int(exact), int(exact) + 1, round(exact)}):
        if offset >= 0 and same_bytes(s_raw, offset * frame, o_raw):
            return
    found.append(("audio:pcm", f"audio: {n} output samples differ from the source's at sample "
                  f"{float(exact):.2f} (within one sample)"))


def stored_times(listed):
    """Packets listed with edit lists ignored, at their stored decode times.
    With a negative composition offset, ffprobe moves every decode time back
    by the smallest one (docs/design/edit-lists.md, "What `ffprobe` lists",
    rule 6). Stored decode times start at 0 in a movie without fragments
    (ISO/IEC 14496-12 8.6.1.2), so the first listed decode time is that
    move."""
    move = listed[0].get("dts") if listed else None
    if not isinstance(move, int) or move == 0:
        return listed
    return [dict(p, dts=p["dts"] - move) if isinstance(p.get("dts"), int) else p for p in listed]


def check_several(kind, entry, track, src, out, src_edits, out_edits, out_index, zero, end, ffprobe, ffmpeg,
                  found, info, evidence):
    """The checks of one kept track when the source or the output has
    several edits (docs/design/edit-list-writers.md, common decision 3):
    durations from the output's boxes, packets and decoded content read with
    edit lists ignored and compared through the edits as ISO/IEC 14496-12
    applies them; ffmpeg's own edit-applied view reported, not gating.
    Returns (agreeing, disagreeing) packets."""
    source, output = entry["source"], entry["output"]
    movie_ts, timescale, entries = out_edits
    src_windows = iso_windows(*src_edits)
    out_windows = iso_windows(*out_edits)
    # Durations: the track header lasts its edits (8.3.2), and the video's
    # edits span the clip.
    total = sum(Fraction(d, movie_ts) for d, _, _ in entries)
    oh = headers(output)
    if oh and abs(Fraction(oh["traks"][out_index]["tkhd"], movie_ts) - total) > 0:
        found.append((f"{kind}:duration", f"{kind}: track header {oh['traks'][out_index]['tkhd']} ticks, "
                      f"edits {float(total)} s"))
    # The first track's edits span the clip: the video's, or the audio's in
    # a clip without video.
    if out_index == 0 and total != end - zero:
        found.append((f"{kind}:edit", f"{kind} edits {float(total)} s, clip {float(end - zero)} s"))
    # Packets: stored times, the output's decode times starting at the run's.
    src_raw = cached((source, "raw packets", track["stream_index"]),
                     lambda: stored_times(packets(ffprobe, source, track["stream_index"], raw=True)))
    out_raw = stored_times(packets(ffprobe, output, out_index, raw=True))
    start = next((i for i, p in enumerate(src_raw) if p.get("pos") == str(track["first_pos"])), None)
    first_dts = int(src_raw[start]["dts"]) if start is not None and isinstance(src_raw[start].get("dts"), int) else 0
    agree, disagree = compare_packets(kind, src_raw, out_raw, track, Fraction(first_dts), found, evidence)
    zero_ticks, end_ticks = zero * timescale, end * timescale
    second = []
    if kind == "video":
        src_frames = cached((source, "raw framemd5", track["stream_index"]),
                            lambda: framemd5(ffmpeg, source, track["stream_index"], video=True, raw=True))
        out_frames = framemd5(ffmpeg, output, out_index, video=True, raw=True)
        compare_frames_several(src_frames, out_frames, src_windows, out_windows, zero_ticks, end_ticks, found)
        second_view_video(cached((source, "framemd5", track["stream_index"]),
                                 lambda: framemd5(ffmpeg, source, track["stream_index"], video=True)),
                          framemd5(ffmpeg, output, out_index, video=True),
                          cached((source, "model", track["stream_index"]),
                                 lambda: ffmpeg_model(src_raw, src_frames, src_edits)),
                          ffmpeg_model(out_raw, out_frames, out_edits), zero, end, second)
    else:
        channels = int(src.get("channels") or 1)
        out_pcm = pcm(ffmpeg, output, out_index, int(out.get("channels") or 1), raw=True)
        evidence["audio_output_pcm"] = out_pcm
        check_audio_several(cached((source, "raw pcm", track["stream_index"]),
                                   lambda: pcm(ffmpeg, source, track["stream_index"], channels, raw=True)),
                            out_pcm, src_windows, out_windows, first_dts, zero_ticks, timescale, found,
                            end=end_ticks, media_end=max((p["pts"] + p.get("duration", 0) for p in src_raw
                                                          if isinstance(p.get("pts"), int)), default=None))
        rate = int(src.get("sample_rate") or 0)
        if rate:
            applied = pcm(ffmpeg, output, out_index, int(out.get("channels") or 1))
            check_audio_pcm(cached((source, "pcm", track["stream_index"]),
                                   lambda: pcm(ffmpeg, source, track["stream_index"], channels)),
                            applied, zero, rate, second)
            if applied:
                pathlib.Path(applied[2]).unlink(missing_ok=True)
    # The second view: ffmpeg's edit-applied output against the source's.
    info.append(f"second view, {kind}: " + ("agrees" if not second else "; ".join(m for _, m in second)))
    return agree, disagree


def check(entry, ffprobe, ffmpeg, mediainfo):
    """Returns the gating problems, the informational differences, the
    differences covered by a known divergence and the packet counts."""
    source, output = entry["source"], entry["output"]
    zero, end = fraction(entry["snapped_start"]), fraction(entry["written_end"])
    found, info = [], []
    counts = {"agree": 0, "disagree": 0}

    probe = ffprobe_json(ffprobe, "-show_format", "-show_streams", output)
    src_probe = cached((source, "probe"), lambda: ffprobe_json(ffprobe, "-show_format", "-show_streams", source))
    oh, sh = headers(output), cached((source, "headers"), lambda: headers(source))
    brand = probe["format"].get("tags", {}).get("major_brand")
    if "mp4" not in probe["format"].get("format_name", "") or brand != "isom":
        found.append(("format", f"output format {probe['format'].get('format_name')!r}, brand {brand!r}"))
    kept = ([("video", entry["video"])] if entry.get("video") else []) + (
        [("audio", entry["audio"])] if entry.get("audio") else [])
    if len(probe["streams"]) != len(kept) or not oh or len(oh["traks"]) != len(kept):
        found.append(("streams", f"output has {len(probe['streams'])} streams, expected {len(kept)}"))
        return [m for _, m in found], [], [], counts
    movie_timescale = oh["mvhd"][0]
    evidence = {"source_headers": sh, "output_headers": oh, "entry": entry, "ffmpeg": ffmpeg,
                "ffprobe": ffprobe, "output_streams": probe["streams"]}
    several_any = False

    for out_index, (kind, track) in enumerate(kept):
        src = next((s for s in src_probe["streams"] if s.get("index") == track["stream_index"]), None)
        out = probe["streams"][out_index]
        if src is None:
            found.append((f"{kind}:stream", f"{kind}: no source stream {track['stream_index']}"))
            continue
        for key in VIDEO_KEYS if kind == "video" else AUDIO_KEYS:
            if src.get(key) != out.get(key):
                found.append((f"{kind}:{key}", f"{kind} {key}: source {src.get(key)!r}, output {out.get(key)!r}"))
                if kind == "audio" and key == "profile":
                    evidence["audio_source_stream"] = src
                    evidence["audio_source_extradata"] = extradata(ffprobe, source, track["stream_index"])
                    evidence["audio_output_extradata"] = extradata(ffprobe, output, out_index)
        if kind == "video" and rotation(src) != rotation(out):
            found.append(("video:rotation", f"video rotation: source {rotation(src)}, output {rotation(out)}"))
        base = Fraction(out["time_base"]) if out.get("time_base") else None
        # Several edits on either side: the edit arithmetic of common
        # decision 3 (docs/design/edit-list-writers.md).
        src_edits = cached((source, "edits", track["stream_index"]),
                           lambda: full_edit_list(source, track["stream_index"]))
        out_edits = full_edit_list(output, out_index)
        if src_edits is not None and out_edits is not None and (
                several_edits(src_edits[2]) or several_edits(out_edits[2])):
            several_any = True
            agree, disagree = check_several(kind, entry, track, src, out, src_edits, out_edits, out_index,
                                            zero, end, ffprobe, ffmpeg, found, info, evidence)
            counts["agree"] += agree
            counts["disagree"] += disagree
            continue
        # Durations: the output's own edits, capped by its media duration,
        # as CoreAV reports a stream's duration, and for the
        # video the clip (whose media duration is never shorter than its edit).
        empty, segment = edit_span(oh["traks"][out_index], movie_timescale)
        timescale = oh["traks"][out_index]["mdhd"][0]
        media_duration = Fraction(oh["traks"][out_index]["mdhd"][1], timescale)
        expected_duration = media_duration if segment is None else min(segment, media_duration)
        # The edit itself, not capped by the media duration: the media
        # header could hide an edit one tick long.
        written = media_duration if segment is None else segment
        if kind == "video" and written != end - zero:
            found.append(("video:edit", f"video edit {float(written)} s, clip {float(end - zero)} s"))
        if kind == "audio" and out_index == 0 and empty + written != end - zero:
            found.append(("audio:edit", f"audio edits {float(empty + written)} s, clip {float(end - zero)} s"))
        try:
            reported = Fraction(out["duration_ts"]) * base
            if abs(reported - expected_duration) > Fraction(1, timescale):
                found.append((f"{kind}:duration", f"{kind} duration {float(reported):.6f} s, "
                              f"edits {float(expected_duration):.6f} s"))
            start_time = Fraction(out.get("start_time", "0"))
            if abs(start_time - empty) > Fraction(1, 1000):
                found.append((f"{kind}:start_time", f"{kind} start {float(start_time):.6f} s, "
                              f"empty edit {float(empty):.6f} s"))
        except (KeyError, TypeError, ValueError, ZeroDivisionError):
            found.append((f"{kind}:duration", f"{kind}: no duration"))
        # Packets.
        src_packets = cached((source, "packets", track["stream_index"]),
                             lambda: packets(ffprobe, source, track["stream_index"]))
        out_packets = packets(ffprobe, output, out_index)
        evidence[f"{kind}_source_packets"], evidence[f"{kind}_output_packets"] = src_packets, out_packets
        shift = zero / base if base else Fraction(0)
        evidence[f"{kind}_base"] = base
        agree, disagree = compare_packets(kind, src_packets, out_packets, track, shift, found, evidence)
        counts["agree"] += agree
        counts["disagree"] += disagree
        # Decoded content.
        if kind == "video":
            compare_video_frames(cached((source, "framemd5", track["stream_index"]),
                                        lambda: framemd5(ffmpeg, source, track["stream_index"], video=True)),
                                 framemd5(ffmpeg, output, out_index, video=True), zero, end, found, evidence)
        else:
            rate = int(src.get("sample_rate") or 0)
            if rate:
                channels = int(src.get("channels") or 1)
                out_pcm = pcm(ffmpeg, output, out_index, int(out.get("channels") or 1))
                evidence["audio_output_pcm"] = out_pcm
                check_audio_pcm(cached((source, "pcm", track["stream_index"]),
                                       lambda: pcm(ffmpeg, source, track["stream_index"], channels)),
                                out_pcm, zero, rate, found)

    # What the black boxes do not read, from both files' own boxes
    # (scripts/writer_boxes.py): samples, sync flags, sample entries,
    # timescales, header durations and the audio edit.
    found.extend(writer_boxes.check_clip(entry, several_any))
    writer_boxes.forget(output)

    if mediainfo:
        tracks = mediainfo_tracks(mediainfo, output)
        src_tracks = cached((source, "mediainfo"), lambda: mediainfo_tracks(mediainfo, source))
        general = [t for t in tracks if t.get("@type") == "General"]
        if [g.get("Format") for g in general] != ["MPEG-4"]:
            found.append(("mediainfo", f"MediaInfo: general {[g.get('Format') for g in general]}"))
        for out_index, (kind, track) in enumerate(kept):
            mi_type = "Video" if kind == "video" else "Audio"
            mi_out = [t for t in tracks if t.get("@type") == mi_type and t.get("StreamOrder") == str(out_index)]
            mi_src = [t for t in src_tracks if t.get("@type") == mi_type
                      and t.get("StreamOrder") == str(track["stream_index"])]
            if len(mi_out) != 1 or len(mi_src) != 1:
                found.append((f"mediainfo:{kind}", f"MediaInfo: {len(mi_out)} output and {len(mi_src)} "
                              f"source {kind} tracks"))
                continue
            for key in MEDIAINFO_VIDEO if kind == "video" else MEDIAINFO_AUDIO:
                if mi_out[0].get(key) != mi_src[0].get(key):
                    found.append((f"mediainfo:{kind}:{key}", f"MediaInfo {kind} {key}: source "
                                  f"{mi_src[0].get(key)!r}, output {mi_out[0].get(key)!r}"))
            # MediaInfo's duration follows the track header: the sum of the
            # output's edits.
            trak = oh["traks"][out_index]
            try:
                duration = float(mi_out[0]["Duration"])
                if abs(duration - trak["tkhd"] / movie_timescale) > 0.0015:
                    found.append((f"mediainfo:{kind}:Duration", f"MediaInfo {kind} duration {duration}, "
                                  f"track header {trak['tkhd'] / movie_timescale:.6f}"))
            except (KeyError, TypeError, ValueError):
                info.append(f"MediaInfo {kind}: no duration")

    problems, covered = [], []
    kds = known_divergences()
    fields = {field for field, _ in found}
    try:
        for field, message in found:
            # The box checks compare the files' own boxes; no known
            # divergence covers them (docs/design/writer-oracle-boxes.md).
            kd = None if field.startswith("box:") else next(
                (kd_id for kd_id, pattern, predicate in kds
                 if fnmatch.fnmatch(field, pattern) and predicate(field, fields, evidence)), None)
            if kd:
                covered.append({"kd": kd, "problem": message})
                # A covered mismatch counts as agreement;
                # missing and extra packets stay disagreements.
                if field.endswith(":packets"):
                    moved = evidence.get(f"{field.split(':')[0]}_mismatched", 0)
                    counts["agree"] += moved
                    counts["disagree"] -= moved
            else:
                problems.append(message)
    finally:
        if evidence.get("audio_output_pcm"):
            pathlib.Path(evidence["audio_output_pcm"][2]).unlink(missing_ok=True)
    return problems, info, covered, counts


# Known divergences of clips (docs/known-divergences-clip.md). Each predicate
# gets the field that differs, every field that differs on the clip and the
# collected evidence, and rechecks the entry's conditions; a near miss is
# never covered.

def _frames_but_edges(expected, actual, most, edge):
    """Whether `actual` is `expected` in order and unchanged, short of at
    most `most` frames, each within `edge` frames of an end."""
    if not actual or not 0 < len(expected) - len(actual) <= most:
        return False
    skipped, j = [], 0
    for i, frame in enumerate(expected):
        if j < len(actual) and actual[j] == frame:
            j += 1
        else:
            skipped.append(i)
    return j == len(actual) and all(i < edge or i >= len(expected) - edge for i in skipped)


def edit_midframe(field, fields, c):
    """KD-C-EDIT-MIDFRAME."""
    if field == "video:framemd5" and "video:packets" not in fields:
        # The clip's own edit starts inside a frame as well (it starts where
        # the source does), so ffmpeg moves both timelines alike and the
        # packets agree; only the frame that ffmpeg's timeline puts on the
        # other side of the clip's edit is missing.
        return _midframe_edge_frame(c)
    if field not in ("video:packets", "video:framemd5") or not c.get("video_run"):
        return False
    sh = c["source_headers"]
    stream = c["entry"]["video"]["stream_index"]
    if not sh or len(sh["traks"]) <= stream or not sh["traks"][stream].get("elst"):
        return False
    media = [t for _, t in sh["traks"][stream]["elst"] if t != -1]
    run, out, shift = c["video_run"]
    durations = sorted(p["duration"] for p in run if isinstance(p.get("duration"), int) and p["duration"] > 0)
    if len(media) != 1 or not durations or len(run) != len(out):
        return False
    frame = durations[len(durations) // 2]
    # The source's edit starts inside a frame.
    if media[0] % frame == 0:
        return False
    # Every packet the same, at the same constant offset smaller than a frame.
    delta = _constant_offset(run, out, shift)
    if delta is None or delta == 0 or abs(delta) >= durations[0]:
        return False
    if field == "video:packets":
        return True
    # The decoded frames agree once the same offset is applied.
    expected, actual, at_end = _frames_with_offset(c, delta)
    if bool(expected) and actual in (expected, expected + at_end):
        return True
    # With unsigned version 0 offsets as well, a few frames near the clip's
    # ends are left out as KD-C-CTTS-UNSIGNED describes.
    oh = c["output_headers"]
    return (bool(oh and oh["traks"] and oh["traks"][0].get("ctts_unsigned_large"))
            and _frames_but_edges(expected, actual, 4, 8))


def _midframe_edge_frame(c):
    """The head-clip case of KD-C-EDIT-MIDFRAME: a clip that starts where the
    source does, at a constant frame duration, whose own media edit and the
    source's both start inside a frame; the decoded frames equal the
    source's but for exactly one, the last, which on the edit's exact
    timeline lies past the clip's edit, within a frame of it. `ffmpeg`
    starts the source's timeline at the first frame boundary after the
    edit's media time, so a frame it places inside the clip's window lies
    that fraction of a frame later on the exact timeline. (A frame at the
    clip's start cannot be missing so: every frame `ffmpeg` places in the
    window starts after the edit's start on the exact timeline.)"""
    oh, sh = c["output_headers"], c["source_headers"]
    stream = c["entry"]["video"]["stream_index"]
    run = (c.get("video_run") or ([], [], 0))[0]
    durations = sorted(p["duration"] for p in run if isinstance(p.get("duration"), int) and p["duration"] > 0)
    if (not durations or not oh or not oh["traks"] or not oh["traks"][0].get("elst")
            or not sh or len(sh["traks"]) <= stream or not sh["traks"][stream].get("elst")):
        return False
    frame = durations[len(durations) // 2]
    # A constant frame duration (the last frame aside): otherwise the
    # median does not say where the next frame boundary is.
    run_durations = [p.get("duration") for p in run[:-1]]
    if any(d != frame for d in run_durations) or fraction(c["entry"]["snapped_start"]) != 0:
        return False
    # One media edit each, without an empty edit, whose time the exact
    # timeline below would not count.
    if len(sh["traks"][stream]["elst"]) != 1 or len(oh["traks"][0]["elst"]) != 1:
        return False
    source_media = [t for _, t in sh["traks"][stream]["elst"] if t != -1]
    clip_media = [(d, t) for d, t in oh["traks"][0]["elst"] if t != -1]
    # Both edits start inside a frame.
    if (len(source_media) != 1 or len(clip_media) != 1 or source_media[0] % frame == 0
            or clip_media[0][1] % frame == 0):
        return False
    expected, actual = c.get("video_frames") or ([], [])
    if not _frames_but_edges([x[1:] for x in expected], [x[1:] for x in actual], 1, 1):
        return False
    if [x[1:] for x in expected[:-1]] != [x[1:] for x in actual]:
        return False
    missing = expected[-1]
    timescale, movie_timescale = oh["traks"][0]["mdhd"][0], oh["mvhd"][0]
    if not timescale or not movie_timescale:
        return False
    length = Fraction(frame, timescale)
    later = Fraction(frame - source_media[0] % frame, timescale)
    exact = missing[0] + later
    clip = Fraction(clip_media[0][0], movie_timescale)
    return clip <= exact < clip + length


def _constant_offset(run, out, shift):
    """The one constant by which every source packet's `pts` and `dts` exceed
    the clip's (shifted by the clip's start), when every packet is the same
    in data, size and keyframe flag; else None."""
    if not run or len(run) != len(out):
        return None
    deltas = set()
    for a, b in zip(run, out):
        if (a.get("data_hash"), a.get("size"), (a.get("flags") or "")[:1]) != \
                (b.get("data_hash"), b.get("size"), (b.get("flags") or "")[:1]):
            return None
        for key in ("pts", "dts"):
            if not isinstance(a.get(key), int) or not isinstance(b.get(key), int):
                return None
            deltas.add(Fraction(a[key]) - (b[key] + shift))
    return deltas.pop() if len(deltas) == 1 else None


def _frames_with_offset(c, delta):
    """(expected, actual, at_end) decoded video frames, the source's taken
    `delta` source ticks later than the clip's window."""
    (sb, sf), (ob, of), zero, end = c["video_framemd5"]
    if sb is None or ob is None:
        return [], [], []
    offset = delta * sb
    begin, finish = zero + offset, end + offset
    expected = [(pts * sb - begin, size, h) for pts, size, h, _ in sf if begin <= pts * sb < finish]
    at_end = [(pts * sb - begin, size, h) for pts, size, h, _ in sf if pts * sb == finish]
    actual = [(pts * ob, size, h) for pts, size, h, _ in of]
    return expected, actual, at_end


def pts_shift(field, fields, c):
    """KD-C-PTS-SHIFT."""
    if field not in ("video:packets", "video:framemd5") or not c.get("video_run"):
        return False
    sh, oh = c["source_headers"], c["output_headers"]
    stream = c["entry"]["video"]["stream_index"]
    # No edit list in the source; a negative smallest offset in the clip.
    if not sh or len(sh["traks"]) <= stream or sh["traks"][stream].get("elst"):
        return False
    smallest = oh["traks"][0].get("ctts_min") if oh and oh["traks"] else None
    if smallest is None or smallest >= 0:
        return False
    # Every packet the same, the source's times later by that offset's size.
    run, out, shift = c["video_run"]
    if _constant_offset(run, out, shift) != -smallest:
        return False
    if field == "video:packets":
        return True
    expected, actual, at_end = _frames_with_offset(c, -smallest)
    return bool(expected) and actual in (expected, expected + at_end)


def _key(packet):
    return (packet.get("flags") or "")[:1] == "K"


def dropped_sync_flags(src, out, shift):
    """How many keyframe flags of `src` the packets `out` leave out, when
    `out` holds the packets of `src`, one for one, equal in data, size and
    times (`shift` in the stream's time base, as for the packet comparison),
    starts with a keyframe and has no flag `src` does not have; otherwise
    None. A packet listed only with edits ignored is compared on its data
    alone."""
    if not out or len(src) != len(out) or not _key(out[0]):
        return None
    tolerance = 0 if shift.denominator == 1 else 1
    dropped = 0
    for s, o in zip(src, out):
        if (o.get("data_hash"), o.get("size")) != (s.get("data_hash"), s.get("size")):
            return None
        if not (o.get("unpresented") or s.get("unpresented")):
            for key in ("pts", "dts"):
                a, b = s.get(key), o.get(key)
                if not isinstance(a, int) or not isinstance(b, int) or abs(Fraction(a) - (b + shift)) > tolerance:
                    return None
        if _key(o) and not _key(s):
            return None
        dropped += _key(s) and not _key(o)
    return dropped


def flags_decoded_keys(ffprobe, path, index, out):
    """Whether the keyframe flags of `out`, the packets of stream `index` of
    `path`, are those of the frames ffmpeg decodes as key frames, matched by
    byte position, edit lists ignored so that every frame is listed: a
    flagged packet decodes to a key frame, and no other packet does."""
    try:
        data = ffprobe_json(ffprobe, "-ignore_editlist", "1", "-select_streams", str(index),
                            "-show_entries", "frame=key_frame,pkt_pos", path)
    except (RuntimeError, ValueError):
        return False
    decoded = {}
    for frame in data.get("frames") or []:
        if frame.get("pkt_pos") is not None:
            decoded[str(frame["pkt_pos"])] = frame.get("key_frame") == 1
    for packet in out:
        key = decoded.get(str(packet.get("pos")))
        if (key is True) != _key(packet):
            return False
    return True


def sync_verified(field, fields, c):
    """KD-C-SYNC-VERIFIED."""
    if field != "video:packets" or not c.get("video_run") or not c.get("ffprobe"):
        return False
    run, out, shift = c["video_run"]
    dropped = dropped_sync_flags(run, out, shift)
    return bool(dropped) and flags_decoded_keys(c["ffprobe"], c["entry"]["output"], 0, out)


def ctts_unsigned(field, fields, c):
    """KD-C-CTTS-UNSIGNED."""
    if field != "video:framemd5" or not c.get("video_frames"):
        return False
    oh, sh = c["output_headers"], c["source_headers"]
    if not oh or not oh["traks"] or not oh["traks"][0].get("ctts_unsigned_large"):
        return False
    stream = c["entry"]["video"]["stream_index"]
    if not sh or len(sh["traks"]) <= stream or not sh["traks"][stream].get("ctts_unsigned_large"):
        return False
    expected, actual = c["video_frames"]
    # The output shows the source's frames in the clip, in order and
    # unchanged, short of a few within eight frames of its start or end.
    return _frames_but_edges(expected, actual, 4, 8)


def audio_index(c):
    """The audio's position in the output: after the video, or first in a
    clip without video."""
    return 1 if c["entry"].get("video") else 0


def audio_stream(c):
    """The output's audio stream as ffprobe lists it, {} when it has none."""
    streams = c.get("output_streams") or []
    return streams[audio_index(c)] if len(streams) > audio_index(c) else {}


def audio_trak(c):
    """The output's audio track headers, None when it has none."""
    oh = c["output_headers"]
    return oh["traks"][audio_index(c)] if oh and len(oh["traks"]) > audio_index(c) else None


def aac_profile(field, fields, c):
    """KD-C-AAC-PROFILE."""
    if field != "audio:profile" or not audio_stream(c):
        return False
    src, out = c.get("audio_source_stream") or {}, audio_stream(c)
    if src.get("codec_name") != "aac" or out.get("codec_name") != "aac":
        return False
    if src.get("profile") == out.get("profile"):
        return False
    # The configuration bytes, as ffprobe dumps them, are the same.
    a, b = c.get("audio_source_extradata"), c.get("audio_output_extradata")
    return bool(a) and a == b


def dts_shift(field, fields, c):
    """KD-C-DTS-SHIFT."""
    if field != "video:packets" or not c.get("video_run"):
        return False
    oh = c["output_headers"]
    if not oh or not oh["traks"] or not oh["traks"][0].get("ctts_reads_negative"):
        return False
    run, out, shift = c["video_run"]
    if not run or len(run) != len(out):
        return False
    deltas = set()
    for a, b in zip(run, out):
        if (a.get("data_hash"), a.get("size"), (a.get("flags") or "")[:1]) != \
                (b.get("data_hash"), b.get("size"), (b.get("flags") or "")[:1]):
            return False
        if not all(isinstance(x.get(k), int) for x in (a, b) for k in ("pts", "dts")):
            return False
        # Presentation times agree; decode times differ by one constant.
        if Fraction(a["pts"]) != b["pts"] + shift:
            return False
        deltas.add(Fraction(a["dts"]) - (b["dts"] + shift))
    # That constant is the cause's: each file's decode times moved back by
    # its own smallest composition offset, read from both files' `ctts`.
    expected = dts_move(c, oh["traks"][0])
    return len(deltas) == 1 and expected is not None and deltas.pop() == expected != 0


def dts_move(c, out_trak, source_track=None):
    """How much further `ffprobe` moves the source's decode times back than
    the output's: the source track's smallest composition offset less the
    output's, each counted where negative (`ctts_min` of both files' own
    headers); None without them."""
    traks = (c.get("source_headers") or {}).get("traks") or []
    index = (c["entry"].get("video") or {}).get("stream_index") if source_track is None else source_track
    if index is None or not 0 <= index < len(traks):
        return None
    src, out = traks[index].get("ctts_min"), out_trak.get("ctts_min")
    if src is None or out is None:
        return None
    return min(src, 0) - min(out, 0)


def stts_short(field, fields, c):
    """KD-C-STTS-SHORT."""
    if field != "video:duration":
        return False
    oh = c["output_headers"]
    trak = oh["traks"][0] if oh and oh["traks"] else None
    stream = (c.get("output_streams") or [None])[0]
    if not trak or not stream or not trak.get("elst") or len(trak["elst"]) != 1 or trak.get("stts_sum") is None:
        return False
    timescale, media_duration = trak["mdhd"]
    # The edit in media ticks (the movie timescale is a multiple of the
    # media timescale in a clip).
    segment = Fraction(trak["elst"][0][0] * timescale, oh["mvhd"][0])
    try:
        reported = int(stream["duration_ts"])
    except (KeyError, TypeError, ValueError):
        return False
    # ffprobe reports the decode durations, which are shorter than the edit;
    # the media header declares at least the edit's length, as the clip
    # writer writes it.
    return reported == trak["stts_sum"] < segment <= media_duration


def empty_edit_duration(field, fields, c):
    """KD-C-EMPTY-EDIT-DURATION."""
    kind = field.split(":", 1)[0]
    if field not in ("video:duration", "audio:duration"):
        return False
    index = 0 if kind == "video" else 1
    oh = c["output_headers"]
    streams = c.get("output_streams") or []
    if not oh or len(oh["traks"]) <= index or len(streams) <= index:
        return False
    trak, stream = oh["traks"][index], streams[index]
    edits = trak.get("elst") or []
    if len(edits) != 2 or edits[0][1] != -1 or edits[1][1] < 0 or trak.get("stts_sum") is None:
        return False
    movie_timescale = oh["mvhd"][0]
    timescale, media_duration = trak["mdhd"]
    empty, segment = edits[0][0], edits[1][0]
    # The output's own values must be the source's: its media timescale
    # (clips copy it), and decode durations that are the kept samples'
    # stored ones (the source's `stts`, not ffprobe's packet durations).
    kept = c["entry"].get(kind) or {}
    source_track = kept.get("stream_index")
    source_traks = (c.get("source_headers") or {}).get("traks") or []
    if source_track is None or source_track >= len(source_traks) \
            or source_traks[source_track].get("mdhd", (None,))[0] != timescale:
        return False
    first, count = kept.get("first_sample", -1), kept.get("samples", 0)
    stored = stts_window_sum(stored_durations(c["entry"]["source"], c.get("source_headers"), source_track,
                                              first + count), first, count)
    if kept.get("first_sample", -1) < 0 or stored is None or stored != trak["stts_sum"]:
        return False

    def ticks(movie_ticks):
        return (movie_ticks * timescale * 2 + movie_timescale) // (2 * movie_timescale)

    try:
        reported = int(stream["duration_ts"])
    except (KeyError, TypeError, ValueError):
        return False
    # ffprobe's duration: the smallest of the media header duration, the
    # decode durations and every edit's duration, the empty edit included
    # (the rule observed from the black box); the edit alone is
    # shorter.
    # Within one tick: the rule's last step rounds a half tick either way.
    expected = min(media_duration, trak["stts_sum"], ticks(empty + segment))
    return abs(reported - expected) <= 1 and abs(reported - ticks(segment)) > 1


def _adts_frames(data):
    frames, at = [], 0
    while at + 7 <= len(data):
        if data[at] != 0xFF or data[at + 1] & 0xF0 != 0xF0:
            return None
        length = ((data[at + 3] & 3) << 11) | (data[at + 4] << 3) | (data[at + 5] >> 5)
        if length < 7 or at + length > len(data):
            return None
        frames.append(data[at:at + length])
        at += length
    return frames if at == len(data) else None


# The raw format ffmpeg extracts a codec's samples in, and the one it reads
# them back as: AAC as ADTS frames, AC-3 and E-AC-3 as their syncframes.
RAW_AUDIO = {"aac": ("adts", "aac"), "ac3": ("ac3", "ac3"), "eac3": ("eac3", "eac3")}


def _sized_frames(data, sizes):
    """`data` cut into consecutive pieces of `sizes` bytes, or None when
    they do not add up to it."""
    frames, at = [], 0
    for size in sizes:
        frames.append(data[at:at + size])
        at += size
    return frames if at == len(data) else None


def kept_audio_decoded(c):
    """The source's kept audio packets, extracted by ffmpeg in their raw
    format (one frame or sample per sample, edit lists ignored), cut at the
    clip's first kept packet and decoded from there to float32 PCM: the path
    of a temporary file, or None."""
    audio = c["entry"]["audio"]
    ffmpeg = c["ffmpeg"]
    raw = RAW_AUDIO.get(audio_stream(c).get("codec_name"))
    if raw is None:
        return None
    source, index = c["entry"]["source"], audio["stream_index"]
    try:
        data = run([ffmpeg, "-v", "error", "-ignore_editlist", "1", "-i", source,
                    "-map", f"0:{index}", "-c", "copy", "-f", raw[0], "-"])
    except RuntimeError:
        return None
    if raw[0] == "adts":
        frames = _adts_frames(data)
    else:
        # Syncframes carry no sample boundaries; the source's stored sample
        # sizes give them.
        stored = cached((source, "raw packets", index),
                        lambda: stored_times(packets(c["ffprobe"], source, index, raw=True)))
        frames = _sized_frames(data, [int(p.get("size") or 0) for p in stored])
    first, count = audio["first_sample"], audio["samples"]
    if frames is None or len(frames) < first + count:
        return None
    tail = b"".join(frames[first:first + count])
    fd, decoded = tempfile.mkstemp(suffix=".f32", dir=_work())
    os.close(fd)
    result = subprocess.run([ffmpeg, "-v", "error", "-y", "-f", raw[1], "-i", "-", "-c:a", "pcm_f32le",
                             "-f", "f32le", decoded], input=tail, capture_output=True, check=False)
    if result.returncode != 0:
        pathlib.Path(decoded).unlink(missing_ok=True)
        return None
    return decoded


def _audio_window_kept(c):
    """Whether the clip kept the source's audio packets the clip rule names, judged from the source's own packets and times rather than from the
    clip's summary: one packet of pre-roll before the first packet presented
    at the snapped start (a discarded priming packet included; none before
    the track's first), every packet presented inside the clip, and none
    beyond the one presented at its end. Within one tick, as
    the audio packets are compared. KD-C-AUDIO-RESTART decodes the kept
    packets themselves, so it needs this to see which packets were kept."""
    src = c.get("audio_source_packets")
    run = (c.get("audio_run") or ([], [], 0))[0]
    start, base = c.get("audio_run_start"), c.get("audio_base")
    if not src or not run or start is None or not base:
        return False
    zero, end = fraction(c["entry"]["snapped_start"]), fraction(c["entry"]["written_end"])
    shift = zero / base
    stop = shift + (end - zero) / base
    tolerance = 1

    def span(p):
        if not isinstance(p.get("pts"), int) or not isinstance(p.get("duration"), int):
            return None
        return Fraction(p["pts"]), Fraction(p["pts"] + p["duration"])

    spans = [span(p) for p in run]
    if any(x is None for x in spans):
        return False
    # Pre-roll: the run starts one packet before the first packet presented
    # at the snapped start (a discarded priming packet included, never
    # before the track's first). A packet that ends at the start, within a
    # tick, allows either.
    presented = [i for i, p in enumerate(src) if "D" not in (p.get("flags") or "")]
    expected = set()
    for bound in (shift - tolerance, shift + tolerance):
        first = next((i for i in presented if (span(src[i]) or (0, 0))[1] > bound), None)
        if first is not None:
            expected.add(max(first - 1, 0))
    if start not in expected:
        return False
    # The end: the last kept packet is presented before the end and reaches
    # it, unless it is the track's last; the next one starts at or after it.
    last_start, last_end = spans[-1]
    following = src[start + len(run):start + len(run) + 1]
    if last_start >= stop + tolerance:
        return False
    if following:
        next_span = span(following[0])
        if next_span is None or next_span[0] < stop - tolerance or last_end < stop - tolerance:
            return False
    return True


def audio_restart(field, fields, c):
    """KD-C-AUDIO-RESTART."""
    if field != "audio:pcm" or not c["entry"].get("audio") or not c.get("audio_output_pcm"):
        return False
    trak = audio_trak(c)
    if not trak or not trak.get("elst"):
        return False
    media = [t for d, t in trak["elst"] if t != -1]
    if len(media) != 1:
        return False
    if not _audio_timing_is_the_sources(c, trak, media[0]):
        return False
    decoded = kept_audio_decoded(c)
    if decoded is None:
        return False
    try:
        return _restart_matches(c, trak, media, decoded)
    finally:
        pathlib.Path(decoded).unlink(missing_ok=True)


def audio_jitter(field, fields, c):
    """KD-C-AUDIO-JITTER."""
    if field != "audio:pcm" or not c["entry"].get("audio") or not c.get("audio_output_pcm"):
        return False
    run_packets = (c.get("audio_run") or ([], [], 0))[0]
    spans, channels, output = c["audio_output_pcm"]
    frame_samples = {samples for _, samples in spans}
    durations = {p.get("duration") for p in run_packets}
    rate = int(audio_stream(c).get("sample_rate") or 0)
    trak = audio_trak(c)
    if len(frame_samples) != 1 or not run_packets or not trak or trak["mdhd"][0] != rate:
        return False
    samples = frame_samples.pop()
    # The stored decode durations are not the decoded frames' length.
    if durations <= {samples} or None in durations:
        return False
    media = [t for d, t in trak.get("elst") or [] if t != -1]
    if len(media) != 1 or not _audio_timing_is_the_sources(c, trak, media[0]):
        return False
    decoded = kept_audio_decoded(c)
    if decoded is None:
        return False
    try:
        frame = 4 * channels
        for offset in range(0, 2 * samples + 1):
            if same_bytes(decoded, offset * frame, output):
                return True
        return False
    finally:
        pathlib.Path(decoded).unlink(missing_ok=True)


def _audio_timing_is_the_sources(c, trak, media_time):
    """Whether the clip's audio timing is the source's, judged from the
    files themselves. The audio entries decode the clip's own kept
    packets at the clip's own times, so wrong times would agree with
    themselves; this checks them apart: the kept packets are those the clip
    rule names, the edit's media time is where the source presents the clip's
    start, and the decode durations are exactly those the source's `stts`
    stores for the kept samples."""
    kept = c["entry"].get("audio") or {}
    traks = (c.get("source_headers") or {}).get("traks") or []
    index, first = kept.get("stream_index"), kept.get("first_sample", -1)
    if index is None or not 0 <= index < len(traks) or first < 0:
        return False
    count = kept.get("samples", 0)
    window = stts_window(stored_durations(c["entry"]["source"], c.get("source_headers"), index, first + count),
                         first, count)
    if not window or merged_stts(trak.get("stts")) != window:
        return False
    return _audio_window_kept(c) and _audio_media_time_is_the_sources(c, media_time)


def _audio_media_time_is_the_sources(c, media_time):
    """Whether the output audio edit's media time is where the source
    presents the output's audio start, from the files' own boxes: the
    output's empty edit gives that start (zero plus its duration), the
    source's edit list maps it to a media time (ISO/IEC 14496-12 8.6.6), and
    the output's decode times start at the first kept packet's stored decode
    time; rounded to the nearest tick, as the clip writer rounds it."""
    audio = c["entry"].get("audio") or {}
    source, output = c["entry"]["source"], c["entry"]["output"]
    src_edits, out_edits = full_edit_list(source, audio.get("stream_index", -1)), full_edit_list(output, audio_index(c))
    if not src_edits or not out_edits or not audio.get("first_pos"):
        return False
    movie_out, media_out, out_entries = out_edits
    empty = sum(Fraction(d, movie_out) for d, t, _ in out_entries[:1] if t == -1)
    movie_src, media_src, src_entries = src_edits
    if media_out != media_src:
        return False
    at = fraction(c["entry"]["snapped_start"]) + empty
    windows = iso_windows(movie_src, media_src, src_entries) if src_entries else [(Fraction(0), None, Fraction(0))]
    if len(windows) != 1:
        return False
    m0, _, start = windows[0]
    source_media = m0 + at * media_src - start
    raw = cached((source, "raw packets", audio["stream_index"]),
                 lambda: stored_times(packets(c["ffprobe"], source, audio["stream_index"], raw=True)))
    first = next((p for p in raw if p.get("pos") == str(audio["first_pos"])), None)
    if first is None or not isinstance(first.get("dts"), int):
        return False
    exact = source_media - first["dts"]
    return media_time == int(exact + Fraction(1, 2))


def _restart_matches(c, trak, media, decoded):
    _, channels, output = c["audio_output_pcm"]
    frame = 4 * channels
    size = os.path.getsize(output)
    if not size or size % frame:
        return False
    # The output presents the decoded samples from its edit's media time on,
    # in samples when the media timescale is not the sample rate.
    rate = int(audio_stream(c).get("sample_rate") or 0)
    timescale = trak["mdhd"][0]
    if not rate or not timescale:
        return False
    start = Fraction(media[0] * rate, timescale)
    for offset in sorted({int(start), round(start)} if start.denominator != 1 else {int(start)}):
        if same_bytes(decoded, offset * frame, output):
            return True
    return False


def ffmpeg_audio_edit_start(media_time, frame, ratio):
    """Where ffmpeg starts presenting an audio track decoded at `ratio`
    times its media timescale, in decoded samples, for one media edit at
    `media_time` ticks over frames of `frame` ticks (KD-C-AUDIO-EDIT-RATE):
    the whole frames before the media time are dropped, and the rest is
    skipped only within the first frame, as that many decoded samples."""
    whole = media_time // frame
    return whole * frame * ratio if whole else media_time


def audio_edit_rate(field, fields, c):
    """KD-C-AUDIO-EDIT-RATE."""
    if field != "audio:pcm" or not c["entry"].get("audio") or not c.get("audio_output_pcm"):
        return False
    trak = audio_trak(c)
    if not trak or not trak.get("elst"):
        return False
    media = [t for d, t in trak["elst"] if t != -1]
    rate = int(audio_stream(c).get("sample_rate") or 0)
    timescale = trak["mdhd"][0]
    durations = {p.get("duration") for p in (c.get("audio_run") or ([], [], 0))[0]}
    if (len(media) != 1 or not timescale or rate <= timescale or rate % timescale
            or len(durations) != 1):
        return False
    frame = durations.pop()
    # On a whole frame ffmpeg starts where the edit does.
    if not isinstance(frame, int) or frame <= 0 or media[0] % frame == 0:
        return False
    if not _audio_timing_is_the_sources(c, trak, media[0]):
        return False
    _, channels, output = c["audio_output_pcm"]
    size = os.path.getsize(output)
    if not size or size % (4 * channels):
        return False
    decoded = kept_audio_decoded(c)
    if decoded is None:
        return False
    try:
        start = ffmpeg_audio_edit_start(media[0], frame, rate // timescale)
        return same_bytes(decoded, start * 4 * channels, output)
    finally:
        pathlib.Path(decoded).unlink(missing_ok=True)


PREDICATES = {
    "ctts_unsigned": ctts_unsigned,
    "edit_midframe": edit_midframe,
    "aac_profile": aac_profile,
    "dts_shift": dts_shift,
    "stts_short": stts_short,
    "empty_edit_duration": empty_edit_duration,
    "audio_restart": audio_restart,
    "audio_jitter": audio_jitter,
    "audio_edit_rate": audio_edit_rate,
    "pts_shift": pts_shift,
    "sync_verified": sync_verified,
}


def known_divergences():
    if not KD_FILE.exists():
        return []
    entries = json.loads(KD_FILE.read_text(encoding="utf-8"))["entries"]
    return [(e["id"], e["field_pattern"], PREDICATES[e["predicate"]]) for e in entries]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--mediainfo")
    parser.add_argument("--report")
    args = parser.parse_args()

    with open(args.manifest, encoding="utf-8") as f:
        manifest = json.load(f)
    results = []
    previous = None
    for entry in manifest:
        if entry["source"] != previous:
            clear_cache()
            previous = entry["source"]
        try:
            problems, info, covered, counts = check(entry, args.ffprobe, args.ffmpeg, args.mediainfo)
        except (RuntimeError, ValueError, KeyError) as error:
            problems, info, covered, counts = [f"harness: {error}"], [], [], {"agree": 0, "disagree": 0}
        results.append({**entry, "problems": problems, "informational": info, "covered": covered,
                        "packets": counts})
        name = f"{entry['output'].rsplit('/', 1)[-1]}"
        print(f"{'FAIL' if problems else 'kd  ' if covered else 'ok  '} {name}")
        for problem in problems:
            print(f"     {problem}")
        for item in covered:
            print(f"     ({item['kd']}) {item['problem']}")
        for difference in info:
            print(f"     (informational) {difference}")
    clear_cache()
    passed = sum(1 for r in results if not r["problems"])
    direct = sum(1 for r in results if not r["problems"] and not r["covered"])
    agree = sum(r["packets"]["agree"] for r in results)
    disagree = sum(r["packets"]["disagree"] for r in results)
    print(f"{passed}/{len(results)} clips pass every oracle "
          f"({direct} directly, {passed - direct} through known divergences); "
          f"packets {agree}/{agree + disagree} agree")
    if args.report:
        with open(args.report, "w", encoding="utf-8") as f:
            json.dump({"passed": passed, "direct": direct, "total": len(results),
                       "packets": {"agree": agree, "disagree": disagree}, "results": results}, f, indent=2)
    return 0 if results and passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
