#!/usr/bin/env python3
"""Black-box oracles for joins (docs/design/join.md).

Reads a manifest of joins (from the `fixtures_round_trip` test of
coreav-isobmff-write's `join` tests, COREAV_JOIN_OUT, or from the corpus
harness) and checks every output with the pinned ffprobe and ffmpeg
binaries, and with MediaInfo when --mediainfo is given.

A join of the segments of one source (an entry with `source`) must give
back the source: `ffprobe` lists the same packets for each stream (data
hash, size, keyframe flag, times), `ffmpeg -copyts -f framemd5` decodes the
same video frames (those the source presents before the joined video's
end, where a clip to the end ends, as the split's check has them) and the
same audio frames, and the stream durations agree.

Any other join is checked against its inputs:

- `ffprobe` reads the output as an MP4 file (major brand `isom`) with the
  first input's video and audio codec, profile, dimensions, rotation,
  sample rate and channel layout;
- coverage: for each stream, the output's packets are the inputs' packets
  one input after another, one for one: the same data hash, size and
  keyframe flag, at the input's times plus its offset (exactly for the
  video, within one tick for the audio; packets `ffprobe` marks discarded
  are not presented and their times are not compared);
- `ffmpeg -copyts -f framemd5` of the output's video is the inputs'
  decoded frames, each input's at its times plus its offset;
- the output's presentation ends, read from its own edit lists: the video
  where the last input's video ends and no later than that input's file
  presents it, the audio no later than the last input's file presents its
  audio; each track header's duration its edit list's, the movie header's
  the longest;
- the output's decoded audio is, for each input, its packets decoded in
  sequence after the previous inputs' (as in the output's media) and
  presented as the input's own edit presents them, at its offset: sample
  for sample, within one sample;
- MediaInfo reports an MPEG-4 file with the first input's formats, and
  durations equal to the track header durations.

A difference counts as agreement only when an entry of
docs/known-divergences-join.md covers it. The binaries are black boxes; no implementation source is read. Exits 1 if any
check fails or the manifest is empty.
"""

import argparse
import collections
import fnmatch
import json
import os
import pathlib
import subprocess
import sys
import tempfile
from fractions import Fraction

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import clip_check as cc  # noqa: E402

KD_FILE = pathlib.Path(__file__).resolve().parent.parent / "docs" / "known-divergences-join.json"


def listed(packets):
    """`ffprobe`'s packets, each stored sample once, in the order of their
    first listing: ffmpeg 7.0.2 lists samples near an edit boundary again,
    some marked discarded (`D`), at the same byte position. A copy not
    marked discarded is kept where there is one."""
    best = {}
    for p in packets:
        pos = p.get("pos")
        if pos not in best or ("D" in (best[pos].get("flags") or "") and "D" not in (p.get("flags") or "")):
            best[pos] = p
    out, seen = [], set()
    for p in packets:
        pos = p.get("pos")
        if pos in seen:
            continue
        seen.add(pos)
        out.append(best[pos])
    return out


def stream_index(probe, kind):
    return next((i for i, s in enumerate(probe["streams"]) if s.get("codec_type") == kind), None)


def check_source(entry, ffprobe, ffmpeg, found, counts, evidence, decode=True):
    """A join of a source's segments against the source: the joined file's
    time 0 is the first segment's, at `source_offset` on the source's
    timeline."""
    source, output = entry["source"], entry["output"]
    offset = cc.fraction(entry.get("source_offset") or {"num": 0, "den": 1})
    src_probe = cc.ffprobe_json(ffprobe, "-show_format", "-show_streams", source)
    probe = cc.ffprobe_json(ffprobe, "-show_format", "-show_streams", output)
    # The split kept the source's first video and first audio stream.
    kinds = [("video", stream_index(src_probe, "video"))]
    if entry["inputs"][0].get("audio_stream") is not None:
        kinds.append(("audio", stream_index(src_probe, "audio")))
    evidence["source_headers"] = cc.headers(source)
    evidence["output_headers"] = cc.headers(output)
    evidence["source_video_index"] = kinds[0][1]
    for kind, src_index in kinds:
        out_index = stream_index(probe, kind)
        src = next((s for s in src_probe["streams"] if s.get("index") == src_index), None)
        if out_index is None or src is None:
            found.append((f"{kind}:stream", f"{kind}: no stream"))
            continue
        out = probe["streams"][out_index]
        for key in (cc.VIDEO_KEYS if kind == "video" else cc.AUDIO_KEYS) + ("time_base",):
            if src.get(key) != out.get(key):
                found.append((f"{kind}:{key}", f"{kind} {key}: source {src.get(key)!r}, joined {out.get(key)!r}"))
                if kind == "audio" and key == "profile":
                    profile_evidence(evidence, ffprobe, probe, src, source, src_index, output, out_index)
        base = Fraction(out["time_base"])
        shift = offset / base
        try:
            # Exact: `start_pts` in each stream's time base (`start_time` is
            # printed to six decimals, coarser than a tick of some
            # timescales).
            start = int(out["start_pts"]) * base + offset
            source_start = int(src["start_pts"]) * Fraction(src["time_base"])
            evidence[f"source_{kind}_start_gap"] = source_start - start
            if abs(start - source_start) > base:
                found.append((f"{kind}:start_time", f"{kind} start: source {src['start_time']}, joined "
                              f"{out['start_time']} plus the first segment's start"))
        except (KeyError, ValueError):
            pass
        # Data, size, keyframe flag and times shifted by the first
        # segment's start; durations as ffprobe reports them vary with the
        # file layout (none for a fragment's first sample), and the stored
        # ones are checked by the coreav-isobmff round trip.
        a = stored_packets(ffprobe, source, src_index)
        b = stored_packets(ffprobe, output, out_index)
        tolerance = 0 if kind == "video" and shift.denominator == 1 else 1
        same = 0
        first = None
        # For KD-J-PTS-SHIFT: the time differences of the packets equal in
        # data, and whether every packet is.
        deltas = {"pts": set(), "dts": set()}
        data_same = len(a) == len(b)
        for i, (x, y) in enumerate(zip(a, b)):
            ok = (x.get("data_hash"), x.get("size"), (x.get("flags") or "")[:1]) == \
                (y.get("data_hash"), y.get("size"), (y.get("flags") or "")[:1])
            data_same &= ok
            if x.get("unpresented") or y.get("unpresented"):
                # Listed only with edits ignored in one of the files (a last
                # sample that an edit does not present): its data and place
                # count, its times are those of an unpresented sample.
                if ok:
                    same += 1
                elif first is None:
                    first = i
                continue
            for key in ("pts", "dts"):
                if not isinstance(x.get(key), int) or not isinstance(y.get(key), int):
                    ok = False
                    data_same = False
                    continue
                delta = Fraction(x[key]) - (y[key] + shift)
                deltas[key].add(delta)
                if abs(delta) > tolerance:
                    ok = False
            if ok:
                same += 1
            elif first is None:
                first = i
        counts["agree"] += same
        counts["disagree"] += max(len(a), len(b)) - same
        evidence[f"source_{kind}_packets"] = (data_same, deltas, base, src.get("time_base") == out.get("time_base"))
        evidence[f"source_{kind}_stored"] = (a, b, shift, output, out_index)
        evidence["ffprobe"] = ffprobe
        evidence[f"source_{kind}_mismatched"] = max(len(a), len(b)) - same
        if same != len(a) or len(a) != len(b):
            found.append((f"{kind}:packets", f"{kind}: packets differ from the source's at "
                          f"{first if first is not None else min(len(a), len(b))} ({len(a)} source, {len(b)} joined)"))
        if not decode:
            continue
        sb, sf = cc.framemd5(ffmpeg, source, src_index, video=kind == "video")
        ob, of = cc.framemd5(ffmpeg, output, out_index, video=kind == "video")
        if sb is None or ob is None:
            found.append((f"{kind}:framemd5", f"{kind}: decoded frames without a time base"))
            continue
        expected = [(t * sb, size, h) for t, size, h, _ in sf]
        at_end = []
        if kind == "video":
            # The frames the source presents before the joined video's end
            # (the last segment's, where a clip to the end ends), as the split's check has them; a frame presented
            # exactly at the end has no presented duration, and ffmpeg may
            # still show it.
            end = cc.fraction(entry["inputs"][-1]["video_end"]) + offset
            at_end = [f for f in expected if f[0] == end]
            expected = [f for f in expected if f[0] < end]
        actual = [(round((t * ob + offset) / sb) * sb, size, h) for t, size, h, _ in of]
        if kind == "video":
            evidence["source_video_frames"] = (sb, sf, ob, of, offset, cc.fraction(entry["inputs"][-1]["video_end"]))
        if actual not in (expected, expected + at_end):
            found.append((f"{kind}:framemd5", f"{kind}: decoded frames differ from the source's"))


def stored_packets(ffprobe, path, index):
    """A file's packets for comparing a join of segments with its source:
    every stored sample, in order (edit lists ignored), with the times
    `ffprobe` gives it under the file's edits where it lists it there, and
    none where it does not (`ffprobe` leaves out a sample of decode duration
    0, or one at the media's end, that the edit does not present), as
    `scripts/split_check.py` compares segments."""
    timed = {p.get("pos"): p for p in listed(cc.packets(ffprobe, path, index))}
    raw = cc.ffprobe_json(ffprobe, "-ignore_editlist", "1", "-select_streams", str(index), "-show_packets",
                          "-show_data_hash", "SHA256", "-show_entries", "packet=" + ",".join(cc.PACKET_KEYS),
                          path).get("packets", [])
    return [timed.get(p.get("pos")) or {**p, "pts": None, "dts": None, "unpresented": True} for p in raw]


def adts_concat_decoded(ffmpeg, ffprobe, inputs, index_key, output, out_index):
    """The inputs' audio packets, extracted by ffmpeg as ADTS frames (edit
    lists ignored), concatenated and decoded to float32 PCM: (path, frame
    starts), each packet's decode time in the output paired with where its
    decoded frame lies in the PCM (edit lists ignored; the decode times are
    checked against the inputs' by the packet comparison). None when that
    fails or the packet counts differ."""
    data = b""
    for item in inputs:
        try:
            adts = cc.run([ffmpeg, "-v", "error", "-ignore_editlist", "1", "-i", item["path"], "-map",
                           f"0:{item[index_key]}", "-c", "copy", "-f", "adts", "-"])
        except RuntimeError:
            return None
        frames = cc._adts_frames(adts)
        if frames is None:
            return None
        data += b"".join(frames)
    times = cc.ffprobe_json(ffprobe, "-ignore_editlist", "1", "-select_streams", str(out_index),
                            "-show_entries", "packet=dts", "-show_packets", output)
    starts = [int(p["dts"]) for p in times.get("packets", [])]
    if len(starts) != len(cc._adts_frames(data) or []):
        return None
    main = _decode_adts(ffmpeg, cc._adts_frames(data) or [])
    if main is None:
        return None
    decoded, positions = main
    return decoded, [(dts, positions.get(n)) for n, dts in enumerate(starts)]


def delivered_decoded(ffmpeg, ffprobe, inputs, index_key, output, out_index):
    """The inputs' audio packets decoded in the order ffmpeg's demuxer
    delivers the output's packets under its edits (KD-J-AUDIO-NOISE): before
    an edit after the first it delivers packets from before the edit's
    media time again, flagged discard, which are decoded and dropped. The
    same (path, frame starts) as `adts_concat_decoded`, the starts placing
    each packet's delivery that is not discarded. None when that fails."""
    frames = []
    for item in inputs:
        try:
            adts = cc.run([ffmpeg, "-v", "error", "-ignore_editlist", "1", "-i", item["path"], "-map",
                           f"0:{item[index_key]}", "-c", "copy", "-f", "adts", "-"])
        except RuntimeError:
            return None
        part = cc._adts_frames(adts)
        if part is None:
            return None
        frames += part
    listing = [cc.ffprobe_json(ffprobe, *extra, "-select_streams", str(out_index), "-show_entries",
                               "packet=pos,dts,flags", "-show_packets", output).get("packets", [])
               for extra in (("-ignore_editlist", "1"), ())]
    stored, delivered = listing
    if len(stored) != len(frames):
        return None
    number = {p.get("pos"): n for n, p in enumerate(stored)}
    if any(p.get("pos") not in number for p in delivered):
        return None
    order = [number[p.get("pos")] for p in delivered]
    shown = [None if "D" in (p.get("flags") or "") else n for p, n in zip(delivered, order)]
    main = _decode_adts(ffmpeg, [frames[n] for n in order], shown)
    if main is None:
        return None
    decoded, positions = main
    # Each packet is placed at its first delivery not flagged discard; one
    # delivered only flagged discard (an edit's pre-roll) at its last
    # delivery, so that an edit starting in it is found.
    hidden = _decode_positions(ffmpeg, [frames[n] for n in order], order, last=True)
    if hidden is None:
        pathlib.Path(decoded).unlink(missing_ok=True)
        return None
    for n, position in hidden.items():
        positions.setdefault(n, position)
    return decoded, [(int(p["dts"]), positions.get(n)) for n, p in enumerate(stored)]


def _decode_adts(ffmpeg, frames, numbers=None):
    """ADTS frames decoded to float32 PCM: (path, {packet: position in the
    PCM}), the frames numbered by `numbers` (by default in order; a None
    number is decoded but not placed). None when decoding fails."""
    fd, decoded = tempfile.mkstemp(suffix=".f32", dir=cc._work())
    os.close(fd)
    result = subprocess.run([ffmpeg, "-v", "error", "-y", "-f", "aac", "-i", "-", "-c:a", "pcm_f32le", "-f",
                             "f32le", decoded], input=b"".join(frames), capture_output=True, check=False)
    positions = _decode_positions(ffmpeg, frames, numbers)
    if result.returncode != 0 or positions is None:
        pathlib.Path(decoded).unlink(missing_ok=True)
        return None
    return decoded, positions


def _decode_positions(ffmpeg, frames, numbers=None, last=False):
    """Where each frame's decoded samples lie in the PCM of `frames`
    decoded: {number: position}, the first frame with a number kept (with
    `last`, the last). The decoder does not output a frame for every
    packet (ffmpeg gives none for the first packet of some streams), so the
    frames' own times say which packet each one decodes. None when that
    fails."""
    listing = subprocess.run([ffmpeg, "-v", "error", "-f", "aac", "-i", "-", "-f", "framemd5", "-"],
                             input=b"".join(frames), capture_output=True, check=False)
    if listing.returncode != 0:
        return None
    positions, at = {}, 0
    for line in listing.stdout.decode(errors="replace").splitlines():
        if line.startswith("#"):
            continue
        fields = [f.strip() for f in line.split(",")]
        try:
            pts, duration = int(fields[2]), int(fields[3])
        except (IndexError, ValueError):
            continue
        if duration > 0:
            k = round(pts / duration)
            number = k if numbers is None else numbers[k] if 0 <= k < len(numbers) else None
            if number is not None and (last or number not in positions):
                positions[number] = at
            at += duration
    return positions


def reference_slot(starts, media, rate, ts):
    """The packet that holds media time `media` (output media ticks) and the
    offset into its decoded frame, in samples; None before the first."""
    for n in range(len(starts) - 1, -1, -1):
        if starts[n][0] <= media:
            return n, (media - starts[n][0]) * rate / ts
    return None


def reference_indices(starts, media, rate, ts):
    """The indices in the reference PCM of media time `media`: one for the
    packet that holds it, or one for each packet at that packet's decode
    time where several share it (a first sample of decode duration 0, say,
    whose frame ffmpeg presents at the same time as the next one's)."""
    slot = reference_slot(starts, media, rate, ts)
    if slot is None:
        return []
    dts = starts[slot[0]][0]
    return [position + slot[1] for d, position in starts if d == dts and position is not None]


def reference_index(starts, media, rate, ts):
    """Index in the reference PCM of media time `media` (output media
    ticks): within the decoded frame of the packet that holds it. `starts`
    holds each packet's decode time and its frame's position in the PCM
    (None when the decoder gave no frame for it). None when no decoded
    frame holds it."""
    slot = reference_slot(starts, media, rate, ts)
    if slot is None or starts[slot[0]][1] is None:
        return None
    return starts[slot[0]][1] + slot[1]


def stored_dts(ffprobe, path, index, headers):
    """The stream's decode times as stored (edit lists ignored), in seconds:
    `ffprobe`'s, with what it moves them back by (the track's smallest
    composition offset read as signed, when negative; KD-J-DTS-SHIFT) added
    back from the file's own `ctts`."""
    probe = cc.ffprobe_json(ffprobe, "-ignore_editlist", "1", "-select_streams", str(index),
                            "-show_entries", "stream=time_base:packet=dts", "-show_streams", "-show_packets", path)
    base = Fraction(probe["streams"][0]["time_base"])
    trak = headers["traks"][index] if headers and len(headers["traks"]) > index else {}
    scale = trak.get("mdhd", (0, 0))[0]
    moved = Fraction(max(0, -(trak.get("ctts_min") or 0)), scale) if scale else 0
    return [int(p["dts"]) * base + moved for p in probe.get("packets", []) if isinstance(p.get("dts"), int)]


def media_edits(headers, index):
    """A track's media edits as (presentation start, media time), both in
    seconds, and the empty time before its first media edit; None without
    an edit list or timescales."""
    trak = headers["traks"][index] if headers and len(headers["traks"]) > index else {}
    movie = (headers or {}).get("mvhd") or (0, 0)
    media_scale = trak.get("mdhd", (0, 0))[0]
    if not trak.get("elst") or not movie[0] or not media_scale:
        return None
    edits, at = [], Fraction(0)
    for duration, media in trak["elst"]:
        if media != -1:
            edits.append((at, Fraction(media, media_scale)))
        at += Fraction(duration, movie[0])
    return edits


def edit_evidence(ffprobe, inputs, key, output, out_index):
    """KD-J-VIDEO-LATER-EDIT's and KD-J-AUDIO-EDIT-START's evidence, read
    apart from the writer: the output's media edits, and for each input the
    edit the output should hold for it (its own first media edit, started at
    its offset, its media time moved to where the input's first sample is
    stored in the output) and, for audio, the distance from that media time
    to the input's next packet start (None when the media time is one)."""
    out_headers = cc.headers(output)
    out_dts = stored_dts(ffprobe, output, out_index, out_headers)
    expected, at = [], 0
    for item in inputs:
        headers = cc.headers(item["path"])
        own = media_edits(headers, item[key])
        dts = stored_dts(ffprobe, item["path"], item[key], headers)
        if not own or not dts or at >= len(out_dts):
            expected.append(None)
            at += len(dts)
            continue
        start, media = own[0]
        after = [t for t in dts if t >= media]
        expected.append({"start": cc.fraction(item["offset"]) + start, "media": media - dts[0] + out_dts[at],
                         "gap": after[0] - media if after and after[0] != media else None})
        at += len(dts)
    return {"output": media_edits(out_headers, out_index) or [], "inputs": expected}


def own_end(ffprobe, path, index):
    """Where a file presents a track up to, in seconds: the end of its edit
    list, or without one where its stored samples end (ISO/IEC 14496-12
    8.6.6); None when unreadable."""
    edits = cc.full_edit_list(path, index)
    if edits is None or not edits[0]:
        return None
    movie, _, entries = edits
    if entries:
        return sum(Fraction(d, movie) for d, _, _ in entries)
    probe = cc.ffprobe_json(ffprobe, "-ignore_editlist", "1", "-select_streams", str(index),
                            "-show_entries", "stream=time_base:packet=pts,duration", "-show_streams",
                            "-show_packets", path)
    base = Fraction(probe["streams"][0]["time_base"])
    return max(((int(p["pts"]) + int(p.get("duration") or 0)) * base for p in probe.get("packets", [])
                if isinstance(p.get("pts"), int)), default=None)


def check_ends(ffprobe, inputs, kinds, output, oh, found):
    """The output's presentation ends, read from its own boxes against the
    inputs' files (ISO/IEC 14496-12 8.6.6): the video's where the last
    input's video ends (the span the placement check bounds) and no later
    than that input's file presents it; the audio's no later than the last
    input's file presents its audio, from its offset (earlier where its
    gapless information trims the padding; an audio edit cut short leaves
    packets out, which the coverage shows). Each track header's duration
    is its edit list's, and the movie header's the longest."""
    last = inputs[-1]
    tkhds = []
    for out_index, (kind, key) in enumerate(kinds):
        end = own_end(ffprobe, output, out_index)
        if end is None:
            found.append((f"{kind}:end", f"{kind}: the output's presentation end cannot be read"))
            continue
        trak = oh["traks"][out_index] if oh and len(oh["traks"]) > out_index else {}
        movie = oh["mvhd"][0] if oh and oh.get("mvhd") else 0
        tkhd = Fraction(trak.get("tkhd", -1), movie) if movie else None
        tkhds.append(tkhd)
        if tkhd != end and trak.get("elst"):
            found.append((f"{kind}:end", f"{kind}: track header duration {float(tkhd or 0):.6f} s, edits "
                          f"{float(end):.6f} s"))
        own = own_end(ffprobe, last["path"], last[key]) if last.get(key) is not None else None
        offset = cc.fraction(last["offset"])
        if kind == "video" and end != cc.fraction(last["video_end"]):
            found.append(("video:end", f"video: the output's edits end at {float(end):.6f} s, the last input's "
                          f"video at {float(cc.fraction(last['video_end'])):.6f} s"))
        if own is not None and end > offset + own:
            found.append((f"{kind}:end", f"{kind}: the output's edits end at {float(end):.6f} s, after the last "
                          f"input presents it ({float(offset + own):.6f} s)"))
    if tkhds and None not in tkhds and Fraction(oh["mvhd"][1], oh["mvhd"][0]) != max(tkhds):
        found.append(("end", f"movie header duration {oh['mvhd'][1] / oh['mvhd'][0]:.6f} s, longest track "
                      f"{float(max(tkhds)):.6f} s"))


def compare_packets(kind, in_lists, offsets, out_packets, base, counts, found, evidence):
    """Coverage of one stream: the output's packets (`listed`) against each
    input's, one input after another, each input's times plus its offset
    (seconds) in the output's time base. Adds to `counts` and `found`, and
    records the evidence the packet known divergences read."""
    at = 0
    mismatched, timing = [], {}
    # Per input, every compared packet's pts and dts offsets, exact ones
    # included: a known divergence that moves an input's times moves
    # all of them by one constant.
    deltas = {}
    # The inputs' packet durations, for the stream's frame duration.
    durations = []
    missing = 0
    # Packets that differ in more than their times.
    other = 0
    for n, (in_packets, offset) in enumerate(zip(in_lists, offsets)):
        shift = offset / base
        tolerance = 0 if kind == "video" and shift.denominator == 1 else 1
        for k, s in enumerate(in_packets):
            if isinstance(s.get("duration"), int):
                durations.append(s["duration"])
            o = out_packets[at] if at < len(out_packets) else None
            at += 1
            if o is None:
                counts["disagree"] += 1
                mismatched.append((n, k))
                missing += 1
                continue
            # Durations as ffprobe reports them depend on the edits
            # (the last packet of an edit is cut to it); the stored
            # durations are checked by the coreav-isobmff round trip.
            data = (o.get("data_hash") == s.get("data_hash") and o.get("size") == s.get("size")
                    and o.get("flags", "")[:1] == s.get("flags", "")[:1])
            same = data
            # Times of packets ffprobe marks as discarded are not
            # presented times; they are not compared.
            discarded = "D" in (o.get("flags") or "") or "D" in (s.get("flags") or "")
            if not same:
                other += 1
            if same and not discarded:
                for name in ("pts", "dts"):
                    a, b = s.get(name), o.get(name)
                    if not isinstance(a, int) or not isinstance(b, int):
                        same = False
                        other += 1
                        continue
                    deltas.setdefault(n, {"pts": set(), "dts": set()})[name].add(Fraction(a) + shift - b)
                    if abs(Fraction(a) + shift - b) > tolerance:
                        timing.setdefault(n, set()).add(Fraction(a) + shift - b)
                        same = False
            if same:
                counts["agree"] += 1
            else:
                counts["disagree"] += 1
                mismatched.append((n, k))
    extra = len(out_packets) - at
    if extra > 0:
        counts["disagree"] += extra
        found.append((f"{kind}:extra", f"{kind}: {extra} output packets of no input"))
    if mismatched:
        found.append((f"{kind}:packets", f"{kind}: {len(mismatched)} packets differ, first (input, packet) "
                      f"{mismatched[:3]}"))
    # The stream's frame duration: the inputs' most common packet
    # duration, in their time base (2,048 ticks a packet where SBR
    # doubles the rate).
    evidence[f"{kind}_frame"] = max(set(durations), key=durations.count) if durations else None
    evidence[f"{kind}_timing"] = timing
    evidence[f"{kind}_deltas"] = deltas
    evidence[f"{kind}_missing"] = missing + max(0, extra)
    evidence[f"{kind}_base"] = base
    evidence[f"{kind}_mismatched"] = len(mismatched)
    evidence[f"{kind}_only_timing"] = other == 0


def check_inputs(entry, ffprobe, ffmpeg, mediainfo, found, counts, evidence, decode=True):
    output, inputs = entry["output"], entry["inputs"]
    probe = cc.ffprobe_json(ffprobe, "-show_format", "-show_streams", output)
    oh = cc.headers(output)
    evidence["output_headers"] = oh
    brand = probe["format"].get("tags", {}).get("major_brand")
    if "mp4" not in probe["format"].get("format_name", "") or brand != "isom":
        found.append(("format", f"output format {probe['format'].get('format_name')!r}, brand {brand!r}"))
    first_probe = cc.ffprobe_json(ffprobe, "-show_streams", inputs[0]["path"])
    kinds = [("video", "video_stream")]
    if inputs[0].get("audio_stream") is not None:
        kinds.append(("audio", "audio_stream"))
    if len(probe["streams"]) != len(kinds):
        found.append(("streams", f"output has {len(probe['streams'])} streams, expected {len(kinds)}"))
        return
    check_ends(ffprobe, inputs, kinds, output, oh, found)
    for out_index, (kind, key) in enumerate(kinds):
        out = probe["streams"][out_index]
        src = next((s for s in first_probe["streams"] if s.get("index") == inputs[0][key]), {})
        for name in cc.VIDEO_KEYS if kind == "video" else cc.AUDIO_KEYS:
            if src.get(name) != out.get(name):
                found.append((f"{kind}:{name}", f"{kind} {name}: first input {src.get(name)!r}, "
                              f"output {out.get(name)!r}"))
                if kind == "audio" and name == "profile":
                    profile_evidence(evidence, ffprobe, probe, src, inputs[0]["path"], inputs[0][key], output,
                                     out_index)
        if kind == "video" and cc.rotation(src) != cc.rotation(out):
            found.append(("video:rotation", "video rotation differs from the first input's"))
        base = Fraction(out["time_base"])
        # Coverage.
        out_packets = listed(cc.packets(ffprobe, output, out_index))
        in_lists = [listed(cc.packets(ffprobe, item["path"], item[key])) for item in inputs]
        compare_packets(kind, in_lists, [cc.fraction(item["offset"]) for item in inputs], out_packets, base,
                        counts, found, evidence)
        evidence[f"{kind}_edits"] = edit_evidence(ffprobe, inputs, key, output, out_index)
        if kind == "video":
            # The smallest composition offset read as signed, and the media
            # timescale, of the output's track and of each input's.
            def smallest(h, index):
                trak = h["traks"][index] if h and len(h["traks"]) > index else {}
                return trak.get("ctts_min"), trak.get("mdhd", (None, 0))[0]
            evidence["video_ctts_min"] = (smallest(oh, out_index),
                                          [smallest(cc.headers(item["path"]), item[key]) for item in inputs])
        if kind == "video":
            # Each input's composition delay: its edit's media time minus its
            # first decode time, in seconds (KD-J-VIDEO-LATER-EDIT).
            delays = []
            stored_ends = []
            for item in inputs:
                h = cc.headers(item["path"])
                trak = h["traks"][item[key]] if h and len(h["traks"]) > item[key] else {}
                media = [t for _, t in trak.get("elst") or [] if t != -1]
                raw = cc.ffprobe_json(ffprobe, "-ignore_editlist", "1", "-select_streams", str(item[key]),
                                      "-show_entries", "packet=dts", "-show_packets", "-read_intervals", "%+#1",
                                      item["path"])
                first_dts = int((raw.get("packets") or [{"dts": 0}])[0].get("dts", 0))
                timescale = trak.get("mdhd", (1, 0))[0]
                delays.append(Fraction(max(0, (media[0] if media else 0) - first_dts), timescale))
                # Where its stored frames end on the output's timeline: the
                # latest composition end, from its edit's media time on
                # (KD-J-VIDEO-EDIT-END).
                stored = cc.ffprobe_json(ffprobe, "-ignore_editlist", "1", "-select_streams", str(item[key]),
                                         "-show_entries", "packet=pts,duration", "-show_packets",
                                         item["path"]).get("packets", [])
                last = max((int(p["pts"]) + int(p.get("duration") or 0) for p in stored
                            if isinstance(p.get("pts"), int)), default=None)
                stored_ends.append(None if last is None or not timescale else
                                   Fraction(last - (media[0] if media else 0), timescale)
                                   + cc.fraction(item["offset"]))
            evidence["video_delays"] = delays
            evidence["video_stored_ends"] = stored_ends
        # Decoded content.
        if not decode:
            pass
        elif kind == "video":
            expected = []
            durations = []
            ends = []
            for item in inputs:
                ib, frames = cc.framemd5(ffmpeg, item["path"], item[key], video=True)
                offset = cc.fraction(item["offset"])
                expected.extend((pts * ib + offset, size, h) for pts, size, h, _ in frames)
                durations.extend(None if d is None else d * ib for _, _, _, d in frames)
                ends.append(len(expected))
            evidence["video_input_ends"] = ends
            evidence["video_frame_durations"] = durations
            # The inputs follow one another without gaps, checked against
            # the inputs' own files: each input starts where the previous
            # one's video ends, its decoded frames lie in its span, and the
            # span is no longer than the input presents on its own (its
            # edits, else its media), so a gap the writer left would show.
            begin = 0
            for n, item in enumerate(inputs):
                start, end = cc.fraction(item["video_start"]), cc.fraction(item["video_end"])
                if n > 0 and start != cc.fraction(inputs[n - 1]["video_end"]):
                    found.append(("video:placement", f"video: input {n} starts at {float(start):.6f} s, "
                                  f"not where input {n - 1} ends"))
                frames = expected[begin:ends[n]]
                begin = ends[n]
                if frames and not (start <= frames[0][0] and frames[-1][0] < end):
                    found.append(("video:placement", f"video: input {n}'s frames lie outside its span"))
                h = cc.headers(item["path"])
                trak = h["traks"][item[key]] if h and len(h["traks"]) > item[key] else {}
                _, own = cc.edit_span(trak, h["mvhd"][0]) if h else (0, None)
                # An input is held until the next one's video starts where
                # that one's audio starts first: by that lead at most.
                lead = Fraction(0)
                if n + 1 < len(inputs):
                    lead = cc.fraction(inputs[n + 1]["video_start"]) - cc.fraction(inputs[n + 1]["offset"])
                if own is not None:
                    # Its edits' duration.
                    if end - start > own + lead:
                        found.append(("video:placement", f"video: input {n} spans {float(end - start):.6f} "
                                      f"s, more than its edits' {float(own):.6f} s"))
                elif n < len(stored_ends) and stored_ends[n] is not None and end > stored_ends[n] + lead:
                    # Without edits, where its stored frames end (the movie
                    # header leaves fragments out).
                    found.append(("video:placement", f"video: input {n} ends at {float(end):.6f} s, after "
                                  f"its frames"))
            ob, frames = cc.framemd5(ffmpeg, output, out_index, video=True)
            actual = [(pts * ob, size, h) for pts, size, h, _ in frames]
            evidence["video_frames"] = (expected, actual)
            if ob is None or expected != actual:
                first = next((i for i, (a, b) in enumerate(zip(expected, actual)) if a != b),
                             min(len(expected), len(actual)))
                found.append(("video:framemd5", f"video framemd5 differs at frame {first} ({len(expected)} "
                              f"input frames, {len(actual)} output frames)"))
        else:
            check_audio(entry, ffprobe, ffmpeg, out_index, out, oh, found, evidence)
    check_mediainfo(mediainfo, inputs, kinds, output, oh, found)


def check_mediainfo(mediainfo, inputs, kinds, output, oh, found):
    """MediaInfo reads an MPEG-4 file with the first input's formats, and
    durations equal to the track header durations."""
    if mediainfo:
        tracks = cc.mediainfo_tracks(mediainfo, output)
        first_tracks = cc.mediainfo_tracks(mediainfo, inputs[0]["path"])
        general = [t for t in tracks if t.get("@type") == "General"]
        if [g.get("Format") for g in general] != ["MPEG-4"]:
            found.append(("mediainfo", f"MediaInfo: general {[g.get('Format') for g in general]}"))
        for out_index, (kind, key) in enumerate(kinds):
            mi_type = "Video" if kind == "video" else "Audio"
            mi_out = [t for t in tracks if t.get("@type") == mi_type]
            mi_src = [t for t in first_tracks if t.get("@type") == mi_type
                      and t.get("StreamOrder") == str(inputs[0][key])]
            if len(mi_out) != 1 or len(mi_src) != 1:
                found.append((f"mediainfo:{kind}", f"MediaInfo: {len(mi_out)} output and {len(mi_src)} "
                              f"first input {kind} tracks"))
                continue
            for name in cc.MEDIAINFO_VIDEO if kind == "video" else cc.MEDIAINFO_AUDIO:
                if mi_out[0].get(name) != mi_src[0].get(name):
                    found.append((f"mediainfo:{kind}:{name}", f"MediaInfo {kind} {name}: first input "
                                  f"{mi_src[0].get(name)!r}, output {mi_out[0].get(name)!r}"))
            trak = oh["traks"][out_index]
            try:
                duration = float(mi_out[0]["Duration"])
                if abs(duration - trak["tkhd"] / oh["mvhd"][0]) > 0.0015:
                    found.append((f"mediainfo:{kind}:Duration", f"MediaInfo {kind} duration {duration}, "
                                  f"track header {trak['tkhd'] / oh['mvhd'][0]:.6f}"))
            except (KeyError, TypeError, ValueError):
                pass


def audio_pieces(entry, oh, out_index):
    """The output's audio edits as pieces: (output start, length, media
    start), in seconds and media ticks."""
    tm = oh["mvhd"][0]
    trak = oh["traks"][out_index]
    ts = trak["mdhd"][0]
    pieces, at = [], Fraction(0)
    for duration, media in trak.get("elst") or [[None, 0]]:
        if duration is None:
            pieces.append((Fraction(0), Fraction(trak["mdhd"][1], ts), Fraction(0)))
            break
        length = Fraction(duration, tm)
        if media >= 0:
            pieces.append((at, length, Fraction(media)))
        at += length
    return pieces, ts


def check_audio(entry, ffprobe, ffmpeg, out_index, out, oh, found, evidence):
    """The output's decoded audio against the inputs' packets decoded in
    sequence, presented by the output's edits."""
    rate = int(out.get("sample_rate") or 0)
    channels = int(out.get("channels") or 1)
    if not rate:
        return
    reference = adts_concat_decoded(ffmpeg, ffprobe, entry["inputs"], "audio_stream", entry["output"],
                                    out_index)
    decoded = cc.pcm(ffmpeg, entry["output"], out_index, channels)
    try:
        if reference is None or decoded is None:
            found.append(("audio:pcm", "audio did not decode"))
            return
        ref_path, starts = reference
        spans, _, out_path = decoded
        pieces, ts = audio_pieces(entry, oh, out_index)
        if len(oh["traks"][out_index].get("elst") or []) >= 8:
            found.append(("audio:pcm", "audio: more edits than the header reader keeps"))
            return
        frame = 4 * channels
        evidence["audio_pieces"] = (pieces, ts, rate, spans, (ref_path, starts), out_path, frame)
        evidence["audio_delivered"] = lambda: delivered_decoded(ffmpeg, ffprobe, entry["inputs"],
                                                                "audio_stream", entry["output"], out_index)
        problems = compare_pieces(pieces, ts, rate, spans, (ref_path, starts), out_path, frame, rounded=False)
        if problems:
            evidence["audio_problems"] = problems
            found.append(("audio:pcm", f"audio: {problems[0]}"))
    finally:
        # Kept for the known divergences while their evidence holds them.
        if not evidence.get("audio_pieces"):
            if decoded:
                pathlib.Path(decoded[2]).unlink(missing_ok=True)
            if reference:
                pathlib.Path(reference[0]).unlink(missing_ok=True)


def compare_pieces(pieces, ts, rate, spans, ref, out_path, frame, rounded):
    """Per audio edit, every decoded output frame presented in it equals the
    reference at the media time the edit gives its start: the decoded frame
    of the packet that holds that media time, from that point (with
    `rounded`, every edit after the first read from the first frame
    boundary at or after its media time, as `KD-J-AUDIO-EDIT-START`
    describes). An edit's last frame may run past it; only its samples
    inside the edit are compared. Returns the problems."""
    ref_path, starts = ref
    problems = []
    # The output's frames with their positions in its PCM.
    frames, at = [], 0
    for t, samples in spans:
        frames.append((t, samples, at))
        at += samples
    for n, (start, length, media) in enumerate(pieces):
        media = Fraction(media)
        if rounded and n > 0:
            # The next packet's start: a frame boundary whatever the
            # decoded frame's length (2,048 samples where SBR doubles the
            # rate).
            slot = reference_slot(starts, media, rate, ts)
            if slot is not None and slot[1] and slot[0] + 1 < len(starts):
                media = Fraction(starts[slot[0] + 1][0])
        end = start + length
        bad = 0
        compared = 0
        for t, samples, position in frames:
            if t < start - Fraction(1, rate) or t >= end:
                continue
            # Only the samples inside the edit.
            count = min(samples, int((end - t) * rate))
            if count <= 0:
                continue
            frame_media = media + (t - start) * ts
            indices = reference_indices(starts, frame_media, rate, ts)
            if not indices:
                bad += 1
                continue
            compared += 1
            ok = any(_same(ref_path, (int(round(index)) + d) * frame, out_path, position * frame, count * frame)
                     for index in indices for d in (0, -1, 1) if int(round(index)) + d >= 0)
            if not ok:
                bad += 1
        if bad or not compared:
            problems.append(f"edit {n}: {bad} of {compared} decoded frames from {float(start):.6f} s differ "
                            f"from the inputs' packets decoded in sequence")
    return problems


def _same(path_a, offset_a, path_b, offset_b, length):
    if offset_a < 0 or offset_a + length > os.path.getsize(path_a) or offset_b + length > os.path.getsize(path_b):
        return False
    with open(path_a, "rb") as a, open(path_b, "rb") as b:
        a.seek(offset_a)
        b.seek(offset_b)
        left = length
        while left > 0:
            n = min(cc.CHUNK, left)
            if a.read(n) != b.read(n):
                return False
            left -= n
    return True


# Several edits (docs/design/edit-list-writers.md, common decision 3): when
# an input, or the source of joined segments, has a list of several edits,
# ffmpeg's edit model differs from ISO/IEC 14496-12's, so samples and frames
# are read with edit lists ignored and the edit arithmetic is the harness's
# own (`clip_check.iso_windows`, `clip_check.iso_presented`), from each
# file's own boxes. ffmpeg's edit-applied video is reported beside it, not
# gating. Joins of inputs with one edit each keep the checks above, whose
# known divergences describe how ffmpeg plays the output's edits.

# The window of a track without an edit list: presentation is media time.
NO_EDITS = [(Fraction(-(1 << 62)), Fraction(1 << 62), Fraction(-(1 << 62)))]


def has_several(entry, ffprobe):
    """Whether an input, or the source of joined segments, has a kept track
    with several edits."""
    tracks = [(item["path"], item[key]) for item in entry["inputs"] for key in ("video_stream", "audio_stream")
              if item.get(key) is not None]
    if entry.get("source"):
        probe = cc.ffprobe_json(ffprobe, "-show_streams", entry["source"])
        tracks += [(entry["source"], i) for i in (stream_index(probe, "video"), stream_index(probe, "audio"))
                   if i is not None]
    for path, index in tracks:
        edits = cc.full_edit_list(path, index)
        if edits and cc.several_edits(edits[2]):
            return True
    return False


def iso_track(ffprobe, path, index):
    """(media timescale, windows) of a track from its own boxes: the windows
    of its edit list; without one, the samples an `iTunSMPB` item keeps
    after its encoder delay (which the writers read as an edit), else
    `NO_EDITS`. None when unreadable."""
    edits = cc.full_edit_list(path, index)
    if edits is None:
        return None
    if edits[2]:
        return edits[1], cc.iso_windows(*edits)
    smpb = (cc.ffprobe_json(ffprobe, "-show_format", path)["format"].get("tags") or {}).get("iTunSMPB", "").split()
    stream = cc.ffprobe_json(ffprobe, "-select_streams", str(index), "-show_streams", path)["streams"]
    if len(smpb) >= 4 and stream and stream[0].get("codec_type") == "audio":
        delay, samples = int(smpb[1], 16), int(smpb[3], 16)
        return edits[1], [(Fraction(delay), Fraction(delay + samples), Fraction(0))]
    return edits[1], NO_EDITS


def stored(ffprobe, path, index):
    """Every stored sample in order, edit lists ignored, at its stored decode
    and composition times (`clip_check.stored_times`)."""
    return cc.stored_times(cc.packets(ffprobe, path, index, raw=True))


def check_several(entry, ffprobe, ffmpeg, mediainfo, found, counts, info):
    """A join where a file has several edits. The output's stored samples
    are its parts' (the inputs, or for a join of a source's segments the
    source) one after another: data, size, sync flag, composition offset,
    each part's times moved by one constant. Its edits present the frames
    each part's edits present, at the part's time plus its offset (for a
    source's segments, the source's frames from the first segment's start
    to the joined video's end); the inputs follow one another as in
    `check_inputs`. Each output audio edit presents the media its part's edits
    present at that time, and the output's audio covers what the parts
    present, each up to where the next one starts. Streams, ends and
    MediaInfo as for other joins."""
    output, inputs, source = entry["output"], entry["inputs"], entry.get("source")
    probe = cc.ffprobe_json(ffprobe, "-show_format", "-show_streams", output)
    oh = cc.headers(output)
    brand = probe["format"].get("tags", {}).get("major_brand")
    if "mp4" not in probe["format"].get("format_name", "") or brand != "isom":
        found.append(("format", f"output format {probe['format'].get('format_name')!r}, brand {brand!r}"))
    kinds = [("video", "video_stream")]
    if inputs[0].get("audio_stream") is not None:
        kinds.append(("audio", "audio_stream"))
    if len(probe["streams"]) != len(kinds):
        found.append(("streams", f"output has {len(probe['streams'])} streams, expected {len(kinds)}"))
        return
    if source:
        first = cc.ffprobe_json(ffprobe, "-show_streams", source)
        first_index = {kind: stream_index(first, kind) for kind, _ in kinds}
        start = cc.fraction(entry.get("source_offset") or {"num": 0, "den": 1})
        parts = [(source, None, -start)]
    else:
        first = cc.ffprobe_json(ffprobe, "-show_streams", inputs[0]["path"])
        first_index = {kind: inputs[0][key] for kind, key in kinds}
        parts = [(item["path"], item, cc.fraction(item["offset"])) for item in inputs]
        check_ends(ffprobe, inputs, kinds, output, oh, found)
    for out_index, (kind, key) in enumerate(kinds):
        out = probe["streams"][out_index]
        src = next((s for s in first["streams"] if s.get("index") == first_index[kind]), {})
        for name in cc.VIDEO_KEYS if kind == "video" else cc.AUDIO_KEYS:
            if src.get(name) != out.get(name):
                found.append((f"{kind}:{name}", f"{kind} {name}: first input {src.get(name)!r}, "
                              f"output {out.get(name)!r}"))
        indexes = [first_index[kind] if item is None else item[key] for _, item, _ in parts]
        out_track = iso_track(ffprobe, output, out_index)
        tracks = [iso_track(ffprobe, path, index) for (path, _, _), index in zip(parts, indexes)]
        if out_track is None or None in tracks:
            found.append((f"{kind}:edits", f"{kind}: an edit list cannot be read"))
            continue
        if any(t[0] != out_track[0] for t in tracks):
            found.append((f"{kind}:timescale", f"{kind}: media timescales differ"))
            continue
        # Coverage of the stored samples, and each part's run in the output:
        # (its first decode time there, its own first decode time, where its
        # samples end in its media).
        out_packets = stored(ffprobe, output, out_index)
        runs, at = [], 0
        for n, ((path, _, _), index) in enumerate(zip(parts, indexes)):
            part = stored(ffprobe, path, index)
            run = out_packets[at:at + len(part)]
            timed = [(s, o) for s, o in zip(part, run)
                     if all(isinstance(p.get(t), int) for p in (s, o) for t in ("pts", "dts"))]
            moves = {(o["dts"] - s["dts"], o["pts"] - s["pts"]) for s, o in timed}
            same = sum(1 for s, o in zip(part, run)
                       if (s.get("data_hash"), s.get("size"), (s.get("flags") or "")[:1])
                       == (o.get("data_hash"), o.get("size"), (o.get("flags") or "")[:1]))
            if len(run) != len(part) or same != len(part) or len(timed) != len(part) or len(moves) != 1 \
                    or any(a != b for a, b in moves):
                found.append((f"{kind}:packets", f"{kind}: input {n}'s stored samples are not the output's "
                              f"{at} to {at + len(part)} ({same} of {len(part)} equal, times moved by "
                              f"{sorted(moves)[:3]})"))
            counts["agree"] += same
            counts["disagree"] += max(len(part), len(run)) - same
            first_dts = part[0]["dts"] if timed else 0
            base = run[0]["dts"] if timed else 0
            end = max((s["pts"] + int(s.get("duration") or 0) for s, _ in timed), default=first_dts)
            runs.append((base, first_dts, end))
            at += len(part)
        if at < len(out_packets):
            found.append((f"{kind}:extra", f"{kind}: {len(out_packets) - at} output packets of no input"))
            counts["disagree"] += len(out_packets) - at
        if kind == "video":
            several_video(entry, ffmpeg, parts, indexes, tracks, out_track, out_index, found, info)
        else:
            several_audio(bool(source), parts, tracks, out_track, runs, found)
    if not source:
        check_mediainfo(mediainfo, inputs, kinds, output, oh, found)


def iso_frames(ffmpeg, path, index, windows, ts):
    """(time in seconds, content) of each frame a track's edits present
    (ISO/IEC 14496-12 8.6.6), from its frames decoded with edit lists
    ignored; None when they are not in media ticks."""
    base, frames = cc.framemd5(ffmpeg, path, index, video=True, raw=True)
    if base != Fraction(1, ts):
        return None
    listed = [(Fraction(pts), Fraction(duration), (size, h)) for pts, size, h, duration in frames]
    return [(t / ts, c) for t, c in cc.iso_presented(listed, windows)]


def several_video(entry, ffmpeg, parts, indexes, tracks, out_track, out_index, found, info):
    """The video of `check_several`."""
    ts, out_windows = out_track
    actual = iso_frames(ffmpeg, entry["output"], out_index, out_windows, ts)
    shown = [iso_frames(ffmpeg, path, index, windows, ts)
             for (path, _, _), index, (_, windows) in zip(parts, indexes, tracks)]
    if actual is None or None in shown:
        found.append(("video:framemd5", "video: decoded frames not in media ticks"))
        return
    inputs = entry["inputs"]
    video_end = cc.fraction(inputs[-1]["video_end"])
    if entry.get("source"):
        expected = [(t + parts[0][2], c) for t, c in shown[0] if 0 <= t + parts[0][2] < video_end]
    else:
        expected = [(t + offset, c) for (_, _, offset), frames in zip(parts, shown) for t, c in frames]
        for n, (item, frames) in enumerate(zip(inputs, shown)):
            start, end = cc.fraction(item["video_start"]), cc.fraction(item["video_end"])
            if n > 0 and start != cc.fraction(inputs[n - 1]["video_end"]):
                found.append(("video:placement", f"video: input {n} starts at {float(start):.6f} s, "
                              f"not where input {n - 1} ends"))
            moved = [t + cc.fraction(item["offset"]) for t, _ in frames]
            if moved and not (start <= moved[0] and moved[-1] < end):
                found.append(("video:placement", f"video: input {n}'s frames lie outside its span"))
            # What it presents on its own: its edits but an empty run before
            # the first media edit, plus the hold before an input whose
            # audio starts first.
            edits = cc.full_edit_list(item["path"], item["video_stream"])
            if edits and edits[2]:
                lead_in = 0
                for duration, media, _ in edits[2]:
                    if media != -1:
                        break
                    lead_in += duration
                own = Fraction(sum(d for d, _, _ in edits[2]) - lead_in, edits[0])
                lead = Fraction(0)
                if n + 1 < len(inputs):
                    lead = cc.fraction(inputs[n + 1]["video_start"]) - cc.fraction(inputs[n + 1]["offset"])
                if end - start > own + lead:
                    found.append(("video:placement", f"video: input {n} spans {float(end - start):.6f} s, "
                                  f"more than its edits' {float(own):.6f} s"))
    if expected != actual:
        first = next((i for i, (a, b) in enumerate(zip(expected, actual)) if a != b),
                     min(len(expected), len(actual)))
        found.append(("video:framemd5", f"video frames the edits present differ at frame {first} "
                      f"({len(expected)} from the inputs, {len(actual)} in the output)"))
    # The second view: ffmpeg's edit-applied frames, reported.
    ob, of = cc.framemd5(ffmpeg, entry["output"], out_index, video=True)
    applied = []
    for (path, _, offset), index in zip(parts, indexes):
        ib, frames = cc.framemd5(ffmpeg, path, index, video=True)
        applied += [(pts * ib + offset, h) for pts, _, h, _ in frames] if ib else []
    if entry.get("source"):
        applied = [f for f in applied if 0 <= f[0] < video_end]
    mine = [(pts * ob, h) for pts, _, h, _ in of] if ob else []
    differ = len(set(applied) ^ set(mine))
    info.append("second view, video: " + ("agrees" if not differ else
                f"{differ} frames differ in ffmpeg's edit-applied decode"))


def several_audio(segments, parts, tracks, out_track, runs, found):
    """The audio of `check_several`: each output edit presents, at its time,
    the media its part's edits present there, within one tick (edits are
    rounded to the movie timescale); and the output presents what the parts
    present, each from its offset up to where the next one starts (for a
    source's segments, from the first one's start to the output's end),
    within one tick at each boundary."""
    ts, out_windows = out_track

    def merged(spans):
        out = []
        for a, b in sorted(spans):
            if out and a <= out[-1][1] + 1:
                out[-1] = (out[-1][0], max(out[-1][1], b))
            elif b > a:
                out.append((a, b))
        return out
    bounds = [base for base, _, _ in runs[1:]] + [Fraction(1 << 62)]
    for m0, m1, at in out_windows:
        for (_, _, offset), (_, windows), (base, first_dts, _), next_base in zip(parts, tracks, runs, bounds):
            lo, hi = max(m0, base), min(m1, next_base)
            if lo >= hi:
                continue
            sm0, sm1 = lo - base + first_dts, hi - base + first_dts
            s_at = at + lo - m0 - offset * ts
            holder = next((w for w in windows if w[0] - 1 <= sm0 and sm1 <= w[1] + 1), None)
            if holder is None or abs(sm0 - holder[0] + holder[2] - s_at) > 1:
                found.append(("audio:edits", f"audio: the output's edit at {float(at / ts):.6f} s presents "
                              "other media than its input's edits"))
                return
    shown = merged([(at, at + m1 - m0) for m0, m1, at in out_windows])
    pieces = []
    for (_, _, offset), (_, windows), (_, first_dts, media_end) in zip(parts, tracks, runs):
        spans = [(at + max(m0, first_dts) - m0, at + min(m1, media_end) - m0)
                 for m0, m1, at in windows if min(m1, media_end) > max(m0, first_dts)]
        pieces.append(merged([(a + offset * ts, b + offset * ts) for a, b in spans]))
    expected = []
    for n, spans in enumerate(pieces):
        if segments:
            until, spans = (shown[-1][1] if shown else 0), [(max(a, 0), b) for a, b in spans if b > 0]
        else:
            until = next((p[0][0] for p in pieces[n + 1:] if p), None)
        expected += [(a, b if until is None else min(b, until)) for a, b in spans if until is None or a < until]
    expected = merged(expected)
    if len(expected) != len(shown) or any(abs(a - c) > 1 or abs(b - d) > 1
                                          for (a, b), (c, d) in zip(expected, shown)):
        found.append(("audio:edits", "audio: the output presents "
                      f"{[(float(a / ts), float(b / ts)) for a, b in shown][:4]} s, its inputs "
                      f"{[(float(a / ts), float(b / ts)) for a, b in expected][:4]} s"))


def concat_list(paths):
    """A concat demuxer list naming `paths`, each quoted (FFmpeg's quoting:
    a ' closes the quote, is escaped, and the quote reopens)."""
    return "".join("file '" + str(p).replace("'", "'\\''") + "'\n" for p in paths)


def check_ffmpeg_concat(entry, ffprobe, ffmpeg, found, info):
    """The join against `ffmpeg -f concat -safe 0 -i LIST -c copy` on the same
    inputs, run as a black box: for each kept stream, the same packets one
    for one (data hash, size, sync flag). ffmpeg's demuxer converts H.264 to
    Annex B on the way (its `auto_convert` option), which puts the parameter
    sets into sync samples; there the packets agree once both are read
    without NAL units of type 7 and 8 (`filter_units`). Not compared, and said
    so: inputs whose kept streams are at other indexes than the first's (the
    demuxer pairs streams by index), and joins with several edits (its
    demuxer applies its own edit model and drops or repeats samples, as
    docs/design/edit-list-writers.md records)."""
    inputs = entry["inputs"]
    keys = [k for k in ("video_stream", "audio_stream") if inputs[0].get(k) is not None]
    if any(item.get(k) != inputs[0][k] for item in inputs for k in keys):
        info.append("ffmpeg concat: not compared, the inputs' streams are at other indexes")
        return
    if has_several(entry, ffprobe):
        info.append("ffmpeg concat: not compared, several edits")
        return
    work = pathlib.Path(tempfile.mkdtemp(dir=cc._work()))
    try:
        listing = work / "list.txt"
        listing.write_text(concat_list(item["path"] for item in inputs), encoding="utf-8")
        reference = work / "concat.mp4"
        maps = [a for k in keys for a in ("-map", f"0:{inputs[0][k]}")]
        try:
            cc.run([ffmpeg, "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i", str(listing), *maps, "-c",
                    "copy", str(reference)])
        except RuntimeError as error:
            info.append(f"ffmpeg concat: not compared, ffmpeg fails on these inputs ({str(error)[-120:]})")
            return

        def without_parameter_sets(path):
            """The video's packets, every stored one, without NAL units of
            types 7 and 8."""
            stripped = work / f"{pathlib.Path(path).stem}.stripped.mp4"
            cc.run([ffmpeg, "-v", "error", "-y", "-ignore_editlist", "1", "-i", str(path), "-map", "0:v:0", "-c",
                    "copy", "-bsf:v", "filter_units=remove_types=7|8", str(stripped)])
            return cc.packets(ffprobe, str(stripped), 0, raw=True)
        h264 = cc.ffprobe_json(ffprobe, "-show_streams", str(reference))["streams"][0].get("codec_name") == "h264"
        stripped = None
        if h264:
            try:
                stripped = (without_parameter_sets(entry["output"]), without_parameter_sets(reference))
            except RuntimeError as error:
                found.append(("concat:video", f"video: cannot read the packets without parameter sets ({error})"))
                return
        times = 0
        for out_index, key in enumerate(keys):
            kind = key.split("_")[0]
            ours = cc.packets(ffprobe, entry["output"], out_index, raw=True)
            theirs = cc.packets(ffprobe, str(reference), out_index, raw=True)
            if len(ours) != len(theirs):
                found.append((f"concat:{kind}", f"{kind}: {len(ours)} packets, ffmpeg concat {len(theirs)}"))
                continue
            plain = [(p.get("data_hash"), p.get("size"), (p.get("flags") or "")[:1]) for p in ours]
            other = [(p.get("data_hash"), p.get("size"), (p.get("flags") or "")[:1]) for p in theirs]
            if kind == "video" and stripped:
                ours_s, theirs_s = stripped
                same = [a == b or (a[2] == "K" and len(ours_s) == len(theirs_s) == len(ours)
                                   and ours_s[n].get("data_hash") == theirs_s[n].get("data_hash"))
                        for n, (a, b) in enumerate(zip(plain, other))]
            else:
                same = [a == b for a, b in zip(plain, other)]
            if not all(same):
                found.append((f"concat:{kind}", f"{kind}: {same.count(False)} packets differ from ffmpeg concat's, "
                              f"first {same.index(False)}"))
            times += [p.get("pts") for p in cc.packets(ffprobe, entry["output"], out_index)] == \
                [p.get("pts") for p in cc.packets(ffprobe, str(reference), out_index)]
        info.append(f"ffmpeg concat: packets compared; presentation times equal in {times} of {len(keys)} streams")
    finally:
        for path in work.iterdir():
            path.unlink()
        work.rmdir()


# Known divergences of joins (docs/known-divergences-join.md).

def audio_edit_start(field, fields, c):
    """KD-J-AUDIO-EDIT-START."""
    if field == "audio:pcm":
        if not c.get("audio_pieces"):
            return False
        pieces, ts, rate, spans, ref_path, out_path, frame = c["audio_pieces"]
        if len(pieces) < 2:
            return False
        return not compare_pieces(pieces, ts, rate, spans, ref_path, out_path, frame, rounded=True)
    if field != "audio:packets" or not c.get("audio_only_timing") or c.get("audio_missing") != 0:
        return False
    oh = c.get("output_headers")
    trak = oh["traks"][1] if oh and len(oh["traks"]) > 1 else None
    if not trak or not trak.get("elst"):
        return False
    media = [t for _, t in trak["elst"] if t != -1]
    # Every input whose packet times differ starts at an edit after the
    # first, inside a frame, and every one of its times, pts and dts, is
    # off by the distance to the next frame boundary: less than a frame.
    frame = c.get("audio_frame")
    timing = c.get("audio_timing") or {}
    if len(media) < 2 or not frame or not timing:
        return False
    for n in timing:
        if n == 0 or not edit_kept(c, "audio", n):
            return False
        # The distance from the edit's media time to the next packet start,
        # read from the input; within less than a tick, as an input whose
        # offset falls between ticks of the time base is compared.
        gap = c["audio_edits"]["inputs"][n]["gap"]
        shift = one_shift(c["audio_deltas"][n])
        base = c["audio_base"]
        if shift is None or not 0 < shift < frame or gap is None or not abs(shift * base - gap) < base:
            return False
    return True


def edit_kept(c, kind, n):
    """Whether the output holds input `n`'s edit as the input's own file
    and the output's stored decode times give it: starting at its offset,
    from its own first media time."""
    edits = c[f"{kind}_edits"]
    want = edits["inputs"][n]
    return want is not None and (want["start"], want["media"]) in edits["output"]


def one_shift(deltas, dts_term=0):
    """The one constant every compared packet's pts of an input is off by,
    when its dts are all off by that constant plus `dts_term`; else None."""
    if len(deltas["pts"]) != 1:
        return None
    shift = next(iter(deltas["pts"]))
    return shift if deltas["dts"] == {shift + dts_term} else None


def dts_terms(c):
    """KD-J-DTS-SHIFT's term for each input, in the output's time base: how
    much further `ffprobe` moves the output's decode times back than the
    input's (by the smallest composition offset when it is negative). None
    unless the output's smallest offset is the inputs' smallest, read from
    the inputs' own files."""
    (out_min, out_scale), inputs = c["video_ctts_min"]
    base = c["video_base"]
    if not out_scale or not base or not inputs or not all(scale for _, scale in inputs):
        return None

    def delay(smallest, scale):
        return Fraction(max(0, -(smallest or 0)), scale)

    shifted = delay(out_min, out_scale)
    if shifted != max(delay(m, scale) for m, scale in inputs):
        return None
    return [(shifted - delay(m, scale)) / base for m, scale in inputs]


def video_edit_end(field, fields, c):
    """KD-J-VIDEO-EDIT-END."""
    if field != "video:framemd5" or "video:packets" in fields or not c.get("video_frames"):
        return False
    expected, actual = c["video_frames"]
    ends = c.get("video_input_ends") or []
    # The expected frames with each input's first frame moved to the end of
    # the frame before it, where that frame runs past the input's start,
    # and that first frame possibly listed twice.
    candidates = [list(expected)]
    moved_any = False
    stored_ends = c.get("video_stored_ends") or []
    durations = c.get("video_frame_durations") or []
    for n, end in enumerate(ends[:-1]):
        if end <= 0 or end >= len(expected):
            continue
        (t_prev, _, _), (t_first, size, h) = expected[end - 1], expected[end]
        # Where the cut frame ends in full: its own duration as ffmpeg
        # decodes it (else the step before it), or where the input's stored
        # frames end.
        frame = durations[end - 1] if len(durations) == len(expected) else None
        if not frame and end >= 2:
            frame = t_prev - expected[end - 2][0]
        times = {t_prev + frame} if frame else set()
        if n < len(stored_ends) and stored_ends[n] is not None:
            times.add(stored_ends[n])
        times = {t for t in times if t > t_first}
        if not times:
            continue
        moved = []
        for candidate in candidates:
            for at in times:
                shifted = list(candidate)
                shifted[end] = (at, size, h)
                moved.append(shifted)
                twice = list(shifted)
                twice.insert(end, (at, size, h))
                moved.append(twice)
        candidates = moved
        moved_any = True
    return moved_any and any(candidate == actual for candidate in candidates)


def video_later_edit(field, fields, c):
    """KD-J-VIDEO-LATER-EDIT."""
    if field not in ("video:framemd5", "video:packets", "video:extra"):
        return False
    delays = c.get("video_delays")
    if not delays:
        return False
    if field != "video:framemd5":
        # The packets: every stored sample once, in order, with the
        # inputs' data; the first input's times exact, later inputs' times
        # as ffmpeg maps them through edits after the first.
        oh = c.get("output_headers")
        trak = oh["traks"][0] if oh and oh["traks"] else {}
        media = [t for _, t in trak.get("elst") or [] if t != -1]
        timing = c.get("video_timing") or {}
        if not (field == "video:packets" and bool(c.get("video_only_timing")) and c.get("video_missing") == 0
                and timing and 0 not in timing and len(media) > 1):
            return False
        # Each later input's presentation times shifted by one constant
        # other than 0, within the same bounds as its frames, and its decode
        # times by the same constant plus KD-J-DTS-SHIFT's term.
        base = c.get("video_base")
        terms = dts_terms(c)
        if not base or terms is None:
            return False
        for n in timing:
            if n >= len(delays) or n >= len(terms):
                return False
            shift = one_shift(c["video_deltas"][n], terms[n])
            if not shift or not -delays[0] <= -shift * base <= delays[n] or not edit_kept(c, "video", n):
                return False
        return True
    expected, actual = c.get("video_frames") or ([], [])
    ends = c.get("video_input_ends") or []
    if len(expected) != len(actual) or not ends:
        return False
    begin = 0
    for n, end in enumerate(ends):
        pairs = list(zip(expected[begin:end], actual[begin:end]))
        if any(e[1:] != a[1:] for e, a in pairs):
            return False
        shifts = collections.Counter(a[0] - e[0] for e, a in pairs)
        if n == 0:
            if set(shifts) != {0}:
                return False
        else:
            # One shift for the input, at most the first input's delay
            # early or its own late; frames it would put before the
            # previous frame shown at that frame's time.
            shift = shifts.most_common(1)[0][0]
            if not -delays[0] <= shift <= delays[n] or (shift and not edit_kept(c, "video", n)):
                return False
            previous = actual[begin - 1][0] if begin > 0 else None
            for e, a in pairs:
                want = e[0] + shift
                if previous is not None and want < previous:
                    want = previous
                if a[0] != want:
                    return False
                previous = a[0]
        begin = end
    return True


def dts_shift(field, fields, c):
    """KD-J-DTS-SHIFT."""
    if field != "video:packets" or not c.get("video_only_timing") or c.get("video_missing") != 0:
        return False
    timing = c.get("video_timing") or {}
    terms = dts_terms(c) if timing else None
    if terms is None:
        return False
    # Every input whose decode times differ: its presentation times exact,
    # its decode times all off by its term, above 0.
    for n in timing:
        if n >= len(terms) or terms[n] <= 0 or c["video_deltas"][n] != {"pts": {0}, "dts": {terms[n]}}:
            return False
    return True


def source_pts_shift(field, fields, c):
    """KD-J-PTS-SHIFT."""
    if field not in ("video:start_time", "video:packets", "video:framemd5"):
        return False
    sh, oh = c.get("source_headers"), c.get("output_headers")
    index = c.get("source_video_index")
    if not sh or not oh or index is None or len(sh["traks"]) <= index or not oh["traks"]:
        return False
    if sh["traks"][index].get("elst"):
        return False
    smallest = oh["traks"][0].get("ctts_min")
    timescale = oh["traks"][0].get("mdhd", (None, 0))[0]
    if smallest is None or smallest >= 0 or not timescale:
        return False
    shift = Fraction(-smallest, timescale)
    if field == "video:start_time":
        return c.get("source_video_start_gap") == shift
    if field == "video:packets":
        data_same, deltas, base, same_base = c.get("source_video_packets") or (False, {}, None, False)
        ticks = shift / base if base else None
        return bool(data_same and same_base and deltas.get("pts") == {ticks} and deltas.get("dts") == {ticks})
    frames = c.get("source_video_frames")
    if not frames:
        return False
    sb, sf, ob, of, offset, video_end = frames
    end = video_end + offset + shift
    expected = [(t * sb, size, h) for t, size, h, _ in sf if t * sb < end]
    at_end = [(t * sb, size, h) for t, size, h, _ in sf if t * sb == end]
    actual = [(round((t * ob + offset + shift) / sb) * sb, size, h) for t, size, h, _ in of]
    return actual in (expected, expected + at_end)


def sync_verified(field, fields, c):
    """KD-J-SYNC-VERIFIED."""
    stored = c.get("source_video_stored")
    if field != "video:packets" or not stored or not c.get("ffprobe"):
        return False
    a, b, shift, output, index = stored
    dropped = cc.dropped_sync_flags(a, b, shift)
    return bool(dropped) and cc.flags_decoded_keys(c["ffprobe"], output, index, b)


def audio_noise(field, fields, c):
    """KD-J-AUDIO-NOISE."""
    if field != "audio:pcm" or not c.get("audio_pieces") or not c.get("audio_delivered"):
        return False
    pieces, ts, rate, spans, _, out_path, frame = c["audio_pieces"]
    delivered = c["audio_delivered"]()
    if delivered is None:
        return False
    try:
        return not compare_pieces(pieces, ts, rate, spans, delivered, out_path, frame, rounded=len(pieces) > 1)
    finally:
        pathlib.Path(delivered[0]).unlink(missing_ok=True)


PREDICATES = {
    "video_later_edit": video_later_edit,
    "sync_verified": sync_verified,
    "source_pts_shift": source_pts_shift,
    "dts_shift": dts_shift,
    "audio_edit_start": audio_edit_start,
    "video_edit_end": video_edit_end,
    "audio_noise": audio_noise,
}


def profile_evidence(evidence, ffprobe, probe, src, path, src_index, output, out_index):
    """What KD-C-AAC-PROFILE compares: the streams and the configuration
    bytes as ffprobe dumps them."""
    evidence["output_streams"] = probe["streams"]
    evidence["audio_source_stream"] = src
    evidence["audio_source_extradata"] = cc.extradata(ffprobe, path, src_index)
    evidence["audio_output_extradata"] = cc.extradata(ffprobe, output, out_index)


# The clip's entries that apply to joins as they are: the others read
# evidence a join check does not gather.
CLIP_KDS = ("KD-C-AAC-PROFILE",)


def covering(field, fields, evidence):
    """The id of the known divergence that covers `field`: the join's own
    entries, then those of the clip's in `CLIP_KDS`
    (docs/known-divergences-join.md). None when no entry covers it."""
    clip = [kd for kd in cc.known_divergences() if kd[0] in CLIP_KDS]
    for kd_id, pattern, predicate in known_divergences() + clip:
        if not fnmatch.fnmatch(field, pattern):
            continue
        try:
            if predicate(field, fields, evidence):
                return kd_id
        except (KeyError, TypeError, ValueError, IndexError, RuntimeError):
            continue
    return None


def known_divergences():
    entries = json.loads(KD_FILE.read_text(encoding="utf-8"))["entries"]
    return [(e["id"], e["field_pattern"], PREDICATES[e["predicate"]]) for e in entries]


def check(entry, ffprobe, ffmpeg, mediainfo, decode=True, concat=False):
    """Returns the problems, the informational differences, the covered
    differences and the packet counts of one join. Without `decode`, the
    decoded video and audio are not compared; with `concat`, the output is
    also compared with `ffmpeg -f concat`'s (`check_ffmpeg_concat`)."""
    found, info, covered = [], [], []
    counts = {"agree": 0, "disagree": 0}
    evidence = {}
    try:
        if has_several(entry, ffprobe):
            check_several(entry, ffprobe, ffmpeg, mediainfo, found, counts, info)
        elif entry.get("source"):
            check_source(entry, ffprobe, ffmpeg, found, counts, evidence, decode)
        else:
            check_inputs(entry, ffprobe, ffmpeg, mediainfo, found, counts, evidence, decode)
        if concat:
            check_ffmpeg_concat(entry, ffprobe, ffmpeg, found, info)
        problems = []
        fields = {field for field, _ in found}
        for field, message in found:
            kd = covering(field, fields, evidence)
            if kd:
                covered.append({"kd": kd, "problem": message})
                if field.endswith(":packets"):
                    kind = field.split(':')[0]
                    moved = evidence.get(f"{kind}_mismatched", evidence.get(f"source_{kind}_mismatched", 0))
                    counts["agree"] += moved
                    counts["disagree"] -= moved
            else:
                problems.append(message)
    finally:
        pieces = evidence.get("audio_pieces")
        if pieces:
            pathlib.Path(pieces[4][0]).unlink(missing_ok=True)
            pathlib.Path(pieces[5]).unlink(missing_ok=True)
    return problems, info, covered, counts


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--mediainfo")
    parser.add_argument("--report")
    parser.add_argument("--ffmpeg-concat", action="store_true",
                        help="also compare each output with ffmpeg -f concat -c copy on its inputs")
    args = parser.parse_args()
    with open(args.manifest, encoding="utf-8") as f:
        manifest = json.load(f)
    results = []
    for entry in manifest:
        try:
            problems, info, covered, counts = check(entry, args.ffprobe, args.ffmpeg, args.mediainfo,
                                                    concat=args.ffmpeg_concat)
        except (RuntimeError, ValueError, KeyError) as error:
            problems, info, covered, counts = [f"harness: {error}"], [], [], {"agree": 0, "disagree": 0}
        results.append({**entry, "problems": problems, "informational": info, "covered": covered,
                        "packets": counts})
        print(f"{'FAIL' if problems else 'kd  ' if covered else 'ok  '} {entry['label']}")
        for problem in problems:
            print(f"     {problem}")
        for item in covered:
            print(f"     ({item['kd']}) {item['problem']}")
    cc.clear_cache()
    passed = sum(1 for r in results if not r["problems"])
    direct = sum(1 for r in results if not r["problems"] and not r["covered"])
    agree = sum(r["packets"]["agree"] for r in results)
    disagree = sum(r["packets"]["disagree"] for r in results)
    print(f"{passed}/{len(results)} joins pass every oracle ({direct} directly, {passed - direct} through "
          f"known divergences); packets {agree}/{agree + disagree} agree")
    if args.report:
        with open(args.report, "w", encoding="utf-8") as f:
            json.dump({"passed": passed, "direct": direct, "total": len(results),
                       "packets": {"agree": agree, "disagree": disagree}, "results": results}, f, indent=2)
    return 0 if results and passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
