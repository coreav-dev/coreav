#!/usr/bin/env python3
"""Black-box oracles for audio extraction.

Reads the manifest that the `fixtures_round_trip` test of
coreav-isobmff-write writes (COREAV_EXTRACT_OUT) and checks every output
against its source with the pinned ffprobe and ffmpeg binaries, and with
MediaInfo when --mediainfo is given:

- ffprobe reads the output as an M4A file (major brand `M4A `) with one AAC
  stream whose codec, profile, sample rate, channel layout, start time and
  duration equal the source stream's, and whose frame count equals the
  number of source packets (ffprobe counts no frames, or only the movie box
  samples, in a fragmented source);
- `ffprobe -show_packets -show_data_hash` lists the same packets (data hash,
  size, pts, dts) for the output as for the source stream. Packet durations
  are informational, as in the oracle harness: for fragmented
  sources ffprobe's packet durations do not follow the stored sample
  durations at the first and last sample, while the output's are stored
  in its sample table. The coreav round trip checks every duration exactly;
- `ffmpeg -copyts -f framemd5` of the decoded output equals that of the
  source stream (on each file's own presentation times);
- MediaInfo reports an MPEG-4 file whose audio track has the same format,
  sample rate, channels, duration and frame count as the source track.

A difference counts as agreement only when an entry of
docs/known-divergences-extract.md covers it: each entry's
predicate below rechecks its conditions from the two files' box headers and
the oracles' own output. Covered differences are reported apart from direct
agreement and never hide another difference of the same output.

With --media, every AAC stream that ffprobe finds in an MP4, MOV or M4A file
there must have an entry in the manifest, unless the oracle overlay
(--kd-overlay, from scripts/oracle_kd.py) shows KD-STRICT on that file:
CoreAV rejects it as truncated, invalid or over a limit, so it has no
streams to extract. Coverage can therefore not shrink silently.

The binaries are black boxes; no implementation source
is read. Exits 1 if any check fails or the manifest is empty.
"""

import argparse
import json
import pathlib
import struct
import subprocess
import sys
from bisect import bisect_left
from fractions import Fraction

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import writer_boxes  # noqa: E402

IN_SCOPE = (".mp4", ".mov", ".m4a")

STREAM_KEYS = (
    "codec_type",
    "codec_name",
    "profile",
    "sample_rate",
    "channels",
    "channel_layout",
    "start_pts",
    "start_time",
    "duration_ts",
    "duration",
    "time_base",
)
PACKET_KEYS = ("data_hash", "size", "pts", "dts")
PACKET_INFO = ("duration",)
MEDIAINFO_KEYS = (
    "Format",
    "Format_AdditionalFeatures",
    "SamplingRate",
    "Channels",
    "Duration",
    "FrameCount",
)
KD_FILE = pathlib.Path(__file__).resolve().parent.parent / "docs" / "known-divergences-extract.json"


def run(args):
    result = subprocess.run(args, capture_output=True, check=False)
    if result.returncode != 0:
        raise RuntimeError(f"{args[0]} failed ({result.returncode}): {result.stderr.decode(errors='replace')[:400]}")
    return result.stdout


def ffprobe_json(ffprobe, *args):
    return json.loads(run([ffprobe, "-v", "error", "-of", "json", *args]))


def stream(ffprobe, path, index):
    data = ffprobe_json(ffprobe, "-show_format", "-show_streams", path)
    matches = [s for s in data["streams"] if s.get("index") == index]
    if len(matches) != 1:
        raise RuntimeError(f"{path}: no stream {index}")
    return matches[0], data["format"]


def packets(ffprobe, path, index, raw=False):
    data = ffprobe_json(
        ffprobe,
        *(("-ignore_editlist", "1") if raw else ()),
        "-select_streams",
        str(index),
        "-show_packets",
        "-show_data_hash",
        "SHA256",
        "-show_entries",
        "packet=" + ",".join(PACKET_KEYS + PACKET_INFO),
        path,
    )
    return data.get("packets", [])


def framemd5(ffmpeg, path, index, raw=False):
    # `-copyts` keeps the file's own presentation times. Without it ffmpeg
    # moves the earliest start among all of a file's streams to 0, so a
    # source whose video starts before its audio would be compared on a
    # different origin than its one-stream output. With `raw`, the edit list
    # is ignored, so the frames come at their stored times and the harness
    # applies the edits itself (docs/design/edit-list-writers.md, common
    # decision 3).
    out = run([ffmpeg, "-v", "error", "-copyts", *(("-ignore_editlist", "1") if raw else ()),
               "-i", path, "-map", f"0:{index}", "-f", "framemd5", "-"])
    return [line for line in out.decode().splitlines() if line and not line.startswith("#")]


def mediainfo_audio(mediainfo, path, stream_order):
    data = json.loads(run([mediainfo, "--Output=JSON", path]))
    tracks = data["media"]["track"]
    general = [t for t in tracks if t.get("@type") == "General"]
    audio = [t for t in tracks if t.get("@type") == "Audio"]
    if stream_order is not None:
        audio = [t for t in audio if t.get("StreamOrder") == str(stream_order)]
    return general, audio


# Box headers, read independently of CoreAV (ISO/IEC 14496-12 4.2): the
# movie header, and per track the track header duration, the edit list, the
# media header and the handler (8.2.2, 8.3.2, 8.6.6, 8.4.2, 8.4.3).


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


def _top_level_moov(path, cap=64 << 20):
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
            if kind == b"moov":
                if size > cap:
                    return None
                f.seek(at + header)
                return f.read(size - header)
            at += size
    return None


def headers(path):
    """{'mvhd': (timescale, duration), 'traks': [...]}, or None."""
    moov = _top_level_moov(path)
    if moov is None:
        return None
    out = {"mvhd": None, "traks": []}
    try:
        for kind, body in _children(moov):
            if kind == "mvhd":
                out["mvhd"] = struct.unpack(">IQ", body[20:32]) if body[0] else struct.unpack(">II", body[12:20])
            if kind != "trak":
                continue
            trak = {"elst": None}
            for k2, b2 in _children(body):
                if k2 == "tkhd":
                    trak["tkhd"] = struct.unpack(">Q", b2[28:36])[0] if b2[0] else struct.unpack(">I", b2[20:24])[0]
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
            out["traks"].append(trak)
    except (struct.error, IndexError):
        return None
    return out


MAX_ELST_BYTES = 8 << 20


def full_elst(path, index):
    """(movie timescale, media timescale, every entry of the track's edit
    list) as stored, `(segment_duration, media_time)`; `None` when the file
    cannot be read. Read here from ISO/IEC 14496-12 8.2.2, 8.4.2 and 8.6.6,
    not from `headers`, which keeps the first eight entries only."""
    moov = _top_level_moov(path, MAX_ELST_BYTES)
    if moov is None:
        return None
    movie_ts, traks = None, []
    try:
        for kind, body in _children(moov):
            if kind == "mvhd":
                movie_ts = struct.unpack(">I", body[20:24] if body[0] else body[12:16])[0]
            elif kind == "trak":
                traks.append(body)
        if not movie_ts or not 0 <= index < len(traks):
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


def several_apply(movie_ts, media_ts, entries):
    """Whether a list of several media edits is inside the model CoreAV
    applies (docs/design/edit-lists.md, decision 1): every entry an empty
    edit — whatever its rate, which presents nothing either way — or a
    media edit at rate 1 with a duration, and the media edits' windows in
    increasing order without overlap."""
    if not entries or not movie_ts or not media_ts:
        return False
    if sum(duration for duration, _, _ in entries) > (1 << 64) - 1:
        return False
    end = None
    for duration, media_time, rate in entries:
        if media_time == -1:
            continue
        if media_time < 0 or rate != 0x10000 or duration == 0:
            return False
        start = Fraction(media_time)
        if end is not None and start < end:
            return False
        end = start + Fraction(duration * media_ts, movie_ts)
    return end is not None


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
    those whose composition interval overlaps the window's media interval, at
    their composition time less the window's media start plus its
    presentation start; in media ticks, sorted."""
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


def first_stored_dts(ps):
    """The first packet's stored decode time, `None` when there is none."""
    try:
        return int(ps[0]["dts"]) if ps else None
    except (KeyError, TypeError, ValueError):
        return None


def check_several(source, index, output, ffprobe, ffmpeg, found):
    """Common decision 3 for a source with several media edits: `ffmpeg`
    lists the samples and decodes the frames with edit lists ignored, and the
    edit arithmetic is done here from each file's own boxes (ISO/IEC
    14496-12 8.6.6).

    - the stored samples of the output are the source's, by hash and size, at
      the source's stored times less the source's first decode time;
    - the frames the output's edits present are the ones the source's edits
      present, at the same times.
    """
    src_full, out_full = full_elst(source, index), full_elst(output, 0)
    if not src_full or not out_full:
        found.append(("several:boxes", "the edit list could not be read from both files"))
        return
    src_packets = packets(ffprobe, source, index, raw=True)
    out_packets = packets(ffprobe, output, 0, raw=True)
    def times(ps, shift):
        """(hash, size, pts, dts) of each packet, or None when `ffprobe`
        gives no usable time: the comparison then fails rather than the
        run."""
        out = []
        for p in ps:
            try:
                out.append((p.get("data_hash"), p.get("size"),
                            int(p["pts"]) - shift, int(p["dts"]) - shift))
            except (KeyError, TypeError, ValueError):
                return None
        return out
    first = first_stored_dts(src_packets)
    if first is None:
        found.append(("several:packets", "the source has no packet with a decode time"))
        return
    want, got = times(src_packets, first), times(out_packets, 0)
    if want is None or got is None:
        found.append(("several:packets", "ffprobe gives a packet without a time"))
        return
    if not want or want != got:
        differing = next((i for i, (a, b) in enumerate(zip(want, got)) if a != b), min(len(want), len(got)))
        found.append(("several:packets",
                      f"stored samples differ at {differing} ({len(want)} source, {len(got)} output)"))
    src_frames = _frames(framemd5(ffmpeg, source, index, raw=True))
    out_frames = _frames(framemd5(ffmpeg, output, 0, raw=True))
    if src_frames is None or out_frames is None:
        found.append(("several:framemd5", "framemd5 with edit lists ignored could not be read"))
        return
    def shown(frames, full, shift):
        """The frames the file's edits present, in the output's media
        coordinates: the source's times and media times both move by its
        first decode time, which the output's start at 0."""
        movie_ts, media_ts, entries = full
        moved = [(duration, media_time if media_time == -1 else media_time - shift, rate)
                 for duration, media_time, rate in entries]
        windows = iso_windows(movie_ts, media_ts, moved)
        return iso_presented([(Fraction(pts) - shift, Fraction(duration), (size, digest))
                              for _dts, pts, duration, size, digest in frames], windows)
    want, got = shown(src_frames, src_full, first), shown(out_frames, out_full, 0)
    if not want or want != got:
        differing = next((i for i, (a, b) in enumerate(zip(want, got)) if a != b), min(len(want), len(got)))
        found.append(("several:frames",
                      f"the frames the edits present differ at {differing} "
                      f"({len(want)} source, {len(got)} output)"))


def itunsmpb(fmt):
    """(delay, padding, samples) of an `iTunSMPB` tag as ffprobe lists it."""
    fields = (fmt.get("tags") or {}).get("iTunSMPB", "").split()
    try:
        return tuple(int(f, 16) for f in fields[1:4]) if len(fields) >= 4 else None
    except ValueError:
        return None


def _frames(lines):
    """framemd5 lines as (dts, pts, duration, size, hash), or None."""
    out = []
    for line in lines:
        parts = [p.strip() for p in line.split(",")]
        if len(parts) != 6:
            return None
        try:
            out.append((int(parts[1]), int(parts[2]), int(parts[3]), int(parts[4]), parts[5]))
        except ValueError:
            return None
    return out


# Known divergences of extraction (docs/known-divergences-extract.md). Each
# predicate gets the field that differs and the collected evidence and
# rechecks the entry's conditions; a near miss is never covered.


def tkhd_follows_edits(field, c):
    """KD-X-TKHD."""
    if field not in ("mediainfo:Duration", "mediainfo:FrameCount"):
        return False
    sh, oh, src = c["source_headers"], c["output_headers"], c["source_trak"]
    if not sh or not oh or len(oh["traks"]) != 1 or not oh["mvhd"] or not sh["mvhd"] or src is None:
        return False
    out = oh["traks"][0]
    if "tkhd" not in out or "tkhd" not in src or out["tkhd"] == src["tkhd"]:
        return False
    if out["elst"]:
        # `headers` keeps the first eight entries only; a list of several
        # media edits needs every one of them.
        out_full = c.get("output_elst")
        entries = out_full[2] if out_full and out_full[2] else None
        expected = (sum(duration for duration, _mt, _rate in entries) if entries
                    else sum(duration for duration, _ in out["elst"]))
    else:
        timescale, duration = out["mdhd"]
        expected = (duration * oh["mvhd"][0] + timescale // 2) // timescale
    if out["tkhd"] != expected:
        return False
    # MediaInfo reports the output's track header duration and, for the
    # source, its track header duration or (for example in a fragmented
    # file, whose header durations cover no samples) the media duration,
    # which both files share and the output's track header rounds; to the ms.
    try:
        reported = (float(c["mi_source"]["Duration"]), float(c["mi_output"]["Duration"]))
    except (KeyError, TypeError, ValueError):
        return False
    candidates = [src["tkhd"] / sh["mvhd"][0]]
    if out.get("mdhd") and out["mdhd"][0]:
        candidates.append(out["mdhd"][1] / out["mdhd"][0])
    return (any(abs(reported[0] - value) <= 0.0015 for value in candidates)
            and abs(reported[1] - out["tkhd"] / oh["mvhd"][0]) <= 0.0015)


def edit_discard(field, c):
    """KD-X-EDIT-DISCARD."""
    if field != "nb_frames" or "packets" in c["fields"]:
        return False
    src_n, out_n = c["source_stream"].get("nb_frames"), c["output_stream"].get("nb_frames")
    listed = len(c["source_packets"])
    return (src_n is not None and src_n == out_n and listed < int(src_n)
            and len(c["output_packets"]) == listed)


def edit_multi(field, c):
    """KD-X-EDIT-MULTI."""
    if field != "nb_frames" or "packets" in c["fields"] or not c.get("several"):
        return False
    src_full, out_full = c.get("source_elst"), c.get("output_elst")
    first = c.get("first_dts")
    if not src_full or not out_full or first is None:
        return False
    # The output keeps the source's timescales and its entries, with every
    # media time moved by the first decode time: the values are read from
    # both files, never from the output alone.
    if src_full[0] != out_full[0] or src_full[1] != out_full[1]:
        return False
    if len(src_full[2]) != len(out_full[2]):
        return False
    for (sd, sm, sr), (od, om, orate) in zip(src_full[2], out_full[2]):
        if sd != od or sr != orate:
            return False
        if sm == -1:
            if om != -1:
                return False
        elif om != sm - first:
            return False
    src_n, out_n = c["source_stream"].get("nb_frames"), c["output_stream"].get("nb_frames")
    listed = len(c["source_packets"])
    return (src_n is not None and src_n == out_n and listed > int(src_n)
            and len(c["output_packets"]) == listed)


def gapless_edit(field, c):
    """KD-X-GAPLESS-EDIT."""
    smpb, src, oh = c["smpb"], c["source_trak"], c["output_headers"]
    if not smpb or src is None or src.get("elst") or src.get("mdhd") is None or not oh or len(oh["traks"]) != 1:
        return False
    delay, padding, samples = smpb
    out = oh["traks"][0]
    if not out.get("elst") or len(out["elst"]) != 1 or out["elst"][0][1] != delay:
        return False
    if delay + padding + samples != src["mdhd"][1]:
        return False
    ss, os_ = c["source_stream"], c["output_stream"]
    try:
        rate = int(ss["sample_rate"])
        if ss.get("time_base") != f"1/{rate}" or os_.get("time_base") != f"1/{rate}":
            return False
        if field == "stream:start_pts":
            return ss["start_pts"] == os_["start_pts"] + delay
        if field == "stream:start_time":
            return abs(float(ss["start_time"]) - float(os_["start_time"]) - delay / rate) < 1e-5
        if field == "stream:duration_ts":
            return ss["duration_ts"] == delay + padding + samples and os_["duration_ts"] == samples
        if field == "stream:duration":
            return (abs(float(ss["duration"]) - (delay + padding + samples) / rate) < 1e-5
                    and abs(float(os_["duration"]) - samples / rate) < 1e-5)
    except (KeyError, TypeError, ValueError):
        return False
    if field == "packets":
        a, b = c["source_packets"], c["output_packets"]
        return len(a) == len(b) and all(
            x.get("data_hash") == y.get("data_hash") and x.get("size") == y.get("size")
            and isinstance(y.get("pts"), int) and isinstance(y.get("dts"), int)
            and x.get("pts") == y["pts"] + delay and x.get("dts") == y["dts"] + delay
            for x, y in zip(a, b))
    if field == "framemd5":
        a, b = _frames(c["source_frames"]), _frames(c["output_frames"])
        return bool(a) and b is not None and len(a) == len(b) and all(
            x[2:] == y[2:] and x[0] == y[0] + delay and x[1] == y[1] + delay for x, y in zip(a, b))
    return False


def gapless_conflict(field, c):
    """KD-X-GAPLESS-CONFLICT."""
    smpb, src, oh = c["smpb"], c["source_trak"], c["output_headers"]
    if field != "framemd5" or not smpb or src is None or not src.get("elst") or src.get("mdhd") is None:
        return False
    if sum(smpb) == src["mdhd"][1] or not oh or len(oh["traks"]) != 1:
        return False
    out = oh["traks"][0]
    if not out.get("elst") or [e[0] for e in out["elst"]] != [e[0] for e in src["elst"]]:
        return False
    a, b = _frames(c["source_frames"]), _frames(c["output_frames"])
    if not a or not b or len(a) < 3:
        return False
    # Every frame of the source but its first and last, in order and
    # unchanged, is in the output.
    inner = [x[2:] for x in a[1:-1]]
    decoded = [y[2:] for y in b]
    return any(decoded[i:i + len(inner)] == inner for i in range(len(decoded) - len(inner) + 1))


def fragment_last_frame(field, c):
    """KD-X-FRAG-LAST-FRAME."""
    if field != "framemd5" or not c.get("fragmented"):
        return False
    a, b = _frames(c["source_frames"]), _frames(c["output_frames"])
    packets = c["output_packets"]
    if not a or b is None or len(a) != len(b) or len(packets) != len(c["source_packets"]):
        return False
    stored = packets[-1].get("duration") if packets else None
    return (a[:-1] == b[:-1] and a[-1][:2] == b[-1][:2]
            and c["source_packets"][-1].get("duration") == stored
            and b[-1][2] == stored and a[-1][2] > stored)


def profile_unnamed(field, c):
    """KD-X-PROFILE-UNNAMED."""
    return (field == "stream:profile" and c["source_stream"].get("profile") is None
            and c["output_stream"].get("profile") == "LC"
            and c["source_stream"].get("codec_name") == c["output_stream"].get("codec_name") == "aac")


PREDICATES = {
    "tkhd_follows_edits": tkhd_follows_edits,
    "edit_discard": edit_discard,
    "edit_multi": edit_multi,
    "gapless_edit": gapless_edit,
    "gapless_conflict": gapless_conflict,
    "fragment_last_frame": fragment_last_frame,
    "profile_unnamed": profile_unnamed,
}


def known_divergences():
    entries = json.loads(KD_FILE.read_text(encoding="utf-8"))["entries"]
    return [(e["id"], PREDICATES[e["predicate"]]) for e in entries]


def check(entry, ffprobe, ffmpeg, mediainfo):
    """Returns the gating problems, the informational differences and the
    differences covered by a known divergence ({'kd', 'problem'})."""
    source, index, output = entry["source"], entry["stream_index"], entry["output"]
    found, info = [], []  # (field, message)

    fmt = ffprobe_json(ffprobe, "-show_format", "-show_streams", output)
    brand = fmt["format"].get("tags", {}).get("major_brand")
    if "mp4" not in fmt["format"].get("format_name", "") or brand != "M4A ":
        found.append(("format", f"output format {fmt['format'].get('format_name')!r}, brand {brand!r}"))
    if len(fmt["streams"]) != 1:
        found.append(("streams", f"output has {len(fmt['streams'])} streams"))
    src, src_format = stream(ffprobe, source, index)
    out = fmt["streams"][0] if fmt["streams"] else {}
    for key in STREAM_KEYS:
        if src.get(key) != out.get(key):
            found.append((f"stream:{key}", f"stream {key}: source {src.get(key)!r}, output {out.get(key)!r}"))

    src_packets, out_packets = packets(ffprobe, source, index), packets(ffprobe, output, 0)
    if not src_packets:
        found.append(("source", "source has no packets"))
    if out.get("nb_frames") != str(len(src_packets)):
        found.append(("nb_frames", f"output nb_frames {out.get('nb_frames')!r}, {len(src_packets)} source packets"))
    for keys, sink, label in ((PACKET_KEYS, found, "packets"), (PACKET_INFO, info, "packet durations")):
        a = [{k: p.get(k) for k in keys} for p in src_packets]
        b = [{k: p.get(k) for k in keys} for p in out_packets]
        if a != b:
            differing = [i for i, (x, y) in enumerate(zip(a, b)) if x != y]
            sink.append((label, f"{label} differ at {differing[:5]} ({len(a)} source, {len(b)} output packets)"))

    src_frames, out_frames = framemd5(ffmpeg, source, index), framemd5(ffmpeg, output, 0)
    if not src_frames or src_frames != out_frames:
        found.append(("framemd5", f"framemd5 differs ({len(src_frames)} source, {len(out_frames)} output frames)"))

    # A source with several media edits is checked a second way, from its own
    # boxes (docs/design/edit-list-writers.md, common decision 3): `ffprobe`
    # and `ffmpeg` apply their own model to such a list, which both files
    # here carry, so the checks above would agree with themselves.
    src_elst = full_elst(source, index)
    multi = bool(src_elst) and several_edits(src_elst[2])
    first_dts = None
    if multi:
        check_several(source, index, output, ffprobe, ffmpeg, found)
        first_dts = first_stored_dts(packets(ffprobe, source, index, raw=True))

    # What the black boxes do not read, from both files' own boxes
    # (scripts/writer_boxes.py): every sample, discarded priming included,
    # the sample entry, the timescales and the header durations.
    found.extend(writer_boxes.check_copy(source, output, [index], "extract"))
    writer_boxes.forget()

    mi_source = mi_output = None
    if mediainfo:
        general, audio = mediainfo_audio(mediainfo, output, None)
        _, src_audio = mediainfo_audio(mediainfo, source, index)
        if [g.get("Format") for g in general] != ["MPEG-4"] or len(audio) != 1:
            found.append(("mediainfo",
                          f"MediaInfo: general {[g.get('Format') for g in general]}, {len(audio)} audio tracks"))
        elif len(src_audio) != 1:
            found.append(("mediainfo", "MediaInfo: source audio track not found"))
        else:
            mi_source, mi_output = src_audio[0], audio[0]
            for key in MEDIAINFO_KEYS:
                if mi_output.get(key) != mi_source.get(key):
                    found.append((f"mediainfo:{key}",
                                  f"MediaInfo {key}: source {mi_source.get(key)!r}, output {mi_output.get(key)!r}"))

    problems, covered = [], []
    if found:
        source_headers = headers(source)
        traks = (source_headers or {}).get("traks") or []
        evidence = {
            "fields": {field for field, _ in found},
            "source_stream": src, "output_stream": out,
            "source_packets": src_packets, "output_packets": out_packets,
            "source_frames": src_frames, "output_frames": out_frames,
            "mi_source": mi_source, "mi_output": mi_output,
            "source_headers": source_headers, "output_headers": headers(output),
            "source_trak": traks[index] if index < len(traks) else None,
            "smpb": itunsmpb(src_format),
            "several": multi,
            "source_elst": src_elst,
            "output_elst": full_elst(output, 0),
            "first_dts": first_dts,
            "fragmented": has_fragments(source),
        }
        kds = known_divergences()
        for field, message in found:
            # No known divergence covers a box check's field
            # (docs/design/writer-oracle-boxes.md).
            kd = None if field.startswith("box:") else next(
                (kd_id for kd_id, predicate in kds if predicate(field, evidence)), None)
            if kd:
                covered.append({"kd": kd, "problem": message})
            else:
                problems.append(message)
    return problems, [message for _, message in info], covered


def strict_files(overlay_path):
    """Files on which the overlay checked KD-STRICT (CoreAV rejects them)."""
    if overlay_path is None:
        return set()
    overlay = json.loads(pathlib.Path(overlay_path).read_text(encoding="utf-8"))
    items = overlay["levels"]["streams"]["all"].get("covered_items")
    if items is None:
        raise SystemExit("harness error: overlay has no covered_items (old oracle_kd.py?)")
    return {item["file"] for item in items if "KD-STRICT" in item["kd"]}


def refuses_edit_list(movie_ts, media_ts, entries, fragmented):
    """Whether extraction refuses a track for the shape of its edit list,
    written out from the simple model of edit lists,
    docs/design/edit-lists.md decision 1 and the extraction rules.

    The simple model is one media edit at rate 1 from a media time at or
    above 0, after at most one empty edit. Such an edit with a duration of
    0 is outside the model in a movie without fragments and an edit to the
    end of the media in one with them, and extraction refuses it either
    way. Anything else is applied exactly when it is media edits at rate 1
    with durations, with empty edits between them, whose windows increase
    without overlap, in a movie without fragments; else it is unsupported
    and refused.

    The shape of the list only: extraction also refuses a movie timescale
    of 0, a non-zero first decode time without an edit list, and a media
    time before the first decode time, none of which the shape shows. Each
    of those makes a stream missing from the manifest rather than refused
    by design, which fails loudly."""
    if not entries:
        return False
    unit = 0x10000
    simple = None
    if len(entries) == 1 and entries[0][1] >= 0 and entries[0][2] == unit:
        simple = entries[0]
    elif (len(entries) == 2 and entries[0][1] == -1
            and entries[1][1] >= 0 and entries[1][2] == unit):
        simple = entries[1]
    if simple is not None:
        return not simple[0]
    return fragmented or not several_apply(movie_ts, media_ts, entries)


def outside_supported_model(path, index):
    """[`refuses_edit_list`] for the track at position `index` of `path`."""
    full = full_elst(path, index)
    if not full:
        return False
    movie_ts, media_ts, entries = full
    return refuses_edit_list(movie_ts, media_ts, entries, has_fragments(path))


def has_fragments(path):
    """Whether the movie box declares fragments (`mvex`, 14496-12 8.8.1)."""
    moov = _top_level_moov(path, MAX_ELST_BYTES)
    return moov is not None and any(kind == "mvex" for kind, _ in _children(moov))


def missing_streams(ffprobe, media, manifest, strict, refused=None):
    """AAC streams ffprobe finds in `media` that the manifest does not list;
    those extraction refuses by design go to `refused` instead."""
    listed = {(pathlib.Path(e["source"]).name, e["stream_index"]) for e in manifest}
    missing = []
    for path in sorted(pathlib.Path(media).iterdir()):
        if path.suffix not in IN_SCOPE or path.name in strict:
            continue
        try:
            streams = ffprobe_json(ffprobe, "-show_streams", str(path)).get("streams", [])
        except (RuntimeError, ValueError):
            continue
        for s in streams:
            if s.get("codec_name") == "aac" and (path.name, s.get("index")) not in listed:
                if refused is not None and isinstance(s.get("index"), int) \
                        and outside_supported_model(path, s["index"]):
                    refused.append(f"{path.name} stream {s.get('index')}")
                else:
                    missing.append(f"{path.name} stream {s.get('index')}")
    return missing


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--mediainfo")
    parser.add_argument("--media", help="fixture directory whose AAC streams must all be listed")
    parser.add_argument("--kd-overlay", help="known-divergences.json from scripts/oracle_kd.py")
    parser.add_argument("--report")
    args = parser.parse_args()

    with open(args.manifest, encoding="utf-8") as f:
        manifest = json.load(f)
    results = []
    for entry in manifest:
        try:
            problems, info, covered = check(entry, args.ffprobe, args.ffmpeg, args.mediainfo)
        except (RuntimeError, ValueError, KeyError) as error:
            problems, info, covered = [f"harness: {error}"], [], []
        results.append({**entry, "problems": problems, "informational": info, "covered": covered})
        name = f"{entry['source'].rsplit('/', 1)[-1]} stream {entry['stream_index']}"
        print(f"{'FAIL' if problems else 'kd  ' if covered else 'ok  '} {name}")
        for problem in problems:
            print(f"     {problem}")
        for item in covered:
            print(f"     ({item['kd']}) {item['problem']}")
        for difference in info:
            print(f"     (informational) {difference}")
    passed = sum(1 for r in results if not r["problems"])
    direct = sum(1 for r in results if not r["problems"] and not r["covered"])
    print(f"{passed}/{len(results)} extractions pass every oracle "
          f"({direct} directly, {passed - direct} through known divergences)")
    missing, refused = [], []
    if args.media:
        missing = missing_streams(args.ffprobe, args.media, manifest, strict_files(args.kd_overlay), refused)
        for name in missing:
            print(f"MISSING {name}: an AAC stream ffprobe reads was not extracted")
        for name in refused:
            print(f"refused {name}: edit list outside the simple model")
    if args.report:
        with open(args.report, "w", encoding="utf-8") as f:
            json.dump(
                {"passed": passed, "direct": direct, "total": len(results), "missing": missing,
                 "refused": refused, "results": results},
                f,
                indent=2,
            )
    return 0 if results and passed == len(results) and not missing else 1


if __name__ == "__main__":
    sys.exit(main())
