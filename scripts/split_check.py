#!/usr/bin/env python3
"""Black-box oracles for keyframe splits (docs/design/split.md).

Reads a manifest of splits (from the `fixtures_round_trip` test of
coreav-isobmff-write's `split` tests, COREAV_SPLIT_OUT, or from the corpus
harness) and checks every split against its source with the pinned ffprobe
and ffmpeg binaries, and with MediaInfo when --mediainfo is given:

- coverage, the property a split promises: for each kept stream, the
  stored samples `ffprobe -show_packets -show_data_hash -ignore_editlist 1`
  lists for the segments, taken in segment order, are the source stream's
  stored samples one for one: the same data hash, size and keyframe flag,
  and, where both files present the sample under their edits, the
  source's times minus the segment's start (exactly for the video, within
  one tick for the audio, whose edits are rounded to whole ticks). A source packet that no segment
  holds, one that two segments hold, or one out of order is a failure. Every
  packet is counted towards the packet bar;
- every segment reads as an MP4 file (major brand `isom`) holding the kept
  video stream and, when the segment has audio, the kept audio stream, each
  with the source stream's codec, profile, dimensions, rotation, sample rate
  and channel layout; its video edit runs from its video start to its end;
  ffprobe's stream durations and start times are its edits;
- `ffmpeg -copyts -f framemd5` of the segments' video, each shifted by its
  start, is the source's decoded video frame for frame: every frame the
  source presents before the video's end (the last segment's end, where a
  clip to the end ends) is presented by exactly one segment, at the same
  time and with the same hash;
- each segment's decoded audio equals the source's, sample for sample,
  from its second decoded frame on; the first frame of every segment but
  the first is decoded without the frame before it (a split copies each
  packet once and adds no pre-roll; docs/design/split.md);
- MediaInfo reports an MPEG-4 file per segment whose tracks have the
  source's format, dimensions, sample rate and channels, and durations
  equal to the track header durations.

Each segment is written by the clip writer's code (its tables, file head
and copy pass), so a difference in one segment's streams, durations or
decoded audio counts as agreement when an entry of
docs/known-divergences-clip.md covers it, rechecked by that entry's
predicate from scripts/clip_check.py on the segment as if it were a clip;
covered splits are reported apart from direct agreement. The coverage of
the packets and of the decoded video frames is the split's own promise:
only the split's own entries (docs/known-divergences-split.md) apply to it,
for times and keyframe flags that the oracles report differently.

The binaries are black boxes; no implementation source
is read. Exits 1 if any check fails or the manifest is empty.
"""

import argparse
import fnmatch
import json
import os
import pathlib
import sys
from bisect import bisect_left, bisect_right
from collections import Counter
from fractions import Fraction

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import clip_check as cc  # noqa: E402


def segment_streams(segment, video_stream, audio_stream):
    """[(kind, source stream index, output stream index)] of a segment."""
    kept = [("video", video_stream, 0)]
    if audio_stream is not None and segment["audio_samples"]:
        kept.append(("audio", audio_stream, 1))
    return kept


def raw_packets(ffprobe, path, index):
    """`ffprobe`'s packets of a stream with edit lists ignored: every stored
    sample. With an edit list, `ffprobe` leaves out a sample of decode
    duration 0 that the edit does not present (a last sample at the
    stream's end); ignoring the edits lists it."""
    data = cc.ffprobe_json(ffprobe, "-ignore_editlist", "1", "-select_streams", str(index), "-show_packets",
                           "-show_data_hash", "SHA256", "-show_entries", "packet=" + ",".join(cc.PACKET_KEYS),
                           path)
    return data.get("packets", [])


def segment_packets(ffprobe, path, index):
    """A file's packets for the coverage check, the source's and each
    segment's alike: every stored sample, in order (edit lists ignored),
    with the times `ffprobe` gives it under the file's edits where it lists
    it there, and no times where it does not (a sample the edit leaves
    unpresented)."""
    listed = {p.get("pos"): p for p in cc.packets(ffprobe, path, index)}
    out = []
    for raw in raw_packets(ffprobe, path, index):
        timed = listed.get(raw.get("pos"))
        if timed is not None:
            out.append(timed)
        else:
            out.append({**raw, "pts": None, "dts": None, "unpresented": True})
    return out


def compare_coverage(kind, src, outs, found, evidence=None):
    """Pairs the source's packets with the segments' packets in order.
    `outs` is [(packets, shift in the stream's time base)]. Returns
    (agreeing, disagreeing) packet counts. With `evidence`, records per
    segment the time offsets of the packets that differ in time only, for
    the known divergences."""
    agree = disagree = 0
    i = 0
    mismatched = []
    timing = {}
    other = 0
    # Packets that differ in times or keyframe flags only.
    alike = 0
    for n, (packets, shift) in enumerate(outs):
        tolerance = 0 if kind == "video" and shift.denominator == 1 else 1
        for o in packets:
            s = src[i] if i < len(src) else None
            if s is None:
                disagree += 1
                found.append((f"{kind}:extra", f"{kind}: segment {n} holds a packet past the source's last"))
                continue
            i += 1
            stored = o.get("data_hash") == s.get("data_hash") and o.get("size") == s.get("size")
            data = stored and o.get("flags", "")[:1] == s.get("flags", "")[:1]
            same = data
            deltas = {}
            if o.get("unpresented") or s.get("unpresented"):
                # Listed only with edits ignored, in the segment or in the
                # source: its data and place count, its times are those of
                # an unpresented sample (the frames are compared decoded).
                if data:
                    agree += 1
                else:
                    disagree += 1
                    mismatched.append((n, i - 1))
                    other += 1
                    alike += stored
                continue
            for key in ("pts", "dts"):
                a, b = s.get(key), o.get(key)
                if not isinstance(a, int) or not isinstance(b, int):
                    same = False
                    data = False
                    continue
                deltas[key] = Fraction(a) - (b + shift)
                if abs(deltas[key]) > tolerance:
                    same = False
            if same:
                agree += 1
            else:
                disagree += 1
                mismatched.append((n, i - 1))
                alike += stored
                if data:
                    entry = timing.setdefault(n, {"pts": set(), "dts": set(), "count": 0})
                    entry["pts"].add(deltas["pts"])
                    entry["dts"].add(deltas["dts"])
                    entry["count"] += 1
                else:
                    other += 1
    missing = len(src) - i
    if missing > 0:
        disagree += missing
        found.append((f"{kind}:missing", f"{kind}: {missing} source packets in no segment"))
    if mismatched:
        found.append((f"{kind}:packets", f"{kind}: {len(mismatched)} packets differ, first (segment, packet) "
                      f"{mismatched[:3]} of {len(src)}"))
    if evidence is not None:
        # Packets that differ in time only can be covered; any other
        # difference cannot.
        evidence[f"{kind}_timing"] = timing if not other else None
        evidence[f"{kind}_timing_count"] = sum(t["count"] for t in timing.values())
        evidence[f"{kind}_alike_count"] = alike
    return agree, disagree


def check_video_frames(src, outs, found, offsets=None, end=None):
    """`outs` is [((time base, frames), start)]: the segments' frames,
    shifted by their starts, must be the source's frames in order, those
    presented before `end` (the last segment's end, which is the video's
    end as a clip to the end has it).
    `offsets` holds a time offset per segment, in source ticks, applied to
    the segments' times (for `KD-S-EDIT-MIDFRAME`). Returns whether they
    are."""
    sb, sf = src
    if sb is None:
        found.append(("video:framemd5", "source framemd5 without a time base"))
        return False
    expected = [(pts * sb, size, h) for pts, size, h, _ in sf if end is None or pts * sb < end]
    # A frame presented exactly at the end has no presented duration; ffmpeg
    # may still show it, as the clip check allows.
    at_end = [(pts * sb, size, h) for pts, size, h, _ in sf if end is not None and pts * sb == end]
    actual = []
    for n, ((ob, of), start) in enumerate(outs):
        if ob is None:
            found.append(("video:framemd5", "segment framemd5 without a time base"))
            return False
        offset = (offsets or {}).get(n, 0) * sb
        # A start that is not a whole number of source ticks (an empty edit
        # converted from the movie timescale) is compared on the source's
        # ticks, as ffmpeg rounds the source's times to them.
        actual.extend((round((pts * ob + start + offset) / sb) * sb, size, h) for pts, size, h, _ in of)
    if actual not in (expected, expected + at_end):
        first = next((i for i, (a, b) in enumerate(zip(expected, actual)) if a != b),
                     min(len(expected), len(actual)))
        found.append(("video:framemd5", f"video framemd5 differs at frame {first} ({len(expected)} source "
                      f"frames, {len(actual)} in the segments)"))
        return False
    return True


def audio_matches(src, out, start, rate, skip_first):
    """Whether the segment's decoded audio from its second frame on (from
    its first in the first segment) equals the source's at the same time;
    None when the segment's audio cannot be placed in the source's."""
    (s_spans, _, s_raw), (o_spans, o_channels, o_raw) = src, out
    frame = 4 * o_channels
    total = os.path.getsize(o_raw) // frame
    skip = o_spans[0][1] if skip_first else 0
    if total <= skip:
        return True
    exact = cc.position(s_spans, start + o_spans[0][0], rate)
    if exact is None:
        return None
    with open(o_raw, "rb") as f:
        f.seek(skip * frame)
        tail = f.read()
    tail_path = o_raw + ".tail"
    with open(tail_path, "wb") as f:
        f.write(tail)
    try:
        return any(offset >= 0 and cc.same_bytes(s_raw, (offset + skip) * frame, tail_path)
                   for offset in sorted({int(exact) - 1, int(exact), int(exact) + 1, round(exact)}))
    finally:
        pathlib.Path(tail_path).unlink(missing_ok=True)


def edit_total(entries, movie_timescale):
    """(leading empty, span) of a list of edit list entries in seconds: the
    empty edits before the first media edit, and everything from it on, the
    gaps between media edits included (ISO/IEC 14496-12 8.6.6). The entries
    are `cc.full_edit_list`'s, which reads every one of them; `cc.headers`
    keeps only the first eight."""
    lead, span, seen = Fraction(0), Fraction(0), False
    for duration, media_time, _rate in entries or []:
        if media_time == -1 and not seen:
            lead += Fraction(duration, movie_timescale)
        else:
            seen = True
            span += Fraction(duration, movie_timescale)
    return lead, span


def segment_overhang(oh, kept, segment, movie_timescale, start, end):
    """How much longer the segment's movie lasts than its video, and the
    most that may be: a segment's audio starts up to one frame after its
    video and may end up to one frame after it (docs/design/split.md), so
    away from the last segment the movie is its video plus at most a
    frame. Where a gap the source's edits leave were counted twice — once
    as audio in the segment before it, once as the empty edit of the
    segment it belongs to — the two segments would last longer than the
    source together (split decision 3)."""
    over = Fraction(oh["mvhd"][1], movie_timescale) - (end - start)
    frame = Fraction(0)
    samples = segment.get("audio_samples") or 0
    if len(kept) > 1 and samples:
        timescale, duration = oh["traks"][1]["mdhd"]
        frame = Fraction(duration, timescale * samples)
    return over, frame


def decoded_run(src, out, first_dts, timescale):
    """Why a segment's decoded audio from its second frame on is not the
    source's run, both decoded with edit lists ignored; None where it is.
    `first_dts` is the source decode time of the segment's first audio
    sample."""
    if src is None or out is None:
        return "audio did not decode"
    (s_spans, s_channels, s_raw), (o_spans, o_channels, o_raw) = src, out
    if s_channels != o_channels:
        return f"{s_channels} source and {o_channels} output channels"
    if len(o_spans) < 2:
        return None
    target = o_spans[1][0] + Fraction(first_dts, timescale)
    offset, seen = None, 0
    for start, count in s_spans:
        if abs(start - target) * timescale < Fraction(1, 2):
            offset = seen
            break
        seen += count
    if offset is None or not cc.same_bytes(s_raw, offset * 4 * o_channels, o_raw,
                                           offset_b=o_spans[0][1] * 4 * o_channels):
        return "the decoded audio differs from the source's run"
    return None


def check_segment(n, segment, entry, src_streams, ffprobe, ffmpeg, mediainfo, src_pcm, several=None,
                  edits=None, second_view=True, last=False, raw_audio=None):
    """The checks of one segment as a file: (found, info, evidence, probe,
    headers). `found` holds (field, message) pairs. For a kind in `several`
    the edits are read as a list of several (docs/design/edit-list-writers.md,
    split section): the durations come from the output's own boxes and the
    decoded audio is compared across the whole split, not here."""
    several = several or {}
    edits = edits or {}
    source, output = entry["source"], segment["output"]
    video_stream, audio_stream = entry["video_stream"], entry.get("audio_stream")
    found, info = [], []
    probe = cc.ffprobe_json(ffprobe, "-show_format", "-show_streams", output)
    oh = cc.headers(output)
    kept = segment_streams(segment, video_stream, audio_stream)
    clip_entry = {
        "source": source,
        "output": output,
        "video": {"stream_index": video_stream, "first_sample": segment["video_first"],
                  "samples": segment["video_samples"]},
        "audio": ({"stream_index": audio_stream, "first_sample": segment["audio_first"],
                   "samples": segment["audio_samples"]} if len(kept) > 1 else None),
        "snapped_start": segment["start"],
        "written_end": segment["end"],
    }
    evidence = {"source_headers": cc.cached((source, "headers"), lambda: cc.headers(source)),
                "output_headers": oh, "entry": clip_entry, "ffmpeg": ffmpeg,
                "output_streams": probe["streams"]}
    brand = probe["format"].get("tags", {}).get("major_brand")
    if "mp4" not in probe["format"].get("format_name", "") or brand != "isom":
        found.append(("format", f"format {probe['format'].get('format_name')!r}, brand {brand!r}"))
    if len(probe["streams"]) != len(kept) or not oh or len(oh["traks"]) != len(kept):
        found.append(("streams", f"{len(probe['streams'])} streams, expected {len(kept)}"))
        return found, info, evidence, probe, oh
    movie_timescale = oh["mvhd"][0]
    start = cc.fraction(segment["start"])
    video_start, end = cc.fraction(segment["video_start"]), cc.fraction(segment["end"])
    for kind, src_index, out_index in kept:
        src, out = src_streams.get(src_index), probe["streams"][out_index]
        if src is None:
            found.append((f"{kind}:stream", f"{kind}: no source stream {src_index}"))
            continue
        for key in cc.VIDEO_KEYS if kind == "video" else cc.AUDIO_KEYS:
            if src.get(key) != out.get(key):
                found.append((f"{kind}:{key}", f"{kind} {key}: source {src.get(key)!r}, output {out.get(key)!r}"))
                if kind == "audio" and key == "profile":
                    evidence["audio_source_stream"] = src
                    evidence["audio_source_extradata"] = cc.cached(
                        (source, "extradata", src_index), lambda: cc.extradata(ffprobe, source, src_index))
                    evidence["audio_output_extradata"] = cc.extradata(ffprobe, output, out_index)
        if kind == "video" and cc.rotation(src) != cc.rotation(out):
            found.append(("video:rotation", f"video rotation: source {cc.rotation(src)}, "
                          f"output {cc.rotation(out)}"))
        trak = oh["traks"][out_index]
        timescale = trak["mdhd"][0]
        if several.get(kind):
            # Several edits: the track header lasts its edits (8.3.2), and
            # the video's edits run from its start to the segment's end,
            # the gaps between its windows included. ffprobe's duration and
            # start time follow its own edit model, which common decision 3
            # keeps out of the gating view.
            lead, span = edit_total((edits.get(kind) or (0, 0, []))[2], movie_timescale)
            if Fraction(trak["tkhd"], movie_timescale) != lead + span:
                found.append((f"{kind}:duration", f"{kind}: track header {trak['tkhd']} ticks, "
                              f"edits {float(lead + span)} s"))
            if kind == "video" and (span != end - video_start or lead != video_start - start):
                found.append(("video:edit", f"video edit {float(lead)} s empty and {float(span)} s, "
                              f"expected {float(video_start - start)} s and {float(end - video_start)} s"))
            if kind == "audio" and raw_audio is not None:
                # The decoded audio with edit lists ignored: from the
                # segment's second frame on it is the source's run, its
                # first following a decoder restart (KD-S-AUDIO-RESTART).
                # Here, not across the split, so that the entry reads
                # this segment's evidence.
                out_raw = cc.pcm(ffmpeg, output, out_index, int(out.get("channels") or 1), raw=True)
                evidence["audio_output_pcm"] = out_raw
                problem = decoded_run(raw_audio[0], out_raw, raw_audio[1].get(n, 0), timescale)
                if problem:
                    found.append(("audio:pcm", problem))
            if kind == "audio" and src_pcm is not None and second_view:
                # The second view of common decision 3, reported: the
                # segment's audio as ffmpeg's own edit model presents it,
                # against the source's. The gating comparison is made
                # across the whole split, with edit lists ignored.
                out_pcm = cc.pcm(ffmpeg, output, out_index, int(out.get("channels") or 1))
                rate = int(src.get("sample_rate") or 0)
                try:
                    match = audio_matches(src_pcm, out_pcm, start, rate, n > 0) if out_pcm and rate else None
                finally:
                    if out_pcm:
                        pathlib.Path(out_pcm[2]).unlink(missing_ok=True)
                info.append("second view, audio: " + ("agrees" if match else
                            "the decoded samples differ from the source's" if match is False else
                            "starts outside the source's decoded audio"))
            continue
        empty, edit = cc.edit_span(trak, movie_timescale)
        media_duration = Fraction(trak["mdhd"][1], timescale)
        expected_duration = media_duration if edit is None else min(edit, media_duration)
        if kind == "video" and (edit != end - video_start or empty != video_start - start):
            found.append(("video:edit", f"video edit {float(empty)} s empty and {float(edit or 0)} s, "
                          f"expected {float(video_start - start)} s and {float(end - video_start)} s"))
        try:
            base = Fraction(out["time_base"])
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
        if kind == "audio" and src_pcm is not None:
            rate, channels = int(src.get("sample_rate") or 0), int(src.get("channels") or 1)
            out_pcm = cc.pcm(ffmpeg, output, out_index, int(out.get("channels") or 1))
            evidence["audio_output_pcm"] = out_pcm
            if out_pcm is None or not rate:
                found.append(("audio:pcm", "audio did not decode"))
            elif out_pcm[1] != channels:
                found.append(("audio:pcm", f"audio: {channels} source and {out_pcm[1]} output channels"))
            else:
                match = audio_matches(src_pcm, out_pcm, start, rate, n > 0)
                if not match:
                    found.append(("audio:pcm", "audio: the decoded samples differ from the source's"
                                  if match is False else "audio: starts outside the source's decoded audio"))
    if (several.get("video") or several.get("audio")) and not last:
        over, frame = segment_overhang(oh, kept, segment, movie_timescale, start, end)
        if over > frame:
            found.append(("segment:duration", f"the segment lasts {float(over):.6f} s longer than its "
                          f"video, more than one audio frame ({float(frame):.6f} s)"))
    # What the black boxes do not read, from both files' own boxes
    # (scripts/writer_boxes.py), the segment judged as a clip.
    any_several = False
    for _, src_index, out_index in kept:
        src_edits = cc.cached((source, "edits", src_index), lambda: cc.full_edit_list(source, src_index))
        out_edits = cc.full_edit_list(output, out_index)
        any_several = any_several or any(e is not None and cc.several_edits(e[2]) for e in (src_edits, out_edits))
    found.extend(cc.writer_boxes.check_segment(entry, segment, any_several))
    cc.writer_boxes.forget(output)
    if mediainfo:
        tracks = cc.mediainfo_tracks(mediainfo, output)
        src_tracks = cc.cached((source, "mediainfo"), lambda: cc.mediainfo_tracks(mediainfo, source))
        general = [t for t in tracks if t.get("@type") == "General"]
        if [g.get("Format") for g in general] != ["MPEG-4"]:
            found.append(("mediainfo", f"MediaInfo: general {[g.get('Format') for g in general]}"))
        for kind, src_index, out_index in kept:
            mi_type = "Video" if kind == "video" else "Audio"
            mi_out = [t for t in tracks if t.get("@type") == mi_type and t.get("StreamOrder") == str(out_index)]
            mi_src = [t for t in src_tracks if t.get("@type") == mi_type and t.get("StreamOrder") == str(src_index)]
            if len(mi_out) != 1 or len(mi_src) != 1:
                found.append((f"mediainfo:{kind}", f"MediaInfo: {len(mi_out)} output and {len(mi_src)} "
                              f"source {kind} tracks"))
                continue
            for key in cc.MEDIAINFO_VIDEO if kind == "video" else cc.MEDIAINFO_AUDIO:
                if mi_out[0].get(key) != mi_src[0].get(key):
                    found.append((f"mediainfo:{kind}:{key}", f"MediaInfo {kind} {key}: source "
                                  f"{mi_src[0].get(key)!r}, output {mi_out[0].get(key)!r}"))
            trak = oh["traks"][out_index]
            try:
                duration = float(mi_out[0]["Duration"])
                if abs(duration - trak["tkhd"] / movie_timescale) > 0.0015:
                    found.append((f"mediainfo:{kind}:Duration", f"MediaInfo {kind} duration {duration}, "
                                  f"track header {trak['tkhd'] / movie_timescale:.6f}"))
            except (KeyError, TypeError, ValueError):
                info.append(f"MediaInfo {kind}: no duration")
    return found, info, evidence, probe, oh


# Several edits (docs/design/edit-list-writers.md, common decision 3 and the
# split section): the edit arithmetic is done here, from ISO/IEC 14496-12, on
# each file's own boxes; ffmpeg supplies the samples and the frames with edit
# lists ignored. The split's promise becomes: the frames the segments' edits
# present, each shifted by its segment's start, are the frames the source's
# edits present, every one exactly once.


# The second view is reported, not gating, and costs a decode and a
# comparison against the whole source per segment: a split of a long file
# has hundreds of segments. It is reported for a sample of them, enough to
# show a misreading of the edit model that CoreAV and the harness share:
# the first segment, the last, and every segment whose own edit list has
# several edits, at most this many in segment order.
SECOND_VIEW_SEGMENTS = 8


def second_view_of(count, edits):
    """The indices of the segments whose second view is reported: the
    first, the last, and those whose own edit list has several edits."""
    wanted = {0, count - 1}
    for _kind, (_src, out) in edits.items():
        wanted |= {n for n, e in enumerate(out) if e is not None and cc.several_edits(e[2])}
    return set(sorted(wanted)[:SECOND_VIEW_SEGMENTS])


def several_kind(src_edits, out_edits, holders):
    """Whether a kept track's edits are compared as a list of several: the
    source's list or a segment's has several edits, and every segment that
    holds the track has a readable list. `holders` are the indices of the
    segments that hold it; the others have no list, which is ordinary (a
    segment whose range holds no audio sample) and must not turn the
    comparison off."""
    if src_edits is None or any(out_edits[n] is None for n in holders):
        return False
    return cc.several_edits(src_edits[2]) or any(cc.several_edits(out_edits[n][2]) for n in holders)


def stored_run(ffprobe, path, index):
    """A file's stored samples, in order, at their stored times."""
    return cc.stored_times(raw_packets(ffprobe, path, index))


def iso_presented_once(frames, windows):
    """`cc.iso_presented`, with a frame more than one window presents shown
    once, by the first of them. A split copies each sample into one segment
    and adds no pre-roll (docs/design/split.md), so only the segment that
    holds a sample can show it, and only the first of its showings falls in
    that segment's range (docs/design/edit-list-writers.md, split decision
    5). A window's media runs into the next one's only where the source
    pauses with its media running on."""
    order = sorted(frames, key=lambda f: f[0])
    starts = [f[0] for f in order]
    longest = max((f[1] for f in order), default=0)
    shown, seen = [], set()
    for m0, m1, at in windows:
        for index in range(bisect_left(starts, m0 - longest), bisect_left(starts, m1)):
            cts, duration, content = order[index]
            if index in seen:
                continue
            if (duration > 0 and cts < m1 and cts + duration > m0) or (duration == 0 and m0 <= cts < m1):
                seen.add(index)
                shown.append((cts - m0 + at, content))
    shown.sort(key=lambda s: s[0])
    return shown


def check_frames_several(src, src_windows, outs, end, found):
    """The frames the segments' edits present, each shifted by its
    segment's start, are the frames the source's edits present before
    `end`, in order and exactly once. `src` is (time base, frames) and
    `outs` is [(frames, windows, start in media ticks)], both decoded with
    edit lists ignored. Times are in the video's media ticks."""
    sb, sf = src
    if sb is None:
        found.append(("video:framemd5", "source framemd5 without a time base"))
        return
    frames = [(Fraction(pts), Fraction(duration), (size, h)) for pts, size, h, duration in sf]
    full = Counter(cc.iso_presented(frames, src_windows))
    once = iso_presented_once(frames, src_windows)
    # Nothing the source's edits present first may lie past the last
    # segment's end: `end` comes from CoreAV's own summary, so without this
    # the segments could drop the tail of the source and still compare
    # equal. A frame presented exactly at the end has no presented
    # duration, so it is not required.
    beyond = [t for t, _ in once if t > end]
    if beyond:
        found.append(("video:framemd5", f"the source's edits present {len(beyond)} frames after the last "
                      f"segment's end, first at {float(beyond[0])} ticks"))
    needed = Counter((t, c) for t, c in once if t < end)
    actual = Counter()
    for of, windows, start in outs:
        ob, frames = of
        if ob is None:
            found.append(("video:framemd5", "segment framemd5 without a time base"))
            return
        out_frames = [(Fraction(pts), Fraction(duration), (size, h)) for pts, size, h, duration in frames]
        actual.update((t + start, c) for t, c in cc.iso_presented(out_frames, windows))
    extra = actual - full
    if extra:
        at = min(t for t, _ in extra)
        found.append(("video:framemd5", f"the segments present {sum(extra.values())} frames the source's "
                      f"edits do not, first at {float(at)} ticks"))
    missing = needed - actual
    if missing:
        at = min(t for t, _ in missing)
        found.append(("video:framemd5", f"the segments present {sum(missing.values())} frames fewer than "
                      f"the source's edits first show ({len(needed)} needed), first at {float(at)} ticks"))


def _merged(spans):
    """Spans merged where they meet within a tick."""
    out = []
    for a, b in sorted(spans):
        if out and a <= out[-1][1] + 1:
            out[-1] = (out[-1][0], max(out[-1][1], b))
        elif b > a:
            out.append((a, b))
    return out


def audio_coverage(src_windows, samples, media_end):
    """What the source's edits show, as `(start, end, allowance)` spans of
    its presentation timeline, merged where they meet: `allowance` is how
    much of the span a split may not show.

    Split decision 5: where the source pauses or skips with a sample
    still running on, that sample is shown by the window before the
    break and by the one after it. Only the segment that holds the
    sample can show it, and only as far as its own range reaches, so the
    segments may miss its part in the window that resumes — whole, from
    the next sample on, as a hole inside the window, or the window
    entirely, depending on where the cuts land. The comparison bounds
    that part instead of placing it, and bounds it inside the span it
    belongs to, so one span's allowance cannot pay for a loss in
    another. A boundary that runs on in both media and presentation
    shows the sample once, without a break, and allows nothing.
    `samples` is `[(media time, duration)]` of the stored samples, in
    order."""
    starts = [pts for pts, _ in samples]
    spans = []
    # The sample the window before ended inside, and where that window
    # ended, in media and on the presentation timeline.
    consumed, was = None, None
    for m0, m1, at in src_windows:
        end = m1 if media_end is None else min(m1, media_end)
        if end <= m0:
            continue
        allowance = 0
        i = bisect_right(starts, m0) - 1
        if i >= 0 and starts[i] < m0 < starts[i] + samples[i][1] and consumed is not None \
                and starts[i] < consumed and was != (m0, at):
            allowance = min(starts[i] + samples[i][1], end) - m0
        spans.append((at, at + end - m0, allowance))
        j = bisect_right(starts, end) - 1
        consumed = (starts[j] + samples[j][1]
                    if j >= 0 and starts[j] < end < starts[j] + samples[j][1] else end)
        was = (end, at + end - m0)
    merged = []
    for a, b, allowance in sorted(spans):
        if merged and a <= merged[-1][1] + 1:
            last = merged[-1]
            merged[-1] = (last[0], max(last[1], b), last[2] + allowance)
        elif b > a:
            merged.append((a, b, allowance))
    return merged


def check_audio_several(src_windows, outs, found, samples=None, media_end=None):
    """The edits of a split whose audio has several, across the whole
    split. `outs` is `[(n, windows, start in audio ticks, the source
    decode time of the segment's first sample)]`. Each segment's edits
    present the media a source edit presents at the same time; no two
    segments present overlapping parts of the source's timeline; and
    together they present what the source's edits do, short only by what
    split decision 5 allows. The decoded audio is compared per segment,
    in `check_segment`, where a known divergence can read that segment's
    evidence."""
    # The windows are in order and do not overlap, so the one that could
    # hold a media interval is found by bisection: a source can have
    # thousands of them.
    starts = [w[0] for w in src_windows]
    spans = []
    for n, windows, zero, first_dts in outs:
        for m0, m1, at in windows:
            sm0, sm1, s_at = m0 + first_dts, m1 + first_dts, at + zero
            near = bisect_right(starts, sm0 + 1)
            holder = next((w for w in src_windows[max(near - 1, 0):near + 1]
                           if w[0] - 1 <= sm0 and sm1 <= w[1] + 1), None)
            if holder is None or abs(sm0 - holder[0] + holder[2] - s_at) > 1:
                found.append(("audio:edits", f"segment {n}: the edit at {float(at)} ticks presents other "
                              "media than the source's edits"))
                break
        spans.extend((at + zero, at + zero + m1 - m0, n) for m0, m1, at in windows)
    # No audio presented twice (split decision 5): no two segments present
    # overlapping parts of the source's timeline, by more than the tick the
    # edits are rounded to.
    ordered = sorted(spans)
    for (a0, a1, na), (b0, _b1, nb) in zip(ordered, ordered[1:]):
        if na != nb and b0 < a1 - 1:
            found.append(("audio:edits", f"segments {na} and {nb} both present "
                          f"{float(b0)} to {float(min(a1, _b1))} ticks"))
            break
    expected = audio_coverage(src_windows, samples or [], media_end)
    shown = _merged([(a, b) for a, b, _ in spans])
    # Nothing shown where the source's edits show nothing, and no span
    # short by more than decision 5 allows it.
    outside = [(a, b) for a, b in shown
               if not any(lo - 1 <= a and b <= hi + 1 for lo, hi, _ in expected)]
    if outside:
        found.append(("audio:edits", f"the segments present {[(float(a), float(b)) for a, b in outside][:4]} "
                      f"ticks, which the source's edits do not"))
    for lo, hi, allowance in expected:
        inside = sum(min(b, hi) - max(a, lo) for a, b in shown if a < hi and b > lo)
        short = (hi - lo) - inside
        if not -1 <= short <= allowance + 1:
            found.append(("audio:edits", f"the segments present {float(inside)} ticks of the source's "
                          f"{float(hi - lo)} from {float(lo)}, {float(short)} short where at most "
                          f"{float(allowance)} may be"))
            break


KD_FILE = pathlib.Path(__file__).resolve().parent.parent / "docs" / "known-divergences-split.json"


def audio_restart(field, fields, c):
    """KD-S-AUDIO-RESTART."""
    if field != "audio:pcm" or not c["entry"].get("audio") or not c.get("audio_output_pcm"):
        return False
    oh = c["output_headers"]
    trak = oh["traks"][1] if oh and len(oh["traks"]) > 1 else None
    if not trak:
        return False
    media = [t for d, t in trak.get("elst") or [] if t != -1]
    if trak.get("elst") and len(media) != 1:
        return False
    # The decode below compares the segment with its own kept packets, at
    # the segment's own times; those times must be the source's: the
    # segment's decode durations exactly what the source's `stts` stores for
    # the kept samples.
    kept = c["entry"]["audio"]
    source_traks = (c.get("source_headers") or {}).get("traks") or []
    index = kept.get("stream_index")
    if index is None or not 0 <= index < len(source_traks):
        return False
    first, count = kept.get("first_sample", -1), kept.get("samples", 0)
    window = cc.stts_window(cc.stored_durations(c["entry"]["source"], c.get("source_headers"), index, first + count),
                            first, count)
    if kept.get("first_sample", -1) < 0 or not window or cc.merged_stts(trak.get("stts")) != window:
        return False
    decoded = cc.kept_audio_decoded(c)
    if decoded is None:
        return False
    try:
        return cc._restart_matches(c, trak, media or [0], decoded)
    finally:
        pathlib.Path(decoded).unlink(missing_ok=True)


def _source_edit_midframe(c):
    """The source video's one media edit starts inside a frame: the frame
    duration, else None."""
    sh = c.get("source_headers")
    stream = c.get("video_stream")
    if not sh or stream is None or len(sh["traks"]) <= stream:
        return None
    media = [t for _, t in sh["traks"][stream].get("elst") or [] if t != -1]
    durations = sorted(p["duration"] for p in c.get("video_source_packets") or []
                       if isinstance(p.get("duration"), int) and p["duration"] > 0)
    if len(media) != 1 or not durations:
        return None
    frame = durations[len(durations) // 2]
    return frame if media[0] % frame else None


def edit_midframe(field, fields, c):
    """KD-S-EDIT-MIDFRAME."""
    frame = _source_edit_midframe(c)
    if frame is None or field not in ("video:packets", "video:framemd5"):
        return False
    if field == "video:framemd5":
        offsets = c.get("video_frame_offsets")
        src, outs, end = c["video_framemd5"]
        return bool(offsets) and check_video_frames(src, outs, [], offsets, end)
    timing = c.get("video_timing")
    if not timing or 0 in timing:
        return False
    deltas = set()
    for entry in timing.values():
        deltas |= entry["pts"] | entry["dts"]
    if len(deltas) != 1:
        return False
    delta = deltas.pop()
    return delta != 0 and abs(delta) < frame


def dts_shift(field, fields, c):
    """KD-S-DTS-SHIFT, alone or with KD-S-EDIT-MIDFRAME's constant."""
    timing = c.get("video_timing")
    if field != "video:packets" or not timing:
        return False
    negative = c.get("video_ctts_negative") or {}
    minimum = c.get("video_ctts_min") or {}
    traks = (c.get("source_headers") or {}).get("traks") or []
    stream = c.get("video_stream")
    if stream is None or not 0 <= stream < len(traks) or traks[stream].get("ctts_min") is None:
        return False
    # Presentation times agree, or, when the source's edit starts inside a
    # frame, every segment after the first differs by KD-S-EDIT-MIDFRAME's
    # one constant, which then moves the decode times too.
    presented = set().union(*(entry["pts"] for entry in timing.values()))
    if len(presented) != 1:
        return False
    moved = presented.pop()
    if moved:
        frame = _source_edit_midframe(c)
        if frame is None or abs(moved) >= frame or set(timing) != set(minimum) - {0}:
            return False
    shifted = False
    for n, entry in timing.items():
        if len(entry["dts"]) != 1 or minimum.get(n) is None:
            return False
        # The constant is the cause's: the source's smallest composition
        # offset less the segment's, from both files' `ctts`.
        constant = min(traks[stream]["ctts_min"], 0) - min(minimum[n], 0)
        if entry["dts"] != {moved + constant} or (constant and not negative.get(n)):
            return False
        shifted = shifted or constant != 0
    return shifted


def pts_shift(field, fields, c):
    """KD-S-PTS-SHIFT."""
    if field not in ("video:packets", "video:framemd5"):
        return False
    sh = c.get("source_headers")
    stream = c.get("video_stream")
    if not sh or stream is None or len(sh["traks"]) <= stream or sh["traks"][stream].get("elst"):
        return False
    # The segments hold every sample, so their smallest offset is the
    # source's.
    smallest = min((m for m in c.get("video_ctts_min", {}).values() if m is not None), default=None)
    if smallest is None or smallest >= 0:
        return False
    shift = -smallest
    if field == "video:framemd5":
        src, outs, end = c["video_framemd5"]
        sb = src[0]
        if sb is None:
            return False
        # The offset in the source's ticks (the video's media ticks).
        offsets = {n: shift for n in range(len(outs))}
        return check_video_frames(src, outs, [], offsets, None if end is None else end + shift * sb)
    timing = c.get("video_timing")
    if not timing:
        return False
    return all(entry["pts"] == {shift} and entry["dts"] == {shift} for entry in timing.values())


def sync_verified(field, fields, c):
    """KD-S-SYNC-VERIFIED."""
    segments, src = c.get("video_segments"), c.get("video_source_packets")
    if field != "video:packets" or not segments or not src or not c.get("ffprobe"):
        return False
    at = dropped = 0
    for (path, index), (packets, shift) in segments:
        n = cc.dropped_sync_flags(src[at:at + len(packets)], packets, shift)
        at += len(packets)
        if n is None or not cc.flags_decoded_keys(c["ffprobe"], path, index, packets):
            return False
        dropped += n
    return dropped > 0 and at == len(src)


PREDICATES = {
    "audio_restart": audio_restart,
    "edit_midframe": edit_midframe,
    "dts_shift": dts_shift,
    "pts_shift": pts_shift,
    "sync_verified": sync_verified,
}


def known_divergences():
    entries = json.loads(KD_FILE.read_text(encoding="utf-8"))["entries"]
    return [(e["id"], e["field_pattern"], PREDICATES[e["predicate"]]) for e in entries]


def _covers(predicate, field, fields, evidence):
    """Whether a known divergence's predicate accepts, treating evidence
    it cannot read as not covering."""
    try:
        return bool(predicate(field, fields, evidence))
    except (KeyError, TypeError, ValueError, IndexError, RuntimeError):
        return False


def covering(field, fields, evidence):
    """The id of the known divergence that covers `field` on a segment: the
    split's own entries, then the clip's (docs/known-divergences-split.md).
    None when no entry covers it, and for a box check's field
    (docs/design/writer-oracle-boxes.md)."""
    if field.startswith("box:"):
        return None
    for kd_id, pattern, predicate in known_divergences() + cc.known_divergences():
        if not fnmatch.fnmatch(field, pattern):
            continue
        try:
            if predicate(field, fields, evidence):
                return kd_id
        except (KeyError, TypeError, ValueError, IndexError, RuntimeError):
            continue
    return None


def check(entry, ffprobe, ffmpeg, mediainfo, decode=True):
    """Returns the problems, the informational differences, the covered
    differences and the packet counts of one split. Without `decode`, the
    decoded video and audio are not compared."""
    source = entry["source"]
    segments = entry["segments"]
    video_stream, audio_stream = entry["video_stream"], entry.get("audio_stream")
    problems, info, covered = [], [], []
    counts = {"agree": 0, "disagree": 0}
    src_probe = cc.cached((source, "probe"), lambda: cc.ffprobe_json(ffprobe, "-show_format", "-show_streams",
                                                                      source))
    src_streams = {s.get("index"): s for s in src_probe["streams"]}
    # The audio the split should keep, found from the source itself: its
    # first audio stream, when it is AAC and the split was not made without
    # audio. A split that left it out of every segment fails.
    first_audio = next((st for st in src_probe["streams"] if st.get("codec_type") == "audio"), None)
    if (audio_stream is None and not entry.get("no_audio") and first_audio
            and first_audio.get("codec_name") == "aac"):
        problems.append(f"the source's AAC audio (stream {first_audio.get('index')}) is in no segment")
    src_pcm = None
    audio_channels = int((src_streams.get(audio_stream) or {}).get("channels") or 1)
    if decode and audio_stream is not None:
        src = src_streams.get(audio_stream) or {}
        if int(src.get("sample_rate") or 0):
            src_pcm = cc.cached((source, "pcm", audio_stream),
                                lambda: cc.pcm(ffmpeg, source, audio_stream, audio_channels))
    # Several edits on the source or on any segment: the edit arithmetic of
    # common decision 3 (docs/design/edit-list-writers.md).
    edits = {}
    several = {}
    for kind, src_index in [("video", video_stream)] + ([("audio", audio_stream)] if audio_stream is not None
                                                         else []):
        src_edits = cc.cached((source, "edits", src_index), lambda: cc.full_edit_list(source, src_index))
        out_edits, holders = [], []
        for n, segment in enumerate(segments):
            kept = {k: o for k, _, o in segment_streams(segment, video_stream, audio_stream)}
            # None both for a segment without this kind (an ordinary case:
            # a segment whose range holds no audio sample) and for one
            # whose boxes cannot be read. Only the second keeps the
            # several-edit comparison from running.
            if kind in kept:
                holders.append(n)
            out_edits.append(cc.full_edit_list(segment["output"], kept[kind]) if kind in kept else None)
        edits[kind] = (src_edits, out_edits)
        several[kind] = several_kind(src_edits, out_edits, holders)
    reported = second_view_of(len(segments), edits)
    # What the per-segment decoded comparison of several-edit audio needs:
    # the source decoded with edit lists ignored, and the source decode
    # time each segment's audio run starts at.
    raw_audio = None
    if decode and audio_stream is not None and several["audio"]:
        src_run = cc.cached((source, "run", audio_stream), lambda: stored_run(ffprobe, source, audio_stream))
        at_dts = {}
        for n, segment in enumerate(segments):
            first = segment.get("audio_first")
            dts = src_run[first].get("dts") if isinstance(first, int) and first < len(src_run) else None
            at_dts[n] = dts if isinstance(dts, int) else 0
        raw_audio = (cc.cached((source, "raw pcm", audio_stream),
                               lambda: cc.pcm(ffmpeg, source, audio_stream, audio_channels, raw=True)),
                     at_dts)
    probes = []
    for n, segment in enumerate(segments):
        found, seg_info, evidence, probe, oh = check_segment(
            n, segment, entry, src_streams, ffprobe, ffmpeg, mediainfo, src_pcm, several=several,
            edits={kind: out[n] for kind, (_, out) in edits.items()},
            second_view=n in reported, last=n == len(segments) - 1, raw_audio=raw_audio)
        probes.append(probe)
        info.extend(f"segment {n}: {m}" for m in seg_info)
        fields = {field for field, _ in found}
        try:
            for field, message in found:
                kd = covering(field, fields, evidence)
                if kd:
                    covered.append({"kd": kd, "problem": f"segment {n}: {message}"})
                else:
                    problems.append(f"segment {n}: {message}")
        finally:
            if evidence.get("audio_output_pcm"):
                pathlib.Path(evidence["audio_output_pcm"][2]).unlink(missing_ok=True)
        if any(f == "streams" for f, _ in found):
            return problems, info, covered, counts

    # Coverage: every source packet in exactly one segment, in order.
    found = []
    evidence = {"source_headers": cc.cached((source, "headers"), lambda: cc.headers(source)),
                "video_stream": video_stream, "ffprobe": ffprobe,
                "video_ctts_negative": {n: bool((cc.headers(s["output"]) or {"traks": [{}]})["traks"][0]
                                                .get("ctts_reads_negative")) for n, s in enumerate(segments)},
                "video_ctts_min": {n: (cc.headers(s["output"]) or {"traks": [{}]})["traks"][0].get("ctts_min")
                                   for n, s in enumerate(segments)}}
    moved = {}
    first_dts = {}
    for kind, src_index in [("video", video_stream)] + ([("audio", audio_stream)] if audio_stream is not None
                                                         else []):
        raw = several[kind]
        if raw:
            # With several edits the samples are paired at their stored
            # times, each segment's run starting at its first source
            # decode time (common decision 3); the edits are compared
            # through the frames below.
            src_packets = cc.cached((source, "run", src_index), lambda: stored_run(ffprobe, source, src_index))
        else:
            src_packets = cc.cached((source, "stored packets", src_index),
                                    lambda: segment_packets(ffprobe, source, src_index))
        evidence[f"{kind}_source_packets"] = src_packets
        outs, places = [], []
        at = 0
        for n, segment in enumerate(segments):
            kept = {k: o for k, _, o in segment_streams(segment, video_stream, audio_stream)}
            if kind not in kept:
                continue
            if raw:
                run = stored_run(ffprobe, segment["output"], kept[kind])
                dts = src_packets[at].get("dts") if at < len(src_packets) else None
                first_dts.setdefault(kind, {})[n] = dts if isinstance(dts, int) else 0
                at += len(run)
                outs.append((run, Fraction(first_dts[kind][n])))
            else:
                base = Fraction(probes[n]["streams"][kept[kind]]["time_base"])
                outs.append((segment_packets(ffprobe, segment["output"], kept[kind]),
                             cc.fraction(segment["start"]) / base))
            places.append((segment["output"], kept[kind]))
        evidence[f"{kind}_segments"] = list(zip(places, outs))
        agree, disagree = compare_coverage(kind, src_packets, outs, found, evidence)
        moved[kind] = evidence.get(f"{kind}_alike_count", 0)
        counts["agree"] += agree
        counts["disagree"] += disagree

    # Decoded video: the source's frames, each in exactly one segment.
    end = cc.fraction(segments[-1]["end"])
    if decode and several["video"]:
        src_edits, out_edits = edits["video"]
        timescale = src_edits[1]
        src_frames = cc.cached((source, "raw framemd5", video_stream),
                               lambda: cc.framemd5(ffmpeg, source, video_stream, video=True, raw=True))
        outs = [(cc.framemd5(ffmpeg, s["output"], 0, video=True, raw=True), cc.iso_windows(*e),
                 cc.fraction(s["start"]) * timescale)
                for s, e in zip(segments, out_edits)]
        check_frames_several(src_frames, cc.iso_windows(*src_edits), outs, end * timescale, found)
        # The second view (reported): ffmpeg's edit-applied frames of each
        # segment against the source's from the segment's start.
        src_applied = cc.cached((source, "framemd5", video_stream),
                                lambda: cc.framemd5(ffmpeg, source, video_stream, video=True))
        src_raw = cc.cached((source, "run", video_stream), lambda: stored_run(ffprobe, source, video_stream))
        src_model = cc.cached((source, "model", video_stream),
                              lambda: cc.ffmpeg_model(src_raw, src_frames, src_edits))
        for n, (segment, e) in enumerate(zip(segments, out_edits)):
            if n not in reported:
                continue
            second = []
            cc.second_view_video(src_applied, cc.framemd5(ffmpeg, segment["output"], 0, video=True), src_model,
                                 cc.ffmpeg_model(stored_run(ffprobe, segment["output"], 0),
                                                 outs[n][0], e),
                                 cc.fraction(segment["start"]), cc.fraction(segment["end"]), second)
            info.append(f"segment {n}: second view, video: "
                        + ("agrees" if not second else "; ".join(m for _, m in second)))
        info.append(f"second view, video: reported for {len(reported)} of {len(segments)} segments")
    elif decode:
        src_frames = cc.cached((source, "framemd5", video_stream),
                               lambda: cc.framemd5(ffmpeg, source, video_stream, video=True))
        outs = [(cc.framemd5(ffmpeg, s["output"], 0, video=True), cc.fraction(s["start"])) for s in segments]
        evidence["video_framemd5"] = (src_frames, outs, end)
        timing = evidence.get("video_timing") or {}
        deltas = {d for entry in timing.values() for d in entry["pts"]}
        if len(deltas) == 1:
            delta = deltas.pop()
            evidence["video_frame_offsets"] = {n: delta for n in range(1, len(segments))}
        check_video_frames(src_frames, outs, found, end=end)

    # The edits of audio with several, across the split: the decoded
    # audio is compared per segment, in `check_segment`.
    if audio_stream is not None and several["audio"]:
        src_edits, out_edits = edits["audio"]
        timescale = src_edits[1]
        src_run = cc.cached((source, "run", audio_stream), lambda: stored_run(ffprobe, source, audio_stream))
        media_end = max((p["pts"] + p.get("duration", 0) for p in src_run
                         if isinstance(p.get("pts"), int)), default=None)
        outs = []
        for n, (segment, e) in enumerate(zip(segments, out_edits)):
            kept = {k: o for k, _, o in segment_streams(segment, video_stream, audio_stream)}
            if "audio" not in kept or e is None:
                continue
            outs.append((n, cc.iso_windows(*e), cc.fraction(segment["start"]) * timescale,
                         first_dts.get("audio", {}).get(n, 0)))
        samples = [(p["pts"], p.get("duration", 0)) for p in src_run if isinstance(p.get("pts"), int)]
        check_audio_several(cc.iso_windows(*src_edits), outs, found, samples=samples, media_end=media_end)
    fields = {field for field, _ in found}
    for field, message in found:
        # `covering` is not used here: only the split's own entries apply
        # to the coverage of the packets and the frames. A predicate that
        # cannot read the evidence this comparison collects does not
        # cover the difference, as in `covering`.
        kd = next((kd_id for kd_id, pattern, predicate in known_divergences()
                   if fnmatch.fnmatch(field, pattern) and _covers(predicate, field, fields, evidence)), None)
        if kd:
            covered.append({"kd": kd, "problem": message})
            # Packets that differ in times or keyframe flags only count as
            # agreeing once covered (every entry for packets requires that
            # of each differing packet); missing and extra packets never do.
            if field.endswith(":packets"):
                kind = field.split(":")[0]
                counts["agree"] += moved.get(kind, 0)
                counts["disagree"] -= moved.get(kind, 0)
        else:
            problems.append(message)
    return problems, info, covered, counts


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--mediainfo")
    parser.add_argument("--report")
    parser.add_argument("--no-decode", action="store_true", help="skip the decoded video and audio checks")
    args = parser.parse_args()

    with open(args.manifest, encoding="utf-8") as f:
        manifest = json.load(f)
    results = []
    previous = None
    for entry in manifest:
        if entry["source"] != previous:
            cc.clear_cache()
            previous = entry["source"]
        try:
            problems, info, covered, counts = check(entry, args.ffprobe, args.ffmpeg, args.mediainfo,
                                                    decode=not args.no_decode)
        except (RuntimeError, ValueError, KeyError) as error:
            problems, info, covered, counts = [f"harness: {error}"], [], [], {"agree": 0, "disagree": 0}
        results.append({**entry, "problems": problems, "informational": info, "covered": covered,
                        "packets": counts})
        name = f"{entry['source'].rsplit('/', 1)[-1]} {entry['label']} ({len(entry['segments'])} segments)"
        print(f"{'FAIL' if problems else 'kd  ' if covered else 'ok  '} {name}")
        for problem in problems:
            print(f"     {problem}")
        for item in covered:
            print(f"     ({item['kd']}) {item['problem']}")
        for difference in info:
            print(f"     (informational) {difference}")
    cc.clear_cache()
    passed = sum(1 for r in results if not r["problems"])
    direct = sum(1 for r in results if not r["problems"] and not r["covered"])
    agree = sum(r["packets"]["agree"] for r in results)
    disagree = sum(r["packets"]["disagree"] for r in results)
    print(f"{passed}/{len(results)} splits pass every oracle ({direct} directly, {passed - direct} through "
          f"known divergences); packets {agree}/{agree + disagree} agree")
    if args.report:
        with open(args.report, "w", encoding="utf-8") as f:
            json.dump({"passed": passed, "direct": direct, "total": len(results),
                       "packets": {"agree": agree, "disagree": disagree}, "results": results}, f, indent=2)
    return 0 if results and passed == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
