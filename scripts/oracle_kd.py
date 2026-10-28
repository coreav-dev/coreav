#!/usr/bin/env python3
"""Apply documented known divergences to a coreav-oracle comparison report.

Decision 3 (oracle and correctness policy) and proposal 6.2: a mismatch counts
as agreement only when a documented entry's predicate covers it; covered items
stay in the denominator and are reported apart from direct agreement. The
entries live in docs/known-divergences.md with the twin
docs/known-divergences.json. Captures are read, never rewritten.

Packets are counted in the reference's packet unit: a constant-size PCM
stream, which the candidate lists one sample per packet, counts one unit per
`ffprobe` packet (docs/design/stage3-report.md, "Packet unit"). The
comparison's own per-sample count is reported beside it, under `per_sample`,
and gates nothing.

Usage: oracle_kd.py --report R --reference DIR --candidate DIR --media DIR --out OUT
Exit status: 0 when every in-scope (MP4/MOV) item agrees directly or via a
documented divergence, 1 otherwise, 2 for usage or input errors.
"""
import argparse
import bisect
import collections
import functools
import io
import json
import pathlib
import subprocess
import sys
import zlib
from fractions import Fraction

ROOT = pathlib.Path(__file__).resolve().parent.parent
KD_FILE = ROOT / 'docs' / 'known-divergences.json'
IN_SCOPE = ('.mp4', '.mov', '.m4a')


def micros(value):
    """Round seconds to integer microseconds, halves away from zero."""
    scaled = Fraction(value) * 1_000_000
    sign = -1 if scaled < 0 else 1
    scaled = abs(scaled)
    return sign * int((scaled * 2 + 1) // 2)


def round_half_up(value):
    return int((Fraction(value) * 2 + 1) // 2)


def load(directory, name, level):
    path = pathlib.Path(directory) / f'{name}.{level}.json'
    return json.loads(path.read_text()) if path.exists() else {}


def props(item):
    return item.get('properties') or {}


def fraction(pair):
    return None if pair is None else Fraction(pair[0], pair[1])


# ---------------------------------------------------------------- predicates

# Box header layout and the parent of each container below are those of
# ISO/IEC 14496-12 (section 4.2 and each box's "Container" line); the depth
# limit is the default of Decision 4 (resource limits).
CONTAINERS = {b'moov', b'trak', b'mdia', b'minf', b'stbl', b'edts', b'dinf', b'mvex', b'moof', b'traf'}
PARENTS = {b'trak': b'moov', b'mdia': b'trak', b'minf': b'mdia', b'stbl': b'minf', b'edts': b'trak',
           b'mvex': b'moov', b'traf': b'moof'}
MAX_DEPTH = 32
MAX_MEDIA_BYTES = 256 << 20


def structure_defects(data, max_boxes=1_000_000):
    """Independent evidence that a file is malformed, read from its bytes alone:
    'truncated' (a box runs past the end of the file), 'overrun' (past the end
    of its parent or shorter than its header), 'misplaced' (a container inside
    the wrong parent), 'deep' (nesting beyond the default depth limit), and
    the movie box defects of `movie_defects`."""
    found, boxes = movie_defects(data), 0
    stack = [(0, len(data), None, 0)]
    while stack:
        offset, end, parent, depth = stack.pop()
        while offset < end:
            boxes += 1
            if boxes > max_boxes:
                return found
            past = 'truncated' if parent is None else 'overrun'
            if end - offset < 8:
                found.add(past)
                break
            size, kind, header = int.from_bytes(data[offset:offset + 4], 'big'), data[offset + 4:offset + 8], 8
            if size == 1:
                if end - offset < 16:
                    found.add(past)
                    break
                size, header = int.from_bytes(data[offset + 8:offset + 16], 'big'), 16
            elif size == 0 and parent is None:
                size = end - offset
            if size < header:
                found.add('overrun')
                break
            if offset + size > end:
                found.add(past)
                break
            if kind in PARENTS and PARENTS[kind] != parent:
                found.add('misplaced')
            if kind in CONTAINERS:
                if depth + 1 > MAX_DEPTH:
                    found.add('deep')
                else:
                    stack.append((offset + header, offset + size, kind, depth + 1))
            offset += size
    return found


def _top_level(data, start, end):
    """(type, payload start, end) of the boxes in [start, end), stopping at
    one that does not fit (ISO/IEC 14496-12 4.2)."""
    while end - start >= 8:
        size, kind, header = int.from_bytes(data[start:start + 4], 'big'), data[start + 4:start + 8], 8
        if size == 1:
            if end - start < 16:
                return
            size, header = int.from_bytes(data[start + 8:start + 16], 'big'), 16
        elif size == 0:
            size = end - start
        if size < header or start + size > end:
            return
        yield kind, start + header, start + size
        start += size


def movie_defects(data):
    """'second-moov': more than one top-level movie box (ISO/IEC 14496-12
    8.2.1: exactly one). 'no-mvhd': a top-level movie box without a movie
    header among its children (8.2.2: exactly one, mandatory), unless it
    holds a QuickTime compressed movie (`cmov`) or reference movie (`rmra`),
    which the QuickTime File Format defines without one: CoreAV rejecting
    those is a missing feature, never covered."""
    found = set()
    movies = [(a, e) for kind, a, e in _top_level(data, 0, len(data)) if kind == b'moov']
    if len(movies) > 1:
        found.add('second-moov')
    for start, end in movies:
        children = [kind for kind, _, _ in _top_level(data, start, end)]
        if b'mvhd' not in children and b'cmov' not in children and b'rmra' not in children:
            found.add('no-mvhd')
    return found


EVIDENCE = {
    'truncated_structure': {'truncated'},
    'invalid_structure': {'overrun', 'misplaced', 'second-moov', 'no-mvhd'},
    'limit': {'deep'},
}


def strict_rejection(reference, candidate, defects):
    """A rejection is agreement only when the media bytes independently show
    the kind of defect claimed; a valid file rejected by a regression is not."""
    kind = props(candidate).get('rejection')
    return (reference.get('probe_ok') is True and candidate.get('probe_ok') is False
            and bool(EVIDENCE.get(kind, set()) & defects))


def media_defects(media_dir, name):
    path = pathlib.Path(media_dir) / name
    with open(path, 'rb') as handle:
        data = handle.read(MAX_MEDIA_BYTES + 1)
    if len(data) > MAX_MEDIA_BYTES:
        return set()
    return structure_defects(data)


def _span_us(starts, durations, kinds=None):
    """The latest end minus the earliest start in microseconds, each stream's
    start and duration rounded to microseconds on their own, as the
    reference counts its format duration: with `kinds`, the audio and video
    streams, and each other stream that ends less than a second after them
    (every stream without audio or video; docs/design/probe-duration.md)."""
    begins = [micros(s) for s in starts]
    ends = [b + micros(d) for b, d in zip(begins, durations)]
    kinds = kinds or [None] * len(begins)
    av = [k in ('video', 'audio') for k in kinds]
    if any(av):
        av_end = max(e for e, a in zip(ends, av) if a)
        keep = [a or e < av_end + 1_000_000 for e, a in zip(ends, av)]
    else:
        keep = [True] * len(begins)
    return max(e for e, k in zip(ends, keep) if k) - min(b for b, k in zip(begins, keep) if k)


def _with_dropped(candidate):
    """The candidate with each stream the allowlist dropped standing in as a
    stream of its own, from the timing the adapter reports for it (its
    `duration_ticks` and fragment start), so the format duration clauses can
    count it as the reference counts the stream."""
    dropped = (candidate.get('properties') or {}).get('dropped') or []
    present = {s.get('index') for s in candidate.get('streams', [])}
    extra = []
    for d in dropped:
        if d.get('stream_index') in present or not isinstance(d.get('duration_ticks'), int) or not d.get('timescale'):
            continue
        extra.append({'index': d['stream_index'], 'kind': d.get('kind'), 'timescale': d['timescale'],
                      'time_base': [1, d['timescale']], 'duration_ts': d['duration_ticks'],
                      'properties': {'track_id': d.get('track_id'), 'edits': d.get('edits'), 'dropped': True,
                                     'fragment_start_ticks': d.get('fragment_start_ticks'),
                                     'sidx_end_ticks': d.get('sidx_end_ticks')}})
    if not extra:
        return candidate
    streams = sorted(candidate.get('streams', []) + extra, key=lambda s: s.get('index', 0))
    return {**candidate, 'streams': streams}


def _starts_and_durations(reference, candidate):
    """Each stream's start (its empty edit rounded to stream ticks) and its
    duration on each side, in seconds; None when a stream does not pair.
    Both sides share the starts: in a fragmented movie whose streams all
    start at their first `tfdt`, that common offset leaves each span the
    same, and the format clauses require each side's span to equal its
    format duration exactly."""
    rstreams, cstreams = reference.get('streams', []), paired(reference, candidate)
    if len(candidate.get('streams', [])) != len(rstreams) or not rstreams or not all(cstreams):
        return None
    starts, rdur, cdur = [], [], []
    for r, c in zip(rstreams, cstreams):
        scale, edits = c.get('timescale'), props(c).get('edits')
        if (not scale or r.get('timescale') != scale or r.get('duration_ts') is None
                or c.get('duration_ts') is None or edits == 'unsupported'):
            return None
        empty = edits.get('empty', 0) if isinstance(edits, dict) else 0
        movie_scale = props(candidate).get('movie_timescale')
        starts.append(Fraction(round_half_up(Fraction(empty * scale, movie_scale)), scale))
        rdur.append(Fraction(r['duration_ts'], scale))
        cdur.append(Fraction(c['duration_ts'], scale))
    return starts, rdur, cdur


def stts_short_stream(reference, candidate, index):
    """`index` is the reference position; the candidate stream is the one
    paired with it (by index, as the comparison pairs them)."""
    try:
        r, c = reference['streams'][index], paired(reference, candidate)[index]
    except (IndexError, KeyError):
        return False
    if not c:
        return False
    p = props(c)
    mdhd, stts = p.get('mdhd_duration_ticks'), p.get('stts_duration_ticks')
    if not mdhd or stts is None or not any(w.startswith('duration conflict') for w in p.get('warnings', [])):
        return False
    rule = Fraction(mdhd)
    edit = fraction(p.get('media_edit_duration_ticks'))
    if edit is not None and edit < rule:
        rule = edit
    return (r.get('duration_ts') == stts and stts < c.get('duration_ts', 0)
            and c.get('duration_ts') == round_half_up(rule))


def edit_duration_stream(reference, candidate, index):
    """KD-EDIT-DURATION: CoreAV reports the Decision 13 duration (the media
    edit, capped by the media header; the media edits added up for a list
    of several, docs/design/edit-lists.md decision 5); the reference
    reports its own rule, the smallest of the media header, the `stts` sum
    and every edit's duration added up, empty edits included (for a list
    of several, each edit rounded to whole ticks first, rule 1 of the
    design note)."""
    try:
        r, c = reference['streams'][index], paired(reference, candidate)[index]
    except (IndexError, KeyError):
        return False
    if not c:
        return False
    p = props(c)
    mdhd, stts, edits = p.get('mdhd_duration_ticks'), p.get('stts_duration_ticks'), p.get('edits')
    scale, movie_scale = c.get('timescale'), props(candidate).get('movie_timescale')
    edit = fraction(p.get('media_edit_duration_ticks'))
    if edits == 'multiple' and mdhd and stts is not None and edit is not None and scale and movie_scale:
        entries = p.get('edit_entries')
        if not isinstance(entries, list) or not entries or stts < mdhd:
            return False
        theirs = min(mdhd, stts, sum((2 * e[0] * scale + movie_scale) // (2 * movie_scale) for e in entries))
        # Decision 5 recomputed from the entries: the media edits added up.
        media = Fraction(sum(e[0] for e in entries if e[1] != -1) * scale, movie_scale)
        ours = round_half_up(min(Fraction(mdhd), media))
        return c.get('duration_ts') == ours and r.get('duration_ts') == theirs and theirs != ours
    if not mdhd or stts is None or not isinstance(edits, dict) or edit is None or not scale or not movie_scale:
        return False
    # The edit list must be what makes the two differ: the media edit is
    # shorter than the media or follows an empty edit, and the `stts` sum is
    # not shorter than CoreAV's value (a shorter one is KD-STTS-SHORT's
    # case, with its warning).
    decision13 = min(Fraction(mdhd), edit)
    if (edit >= mdhd and edits['empty'] == 0) or decision13 > stts:
        return False
    edited = round_half_up(Fraction((edits['empty'] + edits['segment']) * scale, movie_scale))
    theirs = min(mdhd, stts, edited)
    return (c.get('duration_ts') == round_half_up(decision13) and r.get('duration_ts') == theirs
            and theirs != c.get('duration_ts'))


def frag_start_stream(reference, candidate, index):
    """KD-FRAG-START: a fragmented stream whose first fragment starts at a
    decode time t after 0 (`tfdt`): CoreAV's duration is its samples' span,
    the reference counts from decode time 0, so its duration is t more; with
    a segment index (`sidx`) for the track, the index's end."""
    try:
        r, c = reference['streams'][index], paired(reference, candidate)[index]
    except (IndexError, KeyError):
        return False
    if not c:
        return False
    p = props(c)
    start, sidx_end, duration = p.get('fragment_start_ticks'), p.get('sidx_end_ticks'), c.get('duration_ts')
    if not isinstance(start, int) or start <= 0 or not isinstance(duration, int):
        return False
    # The reference counts from decode time 0: the first `tfdt` more than
    # CoreAV's span of the samples, or, with a segment index, the index's
    # end (its earliest presentation time plus its subsegment durations),
    # which lies at or after that.
    if isinstance(sidx_end, int):
        # A kept stream's value is its samples' summed durations; a dropped
        # stream has no sample sum to check, only the bound.
        if not p.get('dropped') and duration != p.get('stts_duration_ticks'):
            return False
        return r.get('duration_ts') == sidx_end and sidx_end >= duration + start
    return r.get('duration_ts') == duration + start


def duration_format(reference, candidate):
    """The known divergences a format duration follows from: every stream
    agrees or is covered by KD-STTS-SHORT, KD-EDIT-DURATION, KD-FRAG-START or
    KD-PCM-END, and each side's format duration is its own streams' span.
    None when it does not follow."""
    ids = set()
    # A stream the allowlist dropped counts with the timing CoreAV gives it.
    candidate = _with_dropped(candidate)
    cstreams = paired(reference, candidate)
    if len(candidate.get('streams', [])) != len(cstreams) or not all(cstreams):
        return None
    spans = _starts_and_durations(reference, candidate)
    if spans is None:
        return None
    starts, rdur, cdur = spans
    # Each side's span with its own stream kinds.
    kinds = [r.get('kind') for r in reference.get('streams', [])]
    ckinds = [c.get('kind') for c in cstreams]
    unmatched = []
    for i in range(len(reference.get('streams', []))):
        if stts_short_stream(reference, candidate, i):
            ids.add('KD-STTS-SHORT')
        elif edit_duration_stream(reference, candidate, i):
            ids.add('KD-EDIT-DURATION')
        elif frag_start_stream(reference, candidate, i):
            ids.add('KD-FRAG-START')
        elif pcm_end_stream(reference, candidate, i):
            ids.add('KD-PCM-END')
        elif reference['streams'][i].get('duration_ts') != cstreams[i].get('duration_ts'):
            # Only a dropped stream that decides neither side's span may
            # differ: without it, both spans stay the same.
            if not props(cstreams[i]).get('dropped'):
                return None
            unmatched.append(i)
    if not ids:
        return None
    for i in unmatched:
        keep = [j for j in range(len(starts)) if j != i]
        if not keep:
            return None
        pick = lambda values: [values[j] for j in keep]  # noqa: E731
        if (_span_us(pick(starts), pick(rdur), pick(kinds)) != _span_us(starts, rdur, kinds)
                or _span_us(pick(starts), pick(cdur), pick(ckinds)) != _span_us(starts, cdur, ckinds)):
            return None
    # Each side's format duration follows from its own stream durations, so
    # no other format difference can hide behind the covered streams.
    if (reference.get('format_duration_us') == _span_us(starts, rdur, kinds)
            and candidate.get('format_duration_us') == _span_us(starts, cdur, ckinds)):
        return ids
    return None


def unplaced_format(reference, candidate):
    """KD-EDIT-UNPLACED: CoreAV leaves a stream whose edit list is outside
    its model (`unsupported`) out of the span, where the reference counts it
    from time 0 for its own duration. Every other stream agrees or is
    covered as for `duration_format`; CoreAV's format duration is the span
    of the streams it places, the reference's that span with the unplaced
    streams added. None when this does not hold."""
    candidate = _with_dropped(candidate)
    dropped = {d.get('stream_index'): d for d in (candidate.get('properties') or {}).get('dropped') or []}
    present = {s.get('index'): s for s in candidate.get('streams', [])}
    unplaced, placed = [], []
    for r in reference.get('streams', []):
        kept = present.get(r.get('index'))
        edits = props(kept).get('edits') if kept else dropped.get(r.get('index'), {}).get('edits')
        if kept is not None and not props(kept).get('dropped') and edits != 'unsupported':
            placed.append(r)
        elif kept is not None and props(kept).get('dropped'):
            placed.append(r)
        elif edits == 'unsupported' and isinstance(r.get('duration_ts'), int) and r.get('timescale'):
            unplaced.append(r)
        else:
            return None
    if not unplaced or not placed:
        return None
    reduced = {**reference, 'streams': placed}
    rest = {**candidate, 'streams': [present[r['index']] for r in placed]}
    ids = set()
    for i in range(len(placed)):
        for kd_id, check in (('KD-STTS-SHORT', stts_short_stream), ('KD-EDIT-DURATION', edit_duration_stream),
                             ('KD-FRAG-START', frag_start_stream), ('KD-PCM-END', pcm_end_stream)):
            if check(reduced, rest, i):
                ids.add(kd_id)
                break
        else:
            if placed[i].get('duration_ts') != rest['streams'][i].get('duration_ts'):
                return None
    spans = _starts_and_durations(reduced, rest)
    if spans is None:
        return None
    starts, rdur, cdur = spans
    kinds = [r.get('kind') for r in placed]
    ckinds = [c.get('kind') for c in rest['streams']]
    full = (starts + [Fraction(0)] * len(unplaced),
            rdur + [Fraction(r['duration_ts'], r['timescale']) for r in unplaced],
            kinds + [r.get('kind') for r in unplaced])
    if (candidate.get('format_duration_us') == _span_us(starts, cdur, ckinds)
            and reference.get('format_duration_us') == _span_us(*full)
            and _span_us(*full) != _span_us(starts, rdur, kinds)):
        return ids | {'KD-EDIT-UNPLACED'}
    return None


def stts_short_format(reference, candidate):
    return 'KD-STTS-SHORT' in (duration_format(reference, candidate) or set())


def edit_mapped(stream, packet):
    """The packet's `pts` and `dts` are its media times as the stream's edit
    list (none, or the simple model) maps them: minus the edit's media time,
    plus the presentation offset of a leading empty edit, rounded as the
    adapter rounds. For packets the reference does not list, so that their
    timestamps are still checked."""
    p = props(stream)
    edits, offset = p.get('edits'), fraction(p.get('presentation_offset_ticks'))
    if offset is None or not all(isinstance(packet.get(f), int) for f in ('media_cts', 'media_dts', 'pts', 'dts')):
        return False
    if edits == 'none':
        start = 0
    elif isinstance(edits, dict) and isinstance(edits.get('media_time'), int):
        start = edits['media_time']
    else:
        return False
    return (packet['pts'] == round_half_away(packet['media_cts'] - start + offset)
            and packet['dts'] == round_half_away(packet['media_dts'] - start + offset))


def edit_discard(stream, packet):
    """KD-EDIT-DISCARD: a packet CoreAV lists as discarded lies outside the
    simple edit, and its `pts` and `dts` are the edit's mapping of its media
    times (the media time minus the edit's, plus the presentation offset of
    a leading empty edit, rounded as the adapter rounds), which the
    reference, listing no such packet, cannot check."""
    edits = props(stream).get('edits')
    segment = fraction(props(stream).get('media_edit_duration_ticks'))
    if (packet.get('discard') is not True or not isinstance(edits, dict) or segment is None
            or not edit_mapped(stream, packet)):
        return False
    start, end = Fraction(edits['media_time']), Fraction(edits['media_time']) + segment
    cts, duration = packet.get('media_cts'), packet.get('duration') or 0
    if duration > 0:
        return cts + duration <= start or cts >= end
    return cts < start or cts >= end


def run_discarded(stream, run):
    """Every packet of a run (tools/oracle/SPEC.md, "Runs") lies outside the
    edit: its first and last packets do, on the same side (the media
    timestamps grow along the run)."""
    length, step = run.get('run', 1), run.get('duration') or 0
    first = {k: v for k, v in run.items() if k not in ('run', 'unpaired')}
    if not all(isinstance(first.get(f), int) for f in ('media_cts', 'media_dts', 'pts', 'dts')):
        return False
    last = dict(first, **{f: first[f] + (length - 1) * step for f in ('media_cts', 'media_dts', 'pts', 'dts')})
    edits = props(stream).get('edits')
    if not isinstance(edits, dict) or not edit_discard(stream, first) or not edit_discard(stream, last):
        return False
    start = Fraction(edits['media_time'])
    return (first['media_cts'] < start) == (last['media_cts'] < start)


def ctts_shift(stream, reference_packet, candidate_packet, shift=None):
    """KD-CTTS: the reference's decode time is the candidate's plus the
    smallest composition offset, which is negative and was read signed
    (`ctts_dts_shift`; pass it in to avoid recomputing it per packet)."""
    if candidate_packet is None:
        return False
    if shift is None:
        shift = ctts_dts_shift(stream)
    if shift >= 0:
        return False
    same = all(reference_packet.get(f) == candidate_packet.get(f) for f in ('pos', 'pts', 'size', 'keyframe'))
    return same and reference_packet.get('dts') == candidate_packet.get('dts') + shift


def ctts_v0_shift(stream, movie=None):
    """For KD-CTTS-UNSIGNED: the smallest composition offset of a version 0 table
    CoreAV read unsigned, with every offset of 2^31 or more read as the
    negative value it is as signed; None when no offset is that large, or
    when the file (`movie`, the candidate's properties) declares no ISO
    brand: a QuickTime file's offsets are signed, and reading them unsigned
    would be a CoreAV bug, not this entry."""
    p = props(stream)
    brand = (movie or {}).get('major_brand')
    if p.get('ctts_version') != 0 or p.get('ctts_signed') is not False or not isinstance(brand, str) or brand == 'qt  ':
        return None
    offsets = [q['media_cts'] - q['media_dts'] for q in stream.get('packets', [])
               if isinstance(q.get('media_cts'), int) and isinstance(q.get('media_dts'), int)]
    if not any(o >= 1 << 31 for o in offsets):
        return None
    return min(min(o - (1 << 32) if o >= 1 << 31 else o for o in offsets), 0)


def ctts_v0(stream, reference_packet, candidate_packet, shift=None, movie=None):
    """KD-CTTS-UNSIGNED: an ISO file's version 0 `ctts` that stores negative
    offsets. CoreAV reads them unsigned (ISO/IEC 14496-12 8.6.1.3); the
    reference reads them signed, so a wrapped offset's `pts` is 2^32
    smaller, and shifts every `dts` by the smallest signed offset."""
    if candidate_packet is None:
        return False
    if shift is None:
        shift = ctts_v0_shift(stream, movie)
    if shift is None:
        return False
    cts, dts = candidate_packet.get('media_cts'), candidate_packet.get('media_dts')
    if not all(isinstance(v, int) for v in (cts, dts, candidate_packet.get('pts'), candidate_packet.get('dts'))):
        return False
    wrap = 1 << 32 if cts - dts >= 1 << 31 else 0
    same = all(reference_packet.get(f) == candidate_packet.get(f) for f in ('pos', 'size', 'keyframe'))
    return (same and reference_packet.get('pts') == candidate_packet['pts'] - wrap
            and reference_packet.get('dts') == candidate_packet['dts'] + shift)


def edit_snap_shift(stream):
    """How far `ffprobe` moves a video stream's edit start: from the media
    time of its simple edit list to the first composition time at or after
    it (0 when the media time is a composition time, or for other streams)."""
    edits = props(stream).get('edits')
    if stream.get('kind') != 'video' or not isinstance(edits, dict):
        return 0
    start = edits.get('media_time')
    if not isinstance(start, int) or start < 0:
        return 0
    later = [p['media_cts'] for p in stream.get('packets', [])
             if isinstance(p.get('media_cts'), int) and p['media_cts'] >= start]
    return min(later) - start if later else 0


def ctts_dts_shift(stream):
    """KD-CTTS's shift of `ffprobe`'s decode times: the track's smallest
    composition offset when the offsets were read signed and it is
    negative, else 0."""
    p = props(stream)
    signed = p.get('ctts_signed') if 'ctts_signed' in p else p.get('ctts_version') == 1
    if signed is not True:
        return 0
    offsets = [q['media_cts'] - q['media_dts'] for q in stream.get('packets', [])
               if isinstance(q.get('media_cts'), int) and isinstance(q.get('media_dts'), int)]
    return min(min(offsets, default=0), 0)


def trun_pts_shift(stream):
    """KD-TRUN-SHIFT's shift of `ffprobe`'s presentation times: minus the
    smallest composition offset of a track whose samples all come from
    track fragments and that has no edit list, when that offset is
    negative; else 0."""
    p = props(stream)
    if not isinstance(p.get('fragment_start_ticks'), int) or p.get('edits') != 'none':
        return 0
    offsets = [q['media_cts'] - q['media_dts'] for q in stream.get('packets', [])
               if isinstance(q.get('media_cts'), int) and isinstance(q.get('media_dts'), int)]
    return -min(min(offsets, default=0), 0)


def trun_shift(stream, reference_packet, candidate_packet, shift=None):
    """KD-TRUN-SHIFT: in a fragmented track with negative composition
    offsets (`trun` version 1), the reference's `pts` is the candidate's
    minus the smallest offset over the whole track; `dts` is unchanged."""
    if candidate_packet is None:
        return False
    if shift is None:
        shift = trun_pts_shift(stream)
    if shift <= 0 or not isinstance(candidate_packet.get('pts'), int):
        return False
    same = all(reference_packet.get(f) == candidate_packet.get(f) for f in ('pos', 'dts', 'size', 'keyframe'))
    return same and reference_packet.get('pts') == candidate_packet['pts'] + shift


def edit_snap(stream, reference_packet, candidate_packet, shift=None, dts_shift=None):
    """KD-EDIT-SNAP: `pts` and `dts` earlier by the snap, `dts` also by
    KD-CTTS's shift where that applies (signed offsets, a negative
    smallest)."""
    if candidate_packet is None:
        return False
    if shift is None:
        shift = edit_snap_shift(stream)
    if dts_shift is None:
        dts_shift = ctts_dts_shift(stream)
    if shift <= 0:
        return False
    same = all(reference_packet.get(f) == candidate_packet.get(f) for f in ('pos', 'size', 'keyframe'))
    expected = {'pts': -shift, 'dts': -shift + dts_shift}
    return same and all(isinstance(candidate_packet.get(f), int)
                        and reference_packet.get(f) == candidate_packet[f] + d for f, d in expected.items())


def pcm_unit(candidate_stream):
    """The sample size of a constant-size PCM stream (one sample of every
    channel), or None."""
    c = candidate_stream
    unit, bits, channels = props(c).get('fixed_sample_size'), props(c).get('bits_per_sample'), c.get('channels')
    if (not (c.get('codec_name') or '').startswith('pcm_') or not unit or not bits or bits % 8
            or not channels or unit != channels * bits // 8):
        return None
    return unit


def presentation_end(candidate_stream, movie):
    """Where `ffprobe` ends a PCM stream's last packet, as a timestamp in the
    stream's time base, from CoreAV's own reading of the file; None when
    the file shows no evidence for it. With one edit at media time 0, the
    edit's duration in media ticks; without an edit list, the movie's
    duration (`mvhd`) in media ticks; either rounded to the nearest tick
    (ISO/IEC 14496-12 8.2.2, 8.6.6). An empty edit before the media does not
    move it: `ffprobe` ends the packet there even though the empty edit
    shifts the packet's timestamps."""
    timescale = candidate_stream.get('timescale')
    if not isinstance(timescale, int) or timescale <= 0 or candidate_stream.get('time_base') != [1, timescale]:
        return None
    p = props(candidate_stream)
    edits = p.get('edits')
    if isinstance(edits, dict):
        pair = p.get('media_edit_duration_ticks')
        if (edits.get('media_time') != 0 or not isinstance(pair, list) or len(pair) != 2
                or not all(isinstance(v, int) for v in pair) or pair[1] <= 0):
            return None
        return round_half_up(Fraction(pair[0], pair[1]))
    movie = movie if isinstance(movie, dict) else {}
    scale, duration = movie.get('movie_timescale'), movie.get('raw_movie_duration_ticks')
    if edits != 'none' or not isinstance(scale, int) or scale <= 0 or not isinstance(duration, int):
        return None
    return round_half_up(Fraction(duration * timescale, scale))


def pcm_end_stream(reference, candidate, index):
    """KD-PCM-END: a constant-size PCM stream's duration, which `ffprobe`
    ends where it ends the stream's last packet (`presentation_end`: the
    single edit's duration, or without an edit list the movie header's
    duration in the stream's ticks), while CoreAV reports the Decision 13
    duration of its samples."""
    try:
        r, c = reference['streams'][index], paired(reference, candidate)[index]
    except (IndexError, KeyError):
        return False
    if not c or pcm_unit(c) is None:
        return False
    # CoreAV's value is its Decision 13 duration: the media header's,
    # capped by the media edit.
    p = props(c)
    mdhd, edit = p.get('mdhd_duration_ticks'), fraction(p.get('media_edit_duration_ticks'))
    if not isinstance(mdhd, int) or mdhd <= 0 or not isinstance(c.get('duration_ts'), int):
        return False
    ours = round_half_up(min(Fraction(mdhd), edit) if edit is not None else Fraction(mdhd))
    end = presentation_end(c, candidate.get('properties'))
    return end is not None and c['duration_ts'] == ours and r.get('duration_ts') == end and ours != end


def pcm_duration(candidate_stream, ref, elapsed, previous, movie):
    """Whether a reference packet's duration fits the samples that tile it:
    their summed duration, or, for the stream's last packet, the time from
    its timestamp to where `ffprobe` ends the stream (`presentation_end`),
    shorter or longer than the samples. `previous` is the reference packet
    before the last one, None for any other packet. The exception needs
    every sample of the stream to last the same, and the previous packet's
    duration to be that duration times its samples, so `ffprobe`'s own
    durations check the last packet's samples too. Exact half ticks round
    up; no file has shown how `ffprobe` breaks that tie."""
    duration = ref.get('duration')
    if duration is None or duration == elapsed:
        return True
    unit = pcm_unit(candidate_stream)
    if (previous is None or unit is None or not isinstance(duration, int)
            or not isinstance(ref.get('pts'), int) or not isinstance(previous.get('size'), int)):
        return False
    steps = {p.get('duration') for p in candidate_stream.get('packets', [])}
    if len(steps) != 1 or previous.get('duration') != next(iter(steps)) * (previous['size'] // unit):
        return False
    end = presentation_end(candidate_stream, movie)
    return end is not None and duration > 0 and duration == end - ref['pts']


def pcm_grouping(reference_stream, candidate_stream, movie=None):
    c, unit = candidate_stream, pcm_unit(candidate_stream)
    if unit is None:
        return False
    # The candidate's samples, one per packet or a run of them (`run`
    # consecutive samples, each one size and one duration further), tile the
    # reference packets in order. Walked a run segment at a time, so a run
    # is never expanded.
    candidates, cursor, offset = c.get('packets', []), 0, 0
    refs = reference_stream.get('packets', [])
    for index, ref in enumerate(refs):
        position, end, elapsed, first = ref['pos'], ref['pos'] + ref['size'], 0, (cursor, offset)
        while cursor < len(candidates) and position < end:
            p = candidates[cursor]
            length, step = p.get('run', 1), p.get('duration')
            if (p.get('pos') is None or p['size'] != unit or not step or p['keyframe'] != ref['keyframe']
                    or ref.get('pts') is None or ref.get('dts') is None
                    or p.get('pts') is None or p.get('dts') is None
                    or p['pos'] + offset * unit != position
                    or p['pts'] + offset * step != ref['pts'] + elapsed
                    or p['dts'] + offset * step != ref['dts'] + elapsed):
                return False
            take = min(length - offset, (end - position) // unit)
            if take <= 0:
                return False
            position += take * unit
            elapsed += take * step
            offset += take
            if offset == length:
                cursor, offset = cursor + 1, 0
        if (position != end or (cursor, offset) == first
                or not pcm_duration(c, ref, elapsed, refs[index - 1] if 0 < index == len(refs) - 1 else None, movie)):
            return False
    return cursor == len(candidates) and offset == 0


def run_slice(entry, start, count):
    """Packets `start` to `start + count` of a candidate entry (one packet or
    a run, tools/oracle/SPEC.md "Runs"), as a run."""
    step = entry.get('duration') or 0
    moved = {f: entry[f] + start * step for f in ('pts', 'dts', 'media_dts', 'media_cts')
             if isinstance(entry.get(f), int)}
    return dict(entry, **moved, pos=entry['pos'] + start * entry['size'], run=count)


def pcm_reference_units(reference_stream, candidate_stream, movie=None):
    """A constant-size PCM stream (the candidate lists runs) counted in the
    reference's packet unit (docs/design/stage3-report.md, "Packet unit"):
    `(items, direct, covered, uncovered, notes)`.

    Each reference packet is one unit. It agrees directly when one candidate
    packet starts at its `pos` with the same `pos`, `size`, `pts`, `dts` and
    `keyframe` (as the comparison pairs them); it is covered by KD-PCM when
    the candidate's samples tile it exactly (`pcm_grouping`, for that packet
    alone); otherwise it is a mismatch. Each stretch of consecutive candidate
    samples that starts in no reference packet is one more unit: covered by
    KD-EDIT-DISCARD when all of it lies outside the edit, else a mismatch."""
    refs, entries = reference_stream.get('packets', []), candidate_stream.get('packets', [])
    covered = collections.Counter()
    # Candidate packets laid out by position: every one has a position and a
    # size, and none overlaps another. Otherwise no packet is located, and
    # every reference packet and the candidate's packets are mismatches.
    located = sorted(range(len(entries)), key=lambda k: entries[k].get('pos') if isinstance(
        entries[k].get('pos'), int) else -1)
    spans = []
    for k in located:
        e = entries[k]
        if (not isinstance(e.get('pos'), int) or not isinstance(e.get('size'), int) or e['size'] <= 0
                or not isinstance(e.get('run', 1), int) or e.get('run', 1) < 1
                or (spans and spans[-1][1] > e['pos'])):
            spans = None
            break
        spans.append((e['pos'], e['pos'] + e.get('run', 1) * e['size'], k))
    if spans is None:
        lump = 1 if entries else 0
        return (len(refs) + lump, 0, {}, len(refs) + lump,
                [f"streams/{reference_stream.get('index', '?')}/packets/pcm_layout"])
    starts = [span[0] for span in spans]

    def at(position):
        """(entry, index in the entry) of the candidate packet starting at
        `position`, or None."""
        i = bisect.bisect_right(starts, position) - 1
        if i < 0 or position >= spans[i][1]:
            return None
        e = entries[spans[i][2]]
        index, rest = divmod(position - e['pos'], e['size'])
        return None if rest else (e, index)

    def tiled(ref, previous):
        unit = pcm_unit(candidate_stream)
        if (unit is None or ref.get('pts') is None or ref.get('dts') is None
                or not isinstance(ref.get('size'), int) or ref['size'] <= 0):
            return False
        position, end, elapsed = ref['pos'], ref['pos'] + ref['size'], 0
        while position < end:
            found = at(position)
            if found is None:
                return False
            p, offset = found
            step = p.get('duration')
            if (p['size'] != unit or not step or p.get('keyframe') != ref.get('keyframe')
                    or p.get('pts') is None or p.get('dts') is None
                    or p['pts'] + offset * step != ref['pts'] + elapsed
                    or p['dts'] + offset * step != ref['dts'] + elapsed):
                return False
            take = min(p.get('run', 1) - offset, (end - position) // unit)
            if take <= 0:
                return False
            position += take * unit
            elapsed += take * step
        return pcm_duration(candidate_stream, ref, elapsed, previous, movie)

    # `ffprobe` lists a stream's packets in position order; a reference
    # packet that starts before the end of an earlier-listed one is a
    # mismatch, so no sample counts for two.
    direct, uncovered, notes, claimed, reach = 0, 0, [], [], None
    for index, ref in enumerate(refs):
        found = at(ref['pos']) if isinstance(ref.get('pos'), int) else None
        sample = run_slice(*found, 1) if found else None
        overlaps = reach is not None and isinstance(ref.get('pos'), int) and ref['pos'] < reach
        if isinstance(ref.get('pos'), int) and isinstance(ref.get('size'), int) and ref['size'] > 0:
            claimed.append((ref['pos'], ref['pos'] + ref['size']))
            reach = max(reach or 0, ref['pos'] + ref['size'])
        if (not overlaps and sample is not None
                and all(sample.get(f) == ref.get(f) for f in ('pos', 'size', 'pts', 'dts', 'keyframe'))):
            direct += 1
        elif not overlaps and found is not None and tiled(ref, refs[index - 1] if 0 < index == len(refs) - 1 else None):
            covered['KD-PCM'] += 1
        else:
            uncovered += 1
            if len(notes) < 5:
                notes.append(f"streams/{reference_stream.get('index', '?')}/packets/{index}/pcm_tiling")
    # The candidate samples that start in no reference packet, in stream
    # order; consecutive ones form one stretch.
    claimed.sort()
    union = []
    for start, end in claimed:
        if union and start <= union[-1][1]:
            union[-1][1] = max(union[-1][1], end)
        else:
            union.append([start, end])
    union_starts = [u[0] for u in union]

    def free(e):
        """The pieces (first, count) of a candidate entry outside every
        reference packet."""
        size, length, pieces, index = e['size'], e.get('run', 1), [], 0
        while index < length:
            position = e['pos'] + index * size
            i = bisect.bisect_right(union_starts, position) - 1
            if i >= 0 and position < union[i][1]:
                index = max(index + 1, -(-(union[i][1] - e['pos']) // size))
                continue
            following = union_starts[i + 1] if i + 1 < len(union) else None
            stop = length if following is None else min(length, -(-(following - e['pos']) // size))
            pieces.append((index, stop - index))
            index = stop
        return pieces

    stretches, open_stretch = [], False
    for e in entries:
        pieces = free(e)
        for first, count in pieces:
            if first == 0 and open_stretch:
                stretches[-1].append(run_slice(e, first, count))
            else:
                stretches.append([run_slice(e, first, count)])
            open_stretch = first + count == e.get('run', 1)
        if not pieces:
            open_stretch = False
    for stretch in stretches:
        if all(run_discarded(candidate_stream, piece) for piece in stretch):
            covered['KD-EDIT-DISCARD'] += 1
        else:
            uncovered += 1
            if len(notes) < 5:
                notes.append(f"streams/{reference_stream.get('index', '?')}/packets/extra_run")
    return len(refs) + len(stretches), direct, dict(covered), uncovered, notes


# AAC profile (Decision 25). AudioSpecificConfig layout: ISO/IEC 14496-3
# 1.6.2.1; implicit SBR signalling: ISO/IEC 14496-3 1.6.5 and 4.6.18.
AAC_LC_CODE_POINTS = 'Aac { object_type: 2, sbr: false, ps: false }'
MAX_IMPLICIT_SBR_CORE_RATE = 24000


def by_index(document, index):
    for s in document.get('streams', []):
        if s.get('index') == index:
            return s
    return None


def _plain_lc(c):
    return (c is not None and c.get('codec_name') == 'aac' and c.get('profile') == 'LC'
            and props(c).get('profile_code_points') == AAC_LC_CODE_POINTS)


def aac_implicit_sbr(reference, candidate, index):
    """CoreAV reports the declared LC object type; ffprobe reports HE-AAC from
    SBR data it finds by decoding. Only a core rate of at most 24 kHz can carry
    implicit SBR."""
    r, c = by_index(reference, index), by_index(candidate, index)
    rate = c.get('sample_rate') if c else None
    return (_plain_lc(c) and r is not None and r.get('codec_name') == 'aac'
            and r.get('profile') == 'HE-AAC' and isinstance(rate, int) and 0 < rate <= MAX_IMPLICIT_SBR_CORE_RATE)


def aac_lc_unnamed(reference, candidate, index):
    """A valid LC configuration for which ffprobe prints no profile."""
    r, c = by_index(reference, index), by_index(candidate, index)
    return _plain_lc(c) and r is not None and r.get('codec_name') == 'aac' and r.get('profile') is None


def quicktime_text_dropped(reference, candidate):
    """KD-QT-TEXT: the only streams missing from the candidate are QuickTime
    text tracks that it reports as dropped by the allowlist."""
    dropped = (candidate.get('properties') or {}).get('dropped') or []
    if not dropped or any(d.get('codec_fourcc') != 'text' or d.get('reason') != 'CodecNotAllowed'
                          for d in dropped):
        return False
    rstreams, cstreams = reference.get('streams', []), candidate.get('streams', [])
    dropped_indexes = {d.get('stream_index') for d in dropped}
    if len(dropped_indexes) != len(dropped) or len(rstreams) != len(cstreams) + len(dropped):
        return False
    if {s.get('index') for s in rstreams} != {s.get('index') for s in cstreams} | dropped_indexes:
        return False
    return all(s.get('codec_fourcc') == 'text' for s in rstreams if s.get('index') in dropped_indexes)


def _visual_entry_size(path, index):
    """Width and height the first sample entry of the file's track at
    position `index` declares, from the box headers (ISO/IEC 14496-12
    8.5.2 and 12.1.3: the visual entry's width and height follow 24 bytes
    of reserved and predefined fields after its header)."""
    def boxes(data, start, end):
        at = start
        while at + 8 <= end:
            size = int.from_bytes(data[at:at + 4], 'big')
            header = 8
            if size == 1:
                size, header = int.from_bytes(data[at + 8:at + 16], 'big'), 16
            elif size == 0:
                size = end - at
            if size < header or at + size > end:
                return
            yield data[at + 4:at + 8], at + header, at + size
            at += size

    def child(data, start, end, kind):
        return next(((s, e) for k, s, e in boxes(data, start, end) if k == kind), None)

    data = pathlib.Path(path).read_bytes()
    moov = child(data, 0, len(data), b'moov')
    traks = [(s, e) for k, s, e in boxes(data, *moov) if k == b'trak'] if moov else []
    if index >= len(traks):
        return None
    box = traks[index]
    for name in (b'mdia', b'minf', b'stbl', b'stsd'):
        box = child(data, *box, name) if box else None
    if not box or box[1] - box[0] < 8 + 36:
        return None
    entry = box[0] + 8
    return (int.from_bytes(data[entry + 32:entry + 34], 'big'),
            int.from_bytes(data[entry + 34:entry + 36], 'big'))


def _first_frame_size(path, index):
    """The size of the stream's first decoded frame, from ffprobe (a black
    box), or None."""
    try:
        done = subprocess.run(
            ['ffprobe', '-v', 'error', '-select_streams', str(index), '-read_intervals', '%+#1',
             '-show_entries', 'frame=width,height', '-of', 'json', str(path)],
            capture_output=True, timeout=60, check=False)
        frames = json.loads(done.stdout).get('frames', [])
    except (OSError, subprocess.TimeoutExpired, json.JSONDecodeError):
        return None
    return (frames[0].get('width'), frames[0].get('height')) if frames else None


def declared_size(media_dir, file, reference, candidate, index):
    """CoreAV's width and height are the sample entry's; ffprobe's are the
    decoded pictures', which the codec's parameter sets or frame headers
    make larger or smaller than declared."""
    r, c = by_index(reference, index), by_index(candidate, index)
    if r is None or c is None:
        return False
    path = pathlib.Path(media_dir) / file
    declared = _visual_entry_size(path, index)
    ours, theirs = (c.get('width'), c.get('height')), (r.get('width'), r.get('height'))
    return (declared is not None and ours == declared and theirs != declared
            and _first_frame_size(path, index) == theirs)


# Protected sample entries (ISO/IEC 14496-12 8.12): the entry type names the
# protection; `frma` in its `sinf` holds the original format.
PROTECTED = ('enca', 'encv', 'enct', 'encs')
# The codecs of CoreAV's default allowlist (coreav-isobmff `Allowlist`), as
# `ffprobe` names them: a stream of one of these that CoreAV drops is a
# CoreAV bug, never a known divergence.
ALLOWLISTED_CODEC_NAMES = ('h264', 'hevc', 'av1', 'vp9', 'prores', 'aac', 'mp3', 'opus', 'mov_text')
# The sample entry types CoreAV maps to those codecs (ISO/IEC 14496-12 and
# the codec bindings; Apple ProRes `ap..`; QuickTime PCM): a dropped stream
# with one of them is a CoreAV bug whatever `ffprobe` calls it.
ALLOWLISTED_FOURCCS = ('avc1', 'avc3', 'hvc1', 'hev1', 'av01', 'vp09', 'Opus', 'sowt', 'twos', 'in24',
                       'in32', 'fl32', 'fl64', 'lpcm', 'ipcm', 'fpcm', 'raw ', 'tx3g', 'tmcd')


def allowlisted(stream):
    name = stream.get('codec_name') or ''
    fourcc = stream.get('codec_fourcc') or ''
    # Companded PCM (A-law, mu-law) is not linear PCM, which CoreAV allows.
    linear_pcm = name.startswith('pcm_') and name not in ('pcm_alaw', 'pcm_mulaw')
    return (name in ALLOWLISTED_CODEC_NAMES or linear_pcm or fourcc in ALLOWLISTED_FOURCCS
            or fourcc.startswith('ap'))


def protected_formats(media_dir, file):
    """{track position: original format} for the tracks of the file whose
    first sample entry is protected, read from the box headers with seeks
    (the `frma` box: size 12, type, the original format)."""
    path = pathlib.Path(media_dir) / file
    found = {}
    with open(path, 'rb') as f:
        end = f.seek(0, 2)

        def boxes(start, stop):
            at = start
            while at + 8 <= stop:
                f.seek(at)
                head = f.read(16)
                size, header = int.from_bytes(head[:4], 'big'), 8
                if size == 1:
                    size, header = int.from_bytes(head[8:16], 'big'), 16
                elif size == 0:
                    size = stop - at
                if size < header or at + size > stop:
                    return
                yield head[4:8], at + header, at + size
                at += size

        def child(start, stop, kind):
            return next(((s, e) for k, s, e in boxes(start, stop) if k == kind), None)

        moov = child(0, end, b'moov')
        traks = [(s, e) for k, s, e in boxes(*moov) if k == b'trak'] if moov else []
        for index, box in enumerate(traks):
            for name in (b'mdia', b'minf', b'stbl', b'stsd'):
                box = child(*box, name) if box else None
            entry = next(boxes(box[0] + 8, box[1]), None) if box else None
            if not entry or entry[0].decode('latin1') not in PROTECTED or entry[2] - entry[1] > 1 << 20:
                continue
            f.seek(entry[1])
            payload = f.read(entry[2] - entry[1])
            # The `sinf` box among the entry's children (after its fixed
            # fields, whose size depends on the entry), then `frma` in it.
            at = payload.find(b'sinf')
            while at >= 4:
                size = int.from_bytes(payload[at - 4:at], 'big')
                start, stop = at - 4, at - 4 + size
                if 8 <= size and stop <= len(payload):
                    inner = start + 8
                    while inner + 8 <= stop:
                        child = int.from_bytes(payload[inner:inner + 4], 'big')
                        if child < 8 or inner + child > stop:
                            break
                        if payload[inner + 4:inner + 8] == b'frma' and child == 12:
                            found[index] = payload[inner + 8:inner + 12].decode('latin1')
                        inner += child
                    if index in found:
                        break
                at = payload.find(b'sinf', at + 1)
    return found


# Cover art: an item of the metadata item list (`moov/udta/meta/ilst`, or
# `moov/meta/ilst`; QuickTime File Format, "Metadata"; `meta` is ISO/IEC
# 14496-12 8.11.1), either the `covr` item of iTunes-style metadata or, with
# a `keys` box (QuickTime metadata, items numbered by key), the item of the
# key `com.apple.quicktime.artwork`. It holds one `data` box per picture,
# whose type indicator names the picture format (the "well-known types": 13
# JPEG, 14 PNG, 27 BMP).
COVER_TYPES = {13: 'mjpeg', 14: 'png', 27: 'bmp'}
ARTWORK_KEY = b'mdtacom.apple.quicktime.artwork'
MAX_COVERS = 64
MAX_KEYS_BYTES = 1 << 16


def cover_art(media_dir, file):
    """The codec of each cover picture of the file, in order (None for a
    picture of another type), read from the box headers with seeks."""
    path = pathlib.Path(media_dir) / file
    with open(path, 'rb') as f:
        end = f.seek(0, 2)

        def boxes(start, stop):
            at = start
            while at + 8 <= stop:
                f.seek(at)
                head = f.read(16)
                size, header = int.from_bytes(head[:4], 'big'), 8
                if size == 1:
                    size, header = int.from_bytes(head[8:16], 'big'), 16
                elif size == 0:
                    size = stop - at
                if size < header or at + size > stop:
                    return
                yield head[4:8], at + header, at + size
                at += size

        def child(box, kind):
            return next(((s, e) for k, s, e in boxes(*box) if k == kind), None) if box else None

        def pictures(meta):
            """The picture items of a `meta` box."""
            if not meta:
                return []
            # ISO's `meta` is a full box (4 bytes of version and flags before
            # its children); QuickTime's is not: its first child follows.
            # Either way the first child is `hdlr`; without it, no pictures.
            f.seek(meta[0])
            head = f.read(12)
            if head[4:8] == b'hdlr':
                inner = (meta[0], meta[1])
            elif head[8:12] == b'hdlr':
                inner = (meta[0] + 4, meta[1])
            else:
                return []
            ilst, keys = child(inner, b'ilst'), child(inner, b'keys')
            artwork = None
            if keys and keys[1] - keys[0] <= MAX_KEYS_BYTES:
                # A full box, an entry count, then entries of a size, a
                # namespace and the key; items name their key by its number.
                f.seek(keys[0])
                payload, at, number = f.read(keys[1] - keys[0]), 8, 1
                while at + 8 <= len(payload):
                    size = int.from_bytes(payload[at:at + 4], 'big')
                    if size < 8 or at + size > len(payload):
                        break
                    if payload[at + 4:at + size] == ARTWORK_KEY:
                        artwork = number.to_bytes(4, 'big')
                    at, number = at + size, number + 1
            return [(s, e) for k, s, e in (boxes(*ilst) if ilst else ()) if k == b'covr' or k == artwork]

        moov = child((0, end), b'moov')
        found = []
        for item in pictures(child(child(moov, b'udta'), b'meta')) or pictures(child(moov, b'meta')):
            for kind, start, stop in boxes(*item):
                if kind != b'data' or stop - start < 8 or len(found) >= MAX_COVERS:
                    continue
                f.seek(start)
                # A type-set byte of 0, then the well-known type.
                found.append(COVER_TYPES.get(int.from_bytes(f.read(4), 'big')))
    return found


# Chapter tracks: a track's `tref` box (ISO/IEC 14496-12 8.3.3) of type
# `chap` (QuickTime File Format, "Chapter lists") names the tracks that hold
# its chapter titles.
MAX_CHAPTER_REFERENCE_BYTES = 1 << 12


def chapter_tracks(media_dir, file):
    """The positions among the file's tracks of those that any track names
    in a `tref/chap` box, matched by the `track_ID` of their `tkhd`
    (ISO/IEC 14496-12 8.3.2), read from the box headers with seeks."""
    path = pathlib.Path(media_dir) / file
    found, ids = set(), {}
    with open(path, 'rb') as f:
        end = f.seek(0, 2)

        def boxes(start, stop):
            at = start
            while at + 8 <= stop:
                f.seek(at)
                head = f.read(16)
                size, header = int.from_bytes(head[:4], 'big'), 8
                if size == 1:
                    size, header = int.from_bytes(head[8:16], 'big'), 16
                elif size == 0:
                    size = stop - at
                if size < header or at + size > stop:
                    return
                yield head[4:8], at + header, at + size
                at += size

        def child(box, kind):
            return next(((s, e) for k, s, e in boxes(*box) if k == kind), None) if box else None

        moov = child((0, end), b'moov')
        traks = [(s, e) for k, s, e in boxes(*moov) if k == b'trak'] if moov else []
        for position, trak in enumerate(traks):
            tkhd = child(trak, b'tkhd')
            if tkhd and tkhd[1] - tkhd[0] >= 24:
                # Version and flags, then the times (8 bytes each in
                # version 1, else 4), then the track ID.
                f.seek(tkhd[0])
                head = f.read(24)
                at = 20 if head[0] == 1 else 12
                ids.setdefault(int.from_bytes(head[at:at + 4], 'big'), position)
            chap = child(child(trak, b'tref'), b'chap')
            if chap and chap[1] - chap[0] <= MAX_CHAPTER_REFERENCE_BYTES:
                f.seek(chap[0])
                listed = f.read(chap[1] - chap[0])
                # A list of 32-bit track IDs; 0 is no track.
                found.update(int.from_bytes(listed[at:at + 4], 'big') for at in range(0, len(listed) - 3, 4))
    found.discard(0)
    return {ids[i] for i in found if i in ids}


def chapter_text(reference, candidate, index, chapters):
    """KD-CHAPTER-TEXT: the stream at `index` is a `tx3g` track that another
    track names as its chapter track (`chapters` returns the positions of
    the file's, from `chapter_tracks`; `ffprobe` numbers tracks in file
    order), which `ffprobe` lists as a `bin_data` data stream and CoreAV as
    a `mov_text` subtitle stream."""
    r, c = by_index(reference, index), by_index(candidate, index)
    if r is None or c is None or chapters is None:
        return False
    return (r.get('kind') == 'data' and r.get('codec_name') == 'bin_data'
            and c.get('kind') == 'subtitle' and c.get('codec_name') == 'mov_text'
            and r.get('codec_fourcc') == c.get('codec_fourcc') == 'tx3g' and index in chapters())



# Edit lists with several entries (docs/design/edit-lists.md): what the
# overlay reads of a track to recompute both listings.
MAX_EDIT_LIST_BYTES = 1 << 22
EditTrack = collections.namedtuple('EditTrack', 'movie_timescale timescale audio has_ctts edits')


def edit_track(media_dir, file, position):
    """The edit list of the track at `position` among the file's tracks,
    with the movie and media timescales, whether its handler is `soun` and
    whether it has a `ctts` box, read from the box headers with seeks;
    None when a box is missing or too large. Edits are `(segment_duration,
    media_time, media_rate)` as stored (ISO/IEC 14496-12 8.6.6)."""
    path = pathlib.Path(media_dir) / file
    with open(path, 'rb') as f:
        end = f.seek(0, 2)

        def boxes(start, stop):
            at = start
            while at + 8 <= stop:
                f.seek(at)
                head = f.read(16)
                size, header = int.from_bytes(head[:4], 'big'), 8
                if size == 1:
                    size, header = int.from_bytes(head[8:16], 'big'), 16
                elif size == 0:
                    size = stop - at
                if size < header or at + size > stop:
                    return
                yield head[4:8], at + header, at + size
                at += size

        def child(box, kind):
            return next(((s, e) for k, s, e in boxes(*box) if k == kind), None) if box else None

        def read(box, limit=1 << 16):
            if not box or box[1] - box[0] > limit:
                return None
            f.seek(box[0])
            return f.read(box[1] - box[0])

        def timescale(payload):
            # Version and flags, then the times (8 bytes each in version 1,
            # else 4), then the timescale (8.2.2, 8.4.2).
            if not payload or len(payload) < 24:
                return None
            at = 20 if payload[0] == 1 else 12
            return int.from_bytes(payload[at:at + 4], 'big')

        moov = child((0, end), b'moov')
        traks = [(s, e) for k, s, e in boxes(*moov) if k == b'trak'] if moov else []
        if not 0 <= position < len(traks):
            return None
        trak, mdia = traks[position], None
        mdia = child(trak, b'mdia')
        hdlr = read(child(mdia, b'hdlr'))
        # The last edit list of the track, as CoreAV keeps it.
        lists = [(s, e) for k, s, e in boxes(*trak) if k == b'edts']
        lists = [(s, e) for box in lists for k, s, e in boxes(*box) if k == b'elst']
        elst = read(lists[-1] if lists else None, MAX_EDIT_LIST_BYTES)
        movie_timescale, media_timescale = timescale(read(child(moov, b'mvhd'))), timescale(read(child(mdia, b'mdhd')))
        if not hdlr or len(hdlr) < 12 or not elst or len(elst) < 8 or not movie_timescale or not media_timescale:
            return None
        version, count = elst[0], int.from_bytes(elst[4:8], 'big')
        width = 20 if version == 1 else 12
        if 8 + count * width > len(elst):
            return None
        edits = []
        for at in range(8, 8 + count * width, width):
            if version == 1:
                duration, media_time = int.from_bytes(elst[at:at + 8], 'big'), int.from_bytes(elst[at + 8:at + 16], 'big', signed=True)
            else:
                duration, media_time = int.from_bytes(elst[at:at + 4], 'big'), int.from_bytes(elst[at + 4:at + 8], 'big', signed=True)
            edits.append((duration, media_time, int.from_bytes(elst[at + width - 4:at + width], 'big', signed=True)))
        stbl = child(child(mdia, b'minf'), b'stbl')
        return EditTrack(movie_timescale, media_timescale, hdlr[8:12] == b'soun',
                         child(stbl, b'ctts') is not None, edits)




TEXT_HANDLERS = (b'sbtl', b'text', b'subt')


def text_end(reference, candidate, index, facts):
    """KD-TEXT-END: a 3GPP timed text stream without an edit list, in a
    movie without fragments, whose duration CoreAV reports as its media
    header's, which `ffprobe` extends to the latest end of any track of the
    file, rounded to the stream's ticks. Every value is the file's, read by
    the `Facts` reader (`facts` returns it): the text track's media header
    duration, timescale and sample durations, and each track's edit list or
    media header for the latest end (each track's edit list durations added
    up over the movie timescale, or its media header duration over its
    timescale)."""
    r, c = by_index(reference, index), by_index(candidate, index)
    f = facts() if facts is not None else None
    if r is None or c is None or f is None or f.fragmented or props(candidate).get('fragmented'):
        return False
    p = props(c)
    track = f.tracks.get(p.get('track_id'))
    if (track is None or c.get('codec_name') != 'mov_text' or p.get('edits') != 'none' or track['edits']
            or track['handler'] not in TEXT_HANDLERS or p.get('duration_source') != 'MediaHeader'):
        return False
    scale, mdhd = track['timescale'], track['mdhd_duration']
    if not scale or c.get('timescale') != scale or c.get('duration_ts') != mdhd:
        return False
    samples = f.samples(track['track_id'])
    if samples is None or sum(x.duration for x in samples) > mdhd:
        return False
    ends = []
    for t in f.tracks.values():
        if t['edits'] and f.movie_timescale:
            ends.append(Fraction(sum(e[0] for e in t['edits']), f.movie_timescale))
        elif not t['edits'] and t['timescale']:
            ends.append(Fraction(t['mdhd_duration'], t['timescale']))
    if not ends:
        return False
    latest = round_half_up(max(ends) * scale)
    return r.get('duration_ts') == latest and latest > mdhd

def round_half_away(value):
    """Rounds a Fraction to the nearest integer, halves away from zero, as
    the adapter rounds CoreAV's exact times."""
    magnitude = abs(value)
    whole, rest = divmod(magnitude.numerator, magnitude.denominator)
    if 2 * rest >= magnitude.denominator:
        whole += 1
    return whole if value >= 0 else -whole


def multi_edit_mapping(samples, track):
    """CoreAV's times for a list of several edits
    (docs/design/edit-lists.md, decision 2): `(pts, dts, presented)` per
    sample, the times rounded as the adapter rounds them; None for a list
    CoreAV does not apply that way."""
    t, m = track.timescale, track.movie_timescale
    spans, at = [], 0
    for duration, media_time, rate in track.edits:
        if media_time != -1:
            if media_time < 0 or rate != 0x10000 or duration == 0:
                return None
            end = media_time + Fraction(duration * t, m)
            if spans and media_time < spans[-1][1]:
                return None
            spans.append((media_time, end, Fraction(at * t, m) - media_time))
        at += duration
    if not spans:
        return None
    starts = [s[0] for s in spans]
    out = []
    for cts, dts, duration, _ in samples:
        after = bisect.bisect_right(starts, cts)
        if after and cts < spans[after - 1][1]:
            span, presented = spans[after - 1], True
        elif after < len(spans):
            span = spans[after]
            presented = duration > 0 and span[0] < cts + duration
        else:
            span, presented = spans[-1], False
        out.append((round_half_away(cts + span[2]), round_half_away(dts + span[2]), presented))
    return out


def multi_edit_listing(samples, track):
    """`ffprobe`'s packet listing for a list of several edits, the black-box
    model of docs/design/edit-lists.md ("What ffprobe lists"):
    `(sample, pts, dts, discard)` per listed packet; None where the model
    does not say."""
    n = len(samples)
    if not n:
        return None
    t, m = track.timescale, track.movie_timescale
    cts = [s[0] for s in samples]
    ends = [s[0] + s[2] for s in samples]
    keys = [s[3] for s in samples]
    # Rule 3's F: the first sample with cts >= x is the first where the
    # running maximum reaches x.
    running, peak = [], None
    for c in cts:
        peak = c if peak is None else max(peak, c)
        running.append(peak)
    running_end, peak = [], None
    for e in ends:
        peak = e if peak is None else max(peak, e)
        running_end.append(peak)
    order = sorted(range(n), key=lambda i: cts[i])
    sorted_cts = [cts[i] for i in order]
    longest = max(s[2] for s in samples)
    previous_sync, last = [], 0
    for i, key in enumerate(keys):
        last = i if key else last
        previous_sync.append(last)
    next_sync, following = [n] * (n + 1), n
    for i in range(n - 1, -1, -1):
        following = i if keys[i] else following
        next_sync[i] = following
    dts_shift = min(0, min(s[0] - s[1] for s in samples))
    out, start, anchor, first = [], 0, None, True
    for duration, media_time, rate in track.edits:
        if rate != 0x10000 and media_time != -1:
            return None
        # Rule 1: whole ticks, halves up; starts add the rounded durations.
        length = (2 * duration * t + m) // (2 * m)
        if media_time == -1 and first:
            start += length
            continue
        end = media_time + length
        f_index = bisect.bisect_left(running, media_time)
        if f_index == n:
            start += length
            continue
        overlap = track.audio and first

        def shown(i):
            return (ends[i] > media_time if overlap else cts[i] >= media_time) and cts[i] < end
        low = bisect.bisect_left(sorted_cts, media_time - longest if overlap else media_time)
        high = bisect.bisect_left(sorted_cts, end)
        presented = [order[k] for k in range(low, high) if shown(order[k])]
        if not presented:
            return None
        last_shown = max(presented)
        if track.audio:
            # Rule 7: the sample shown one second (one timescale) earlier.
            preroll = min(bisect.bisect_right(running_end, max(0, media_time - t)), f_index)
        else:
            preroll = previous_sync[f_index]
        if first:
            snap = sorted_cts[bisect.bisect_left(sorted_cts, media_time)]
            shift = start - (media_time if track.audio else snap)
            anchor = samples[f_index][1] - snap
            first = False
        else:
            shift = start + anchor - samples[f_index][1]
        # Rule 8: to the first sync sample at or after the last shown one,
        # or the second with a ctts box.
        stop = next_sync[last_shown]
        if track.has_ctts and stop < n:
            stop = next_sync[stop + 1]
        for i in range(preroll, min(stop, n - 1) + 1):
            out.append((i, cts[i] + shift, samples[i][1] + shift + dts_shift, not shown(i)))
        start += length
    return out


def edit_multi(reference_stream, candidate_stream, track):
    """KD-EDIT-MULTI: a stream whose edit list CoreAV applies as several
    edits, when every reference packet is the black-box model's listing and
    every CoreAV packet has decision 2's times, both recomputed from CoreAV's
    raw media times and sync flags and the edit list read from the file
    (`track`, from `edit_track`)."""
    if track is None or props(candidate_stream).get('edits') != 'multiple':
        return False
    if candidate_stream.get('timescale') != track.timescale:
        return False
    packets = candidate_stream.get('packets', [])
    if any('run' in p or not isinstance(p.get('media_cts'), int) or not isinstance(p.get('media_dts'), int)
           for p in packets):
        return False
    samples = [(p['media_cts'], p['media_dts'], p.get('duration') or 0, bool(p.get('keyframe'))) for p in packets]
    mapping = multi_edit_mapping(samples, track)
    listing = multi_edit_listing(samples, track)
    if mapping is None or listing is None:
        return False
    if any((p.get('pts'), p.get('dts'), p.get('discard')) != (pts, dts, not presented)
           for p, (pts, dts, presented) in zip(packets, mapping)):
        return False
    reference = reference_stream.get('packets', [])
    if len(reference) != len(listing):
        return False
    for r, (i, pts, dts, discard) in zip(reference, listing):
        c = packets[i]
        if (r.get('pos'), r.get('size'), r.get('keyframe'), r.get('pts'), r.get('dts'), bool(r.get('discard'))) != (
                c.get('pos'), c.get('size'), c.get('keyframe'), pts, dts, discard):
            return False
    return True

def cover_art_streams(reference, candidate, covers):
    """KD-COVER-ART: the reference streams the candidate neither lists nor
    reports as dropped, when they are exactly the file's cover pictures:
    as many as `covers` (`cover_art`) holds, the last streams of the
    reference by index, video streams with each picture's codec in order
    and no sample-entry fourcc (a track always has one, so a track CoreAV
    fails to list is never covered). Returns their indexes, or None."""
    listed = {s.get('index') for s in candidate.get('streams', [])}
    dropped = {d.get('stream_index') for d in (candidate.get('properties') or {}).get('dropped') or []}
    rstreams = sorted(reference.get('streams', []), key=lambda s: s.get('index', -1))
    missing = [s for s in rstreams if s.get('index') not in listed | dropped]
    if not missing or not isinstance(covers, list) or len(missing) != len(covers):
        return None
    if [s.get('index') for s in missing] != [s.get('index') for s in rstreams[len(rstreams) - len(missing):]]:
        return None
    if any(s.get('kind') != 'video' or c is None or s.get('codec_name') != c or s.get('codec_fourcc') is not None
           for s, c in zip(missing, covers)):
        return None
    return {s.get('index') for s in missing}


def without_streams(document, indexes):
    return dict(document, streams=[s for s in document.get('streams', []) if s.get('index') not in indexes])


def dropped_entries(reference, candidate, protected=None):
    """The entry that explains each stream the candidate drops, by stream
    index, or None: KD-QT-TEXT when they are all QuickTime text tracks,
    KD-NOT-ALLOWED when the allowlist explains them all, and KD-NOT-ALLOWED
    for all of them when the candidate drops both kinds, the `text` tracks
    meeting KD-QT-TEXT's predicate and the others KD-NOT-ALLOWED's, each
    against the reference without the other part's streams. A mixed drop
    is covered only because of KD-NOT-ALLOWED, so it is counted there."""
    dropped = (candidate.get('properties') or {}).get('dropped') or []
    indexes = [d.get('stream_index') for d in dropped]
    if quicktime_text_dropped(reference, candidate):
        return dict.fromkeys(indexes, 'KD-QT-TEXT')
    if allowlist_dropped(reference, candidate, protected):
        return dict.fromkeys(indexes, 'KD-NOT-ALLOWED')
    text = [d for d in dropped if d.get('codec_fourcc') == 'text']
    other = [d for d in dropped if d.get('codec_fourcc') != 'text']
    if not text or not other:
        return None
    text_indexes = {d.get('stream_index') for d in text}
    other_indexes = {d.get('stream_index') for d in other}

    def only(part):
        return dict(candidate, properties=dict(candidate.get('properties') or {}, dropped=part))
    if (quicktime_text_dropped(without_streams(reference, other_indexes), only(text))
            and allowlist_dropped(without_streams(reference, text_indexes), only(other), protected)):
        return dict.fromkeys(text_indexes | other_indexes, 'KD-NOT-ALLOWED')
    return None


def stream_count_entries(reference, candidate, protected=None, covers=None):
    """The entries that explain a stream count difference, or None: KD-QT-TEXT
    or KD-NOT-ALLOWED for the streams the candidate drops, KD-COVER-ART for
    the cover pictures `ffprobe` adds as streams (`covers` returns the
    file's, read only when needed), or both together."""
    def dropped(ref):
        found = dropped_entries(ref, candidate, protected)
        return set(found.values()) if found else None
    found = dropped(reference)
    if found or covers is None:
        return found
    pictures = cover_art_streams(reference, candidate, covers())
    if pictures is None:
        return None
    rest = without_streams(reference, pictures)
    if not (candidate.get('properties') or {}).get('dropped'):
        same = {s.get('index') for s in rest.get('streams', [])} == {s.get('index') for s in candidate.get('streams', [])}
        return {'KD-COVER-ART'} if same else None
    found = dropped(rest)
    return found | {'KD-COVER-ART'} if found else None


def allowlist_dropped(reference, candidate, protected=None):
    """KD-NOT-ALLOWED: the only streams missing from the candidate are tracks
    it reports as dropped by the allowlist, each with the reference's fourcc
    at that index and a codec the default allowlist does not hold (as
    `ffprobe` names it), so a stream CoreAV fails to recognise is never
    covered; for a protected entry (`enca`, `encv`, ...), dropped for its
    protection and reported by `ffprobe` under its original format, the
    `frma` box of the file (read through `protected`) must hold the
    reference's fourcc."""
    dropped = (candidate.get('properties') or {}).get('dropped') or []
    if not dropped or any(d.get('reason') != 'CodecNotAllowed' for d in dropped):
        return False
    rstreams, cstreams = reference.get('streams', []), candidate.get('streams', [])
    fourccs = {d.get('stream_index'): d.get('codec_fourcc') for d in dropped}
    if len(fourccs) != len(dropped) or len(rstreams) != len(cstreams) + len(dropped):
        return False
    if {s.get('index') for s in rstreams} != {s.get('index') for s in cstreams} | set(fourccs):
        return False
    originals = {}
    if any(f in PROTECTED for f in fourccs.values()):
        if protected is None:
            return False
        originals = protected()

    def same(stream):
        fourcc = fourccs[stream.get('index')]
        if fourcc in PROTECTED:
            return originals.get(stream.get('index')) == stream.get('codec_fourcc')
        return stream.get('codec_fourcc') == fourcc and not allowlisted(stream)
    return all(same(s) for s in rstreams if s.get('index') in fourccs)


PREDICATES = {
    'declared_size': declared_size,
    'strict_rejection': strict_rejection,
    'stts_short': stts_short_stream,
    'edit_duration': edit_duration_stream,
    'frag_start': frag_start_stream,
    'pcm_end': pcm_end_stream,
    'edit_unplaced': unplaced_format,
    'edit_discard': edit_discard,
    'ctts_shift': ctts_shift,
    'ctts_v0': ctts_v0,
    'trun_shift': trun_shift,
    'edit_snap': edit_snap,
    'pcm_grouping': pcm_grouping,
    'pcm_reference_units': pcm_reference_units,
    'aac_implicit_sbr': aac_implicit_sbr,
    'aac_lc_unnamed': aac_lc_unnamed,
    'quicktime_text_dropped': quicktime_text_dropped,
    'allowlist_dropped': allowlist_dropped,
    'dropped_entries': dropped_entries,
    'cover_art_streams': cover_art_streams,
    'chapter_text': chapter_text,
    'edit_multi': edit_multi,
    'text_end': text_end,
}


# ---------------------------------------------------------------- evidence

# The raw values a predicate takes from CoreAV (sample positions, sizes and
# media times; media header, `stts` sum, edit list, first `tfdt`, segment
# index end, movie header) are evidence only once this reader, written
# separately from CoreAV's parser, finds the same values in the file
# (ISO/IEC 14496-12 8.2.2, 8.4.2, 8.6.1, 8.6.6, 8.7, 8.8, 8.16.3). A CoreAV
# bug that moves a value and the evidence for it together is then caught,
# not excused. The reader returns None for anything it cannot read, and a
# predicate then does not apply.
MAX_FACT_SAMPLES = 1 << 24
MAX_FACT_TABLE_BYTES = 1 << 28
MAX_FACT_CHUNK_RUNS = 1 << 20
Sample = collections.namedtuple('Sample', 'pos size dts cts duration')
Run = collections.namedtuple('Run', 'pos size dts offset duration count sync')


class Facts:
    """What the reader found in one file: the movie header, the brand, and
    each track by `track_ID` (`tracks`), with its samples read lazily."""

    def __init__(self, path):
        self.path = pathlib.Path(path)
        self.brand, self.movie_timescale, self.movie_duration = None, None, None
        self.tracks, self.trex, self.sidx, self.moofs = {}, {}, [], []
        self._runs = {}
        # A compressed movie box (`cmov`), inflated: the tables are read
        # from it, the samples from the file.
        self._movie = None
        with open(self.path, 'rb') as f:
            self._f = f
            end = f.seek(0, 2)
            moov, box_start = None, 0
            for kind, start, stop in self._boxes(0, end):
                # Top-level boxes follow each other: each starts where the
                # one before it ends.
                box_start, previous_start = stop, box_start
                if kind == b'ftyp' and self.brand is None and stop - start >= 4:
                    # As the adapter writes a four-character code: printable
                    # ASCII as is, other bytes escaped.
                    self.brand = ''.join(chr(b) if 0x20 <= b < 0x7f else f'\\x{b:02x}'
                                         for b in self._read(start, 4))
                elif kind == b'moov' and moov is None:
                    moov = (start, stop)
                elif kind == b'moof':
                    self.moofs.append((previous_start, start, stop))
                elif kind == b'sidx' and stop - start <= MAX_FACT_TABLE_BYTES:
                    self.sidx.append(self._read(start, stop - start))
            if moov is None:
                raise ValueError('no moov')
            cmov = self._child(moov, b'cmov')
            if cmov is not None:
                self._movie = _inflate_movie(self._payload(self._child(cmov, b'dcom')),
                                             self._payload(self._child(cmov, b'cmvd')))
                self._f = io.BytesIO(self._movie)
                moov = next(((s, e) for k, s, e in self._boxes(0, len(self._movie)) if k == b'moov'), None)
                if moov is None:
                    raise ValueError('no moov in cmov')
            self._moov(*moov)
        self._f = None

    def _open_movie(self):
        """Where the movie box's tables are read from: the file, or the
        inflated movie box."""
        return io.BytesIO(self._movie) if self._movie is not None else open(self.path, 'rb')

    # -- box access
    def _boxes(self, start, stop):
        f, at = self._f, start
        while at + 8 <= stop:
            f.seek(at)
            head = f.read(16)
            size, header = int.from_bytes(head[:4], 'big'), 8
            if size == 1:
                size, header = int.from_bytes(head[8:16], 'big'), 16
            elif size == 0:
                size = stop - at
            if size < header or at + size > stop:
                return
            yield head[4:8], at + header, at + size
            at += size

    def _read(self, start, length):
        if length > MAX_FACT_TABLE_BYTES:
            raise ValueError('table too large')
        self._f.seek(start)
        data = self._f.read(length)
        if len(data) != length:
            raise ValueError('short read')
        return data

    def _child(self, box, kind):
        return next(((s, e) for k, s, e in self._boxes(*box) if k == kind), None) if box else None

    def _payload(self, box):
        return self._read(box[0], box[1] - box[0]) if box else None

    # -- movie box
    def _moov(self, start, stop):
        mvhd = self._payload(self._child((start, stop), b'mvhd'))
        self.movie_timescale, self.movie_duration = _times(mvhd)
        quicktime_handler = False
        for kind, s, e in self._boxes(start, stop):
            if kind == b'mvex':
                for k2, s2, e2 in self._boxes(s, e):
                    if k2 == b'trex' and e2 - s2 >= 24:
                        p = self._read(s2, 24)
                        self.trex[_u32(p, 4)] = (_u32(p, 12), _u32(p, 16), _u32(p, 20))
            if kind != b'trak':
                continue
            track = self._trak(s, e)
            if track is not None:
                quicktime_handler |= track.pop('quicktime_handler')
                self.tracks.setdefault(track['track_id'], track)
        self.fragmented = bool(self.trex) or bool(self.moofs)
        # QuickTime offsets are signed: brand `qt  `, or no `ftyp` and a
        # media handler of component type `mhlr`.
        self.quicktime = self.brand == 'qt  ' or (self.brand is None and quicktime_handler)

    def _trak(self, start, stop):
        trak = (start, stop)
        tkhd = self._payload(self._child(trak, b'tkhd'))
        mdia = self._child(trak, b'mdia')
        mdhd = self._payload(self._child(mdia, b'mdhd'))
        hdlr = self._payload(self._child(mdia, b'hdlr'))
        if not tkhd or len(tkhd) < 24 or not mdhd or not hdlr or len(hdlr) < 12:
            return None
        track_id = _u32(tkhd, 20 if tkhd[0] == 1 else 12)
        timescale, duration = _times(mdhd)
        # The last edit list of the track, as CoreAV keeps it.
        lists = [b for k, s, e in self._boxes(*trak) if k == b'edts'
                 for b in [(s2, e2) for k2, s2, e2 in self._boxes(s, e) if k2 == b'elst']]
        edits = _elst(self._payload(lists[-1])) if lists else []
        stbl = self._child(self._child(mdia, b'minf'), b'stbl')
        tables = {k: (s, e) for k, s, e in self._boxes(*stbl)} if stbl else {}
        return {'track_id': track_id, 'timescale': timescale, 'mdhd_duration': duration, 'edits': edits,
                'handler': hdlr[8:12], 'quicktime_handler': hdlr[4:8] == b'mhlr', 'tables': tables,
                'ctts_version': self._payload(tables[b'ctts'])[0] if b'ctts' in tables else None}

    # -- samples
    def runs(self, track_id):
        """The track's samples in decode order, as runs of regular samples
        (`Run`: each next sample `size` bytes and `duration` ticks further,
        with the same composition offset): those of the movie box, then
        those of its track fragments in file order. Raises ValueError for
        tables it cannot read."""
        if track_id in self._runs:
            if self._runs[track_id] is None:
                raise ValueError('unreadable track')
            return self._runs[track_id]
        self._runs[track_id] = None
        with self._open_movie() as f:
            self._f = f
            try:
                moov = list(self._table_runs(track_id))
            finally:
                self._f = None
        start = sum(r.count * r.duration for r in moov)
        with open(self.path, 'rb') as f:
            self._f = f
            try:
                fragments = list(self._fragment_runs(track_id, start))
            finally:
                self._f = None
        self._runs[track_id] = moov + fragments
        return self._runs[track_id]

    def samples(self, track_id):
        """The track's samples one by one (`Sample`), or None when the reader
        cannot read them; for small tracks (tests)."""
        try:
            runs = self.runs(track_id)
        except (ValueError, IndexError, KeyError, MemoryError):
            return None
        if sum(r.count for r in runs) > 1 << 20:
            return None
        return [Sample(r.pos + k * r.size, r.size, r.dts + k * r.duration, r.dts + k * r.duration + r.offset,
                       r.duration) for r in runs for k in range(r.count)]

    def sync(self, track_id):
        """Whether each sample of the track is a sync sample; for tests."""
        return [r.sync for r in self.runs(track_id) for _ in range(r.count)]

    def _table_runs(self, track_id):
        t = self.tracks[track_id]['tables']
        if b'stts' not in t:
            return
        stts = _entries(self._payload(t[b'stts']), 8)
        total = sum(_u32(e, 0) for e in stts)
        frame = None
        if b'stsz' in t:
            stsz = self._payload(t[b'stsz'])
            fixed, count = _u32(stsz, 4), _u32(stsz, 8)
            frame = self._quicktime_frame(t) if fixed == 1 else None
            if frame:
                # QuickTime uncompressed audio stores 1: a sample is one
                # frame, the channels times the width the code fixes, and
                # older files count the samples in bytes.
                if count == total * frame:
                    count = total
                fixed = frame
            constant = bool(fixed)
            if fixed:
                size_at = lambda i: fixed  # noqa: E731
            else:
                if len(stsz) < 12 + 4 * count:
                    raise ValueError('short stsz')
                size_at = lambda i: _u32(stsz, 12 + 4 * i)  # noqa: E731
        elif b'stz2' in t:
            stz2 = self._payload(t[b'stz2'])
            width, count, constant = stz2[7], _u32(stz2, 8), False
            if width not in (4, 8, 16) or len(stz2) < 12 + (count * width + 7) // 8:
                raise ValueError('bad stz2')
            if width == 4:
                size_at = lambda i: (stz2[12 + i // 2] >> (4 if i % 2 == 0 else 0)) & 15  # noqa: E731
            else:
                n = width // 8
                size_at = lambda i: int.from_bytes(stz2[12 + n * i:12 + n * i + n], 'big')  # noqa: E731
        else:
            raise ValueError('no sample sizes')
        if count != total:
            raise ValueError('sample tables disagree')
        if b'stco' in t:
            chunks = _entries(self._payload(t[b'stco']), 4)
        elif b'co64' in t:
            chunks = _entries(self._payload(t[b'co64']), 8)
        else:
            raise ValueError('no chunk offsets')
        table = _entries(self._payload(t[b'stsc']), 12) if b'stsc' in t else []
        if len(table) > MAX_FACT_CHUNK_RUNS:
            raise ValueError('stsc too long')
        stsc = [(_u32(e, 0), _u32(e, 4)) for e in table]
        if any(first < 1 for first, _ in stsc) or any(b[0] <= a[0] for a, b in zip(stsc, stsc[1:])):
            raise ValueError('bad stsc')
        if frame and stsc:
            # The same files may count each chunk's samples in bytes.
            listed = sum(n * ((stsc[i + 1][0] if i + 1 < len(stsc) else len(chunks) + 1) - first)
                         for i, (first, n) in enumerate(stsc))
            if listed == total * frame:
                stsc = [(first, n // frame) for first, n in stsc]
        # Sync samples (`stss`, 8.6.2): every sample without one.
        sync = None
        if b'stss' in t:
            sync = _entries(self._payload(t[b'stss']), 4)
        offsets, signed = [], False
        if b'ctts' in t:
            ctts = self._payload(t[b'ctts'])
            signed = ctts[0] == 1 or self.quicktime
            offsets = _entries(ctts, 8)
        offset_of = lambda e: int.from_bytes(e[4:8], 'big', signed=signed)  # noqa: E731
        stts_at, stts_left = 0, _u32(stts[0], 0) if len(stts) else 0
        ctts_at, ctts_left = 0, _u32(offsets[0], 0) if len(offsets) else 0
        runs = 0
        index, dts = 0, 0
        for i, (first, per_chunk) in enumerate(stsc):
            last = stsc[i + 1][0] - 1 if i + 1 < len(stsc) else len(chunks)
            for chunk in range(first, last + 1):
                if chunk > len(chunks):
                    raise ValueError('stsc past the chunk offsets')
                pos, left = int.from_bytes(chunks[chunk - 1], 'big'), per_chunk
                while left and index < total:
                    while stts_left == 0:
                        stts_at += 1
                        stts_left = _u32(stts[stts_at], 0)
                    if len(offsets):
                        while ctts_left == 0:
                            ctts_at += 1
                            if ctts_at >= len(offsets):
                                raise ValueError('ctts shorter than the samples')
                            ctts_left = _u32(offsets[ctts_at], 0)
                    delta, offset = _u32(stts[stts_at], 4), offset_of(offsets[ctts_at]) if len(offsets) else 0
                    # As many samples as share this size, duration and offset.
                    n = min(left, stts_left, ctts_left if len(offsets) else left, total - index)
                    size = size_at(index)
                    if not constant:
                        k = 1
                        while k < n and size_at(index + k) == size:
                            k += 1
                        n = k
                    is_sync = True
                    if sync is not None:
                        # Sample numbers count from 1, in increasing order.
                        at = bisect.bisect_left(_Numbers(sync), index + 1)
                        is_sync = at < len(sync) and _u32(sync[at], 0) == index + 1
                        if is_sync:
                            n = 1
                        elif at < len(sync):
                            n = min(n, _u32(sync[at], 0) - 1 - index)
                    runs += 1
                    if runs > MAX_FACT_SAMPLES:
                        raise ValueError('too many runs')
                    yield Run(pos, size, dts, offset, delta, n, is_sync)
                    pos, dts, index, left = pos + n * size, dts + n * delta, index + n, left - n
                    stts_left -= n
                    ctts_left -= n if len(offsets) else 0
        if index != total:
            raise ValueError('chunks hold fewer samples than the tables')

    def _quicktime_frame(self, tables):
        """The frame size of a QuickTime uncompressed audio description
        (QuickTime File Format, "Sound sample descriptions"): `bytesPerFrame`
        of a version 1 entry, else the channels times the width of the code
        (`sowt` and `twos` from the sample size); None for other codes."""
        if b'stsd' not in tables:
            return None
        stsd = self._payload(tables[b'stsd'])
        if len(stsd) < 8 + 36 or _u32(stsd, 4) < 1:
            return None
        entry = stsd[8:8 + _u32(stsd, 8)]
        code = entry[4:8]
        widths = {b'raw ': 1, b'in24': 3, b'in32': 4, b'fl32': 4, b'fl64': 8}
        if len(entry) < 36 or (code not in widths and code not in (b'sowt', b'twos')):
            return None
        version, channels, bits = int.from_bytes(entry[16:18], 'big'), int.from_bytes(entry[24:26], 'big'), \
            int.from_bytes(entry[26:28], 'big')
        if version == 1 and len(entry) >= 52 and _u32(entry, 44):
            return _u32(entry, 44)
        return channels * widths.get(code, (bits + 7) // 8)

    def _fragment_runs(self, track_id, start):
        """The track's samples in its track fragments (ISO/IEC 14496-12
        8.8.7, 8.8.8, 8.8.12): base offsets from `tfhd` (explicit, the
        `moof`, or the end of the data of the track fragment before, of any
        track), decode times from `tfdt` or on from where the samples
        before end (`start`, the movie box's samples' duration, for the
        first)."""
        if track_id not in self.trex or not self.moofs:
            return
        next_dts, samples = start, 0
        for moof_start, payload, stop in self.moofs:
            data_end = None
            for kind, s, e in self._boxes(payload, stop):
                if kind != b'traf':
                    continue
                traf = (s, e)
                tfhd = self._payload(self._child(traf, b'tfhd'))
                if not tfhd or len(tfhd) < 8:
                    raise ValueError('no tfhd')
                tid, flags, at = _u32(tfhd, 4), int.from_bytes(tfhd[1:4], 'big'), 8
                defaults = self.trex.get(tid, (0, 0, 0))
                base = None
                if flags & 0x1:
                    base = int.from_bytes(tfhd[at:at + 8], 'big')
                    at += 8
                if flags & 0x2:
                    at += 4
                d_duration, d_size, d_flags = defaults
                if flags & 0x8:
                    d_duration = _u32(tfhd, at)
                    at += 4
                if flags & 0x10:
                    d_size = _u32(tfhd, at)
                    at += 4
                if flags & 0x20:
                    d_flags = _u32(tfhd, at)
                    at += 4
                if base is None:
                    base = moof_start if (flags & 0x20000 or data_end is None) else data_end
                ours = tid == track_id
                if ours:
                    tfdt = self._payload(self._child(traf, b'tfdt'))
                    if tfdt:
                        next_dts = int.from_bytes(tfdt[4:12], 'big') if tfdt[0] == 1 else _u32(tfdt, 4)
                pos = base
                for k2, s2, e2 in self._boxes(*traf):
                    if k2 != b'trun':
                        continue
                    trun = self._payload((s2, e2))
                    tflags, count, at = int.from_bytes(trun[1:4], 'big'), _u32(trun, 4), 8
                    if tflags & 0x1:
                        pos = base + int.from_bytes(trun[at:at + 4], 'big', signed=True)
                        at += 4
                    first_flags = None
                    if tflags & 0x4:
                        first_flags = _u32(trun, at)
                        at += 4
                    width = 4 * bin(tflags & 0xf00).count('1')
                    if len(trun) < at + count * width:
                        raise ValueError('short trun')
                    samples += count
                    if samples > MAX_FACT_SAMPLES:
                        raise ValueError('too many fragment samples')
                    if not ours and not tflags & 0x200:
                        # Another track's run of default-size samples: only
                        # where its data ends matters.
                        pos += count * d_size
                        continue
                    if ours and not tflags & 0xf00 and count:
                        # Every sample takes the defaults (but the first's
                        # flags): one run.
                        first = first_flags if first_flags is not None else d_flags
                        yield Run(pos, d_size, next_dts, 0, d_duration, 1, not first & 0x10000)
                        if count > 1:
                            yield Run(pos + d_size, d_size, next_dts + d_duration, 0, d_duration, count - 1,
                                      not d_flags & 0x10000)
                        pos, next_dts = pos + count * d_size, next_dts + count * d_duration
                        continue
                    for k in range(count):
                        duration, size, offset = d_duration, d_size, 0
                        sample_flags = first_flags if k == 0 and first_flags is not None else d_flags
                        if tflags & 0x100:
                            duration = _u32(trun, at)
                            at += 4
                        if tflags & 0x200:
                            size = _u32(trun, at)
                            at += 4
                        if tflags & 0x400:
                            sample_flags = _u32(trun, at)
                            at += 4
                        if tflags & 0x800:
                            offset = int.from_bytes(trun[at:at + 4], 'big', signed=trun[0] == 1)
                            at += 4
                        if ours:
                            # sample_is_non_sync_sample (8.8.3.1).
                            yield Run(pos, size, next_dts, offset, duration, 1, not sample_flags & 0x10000)
                            next_dts += duration
                        pos += size
                data_end = pos

    def duration_sum(self, track_id):
        """The summed durations of the track's samples (`stts` and its
        fragments'), or None when the reader cannot read them."""
        try:
            return sum(r.count * r.duration for r in self.runs(track_id))
        except (ValueError, IndexError, KeyError, MemoryError):
            return None

    # -- checks the overlay gates on
    def stream_ok(self, candidate, stream):
        return evidence_stream(self, candidate, stream)

    def samples_ok(self, candidate, stream):
        return evidence_stream(self, candidate, stream) and evidence_samples(self, stream)

    def aac_plain_lc(self, stream):
        return evidence_plain_lc(self, stream)

    def outside_model(self, candidate, stream):
        return evidence_outside_model(self, candidate, stream)

    # -- derived values
    def audio_specific_config(self, track_id):
        """The AudioSpecificConfig of the track's first sample description,
        when it is `mp4a` with an `esds` (ISO/IEC 14496-14 5.6, 14496-1
        7.2.6.5 and 7.2.6.6); None otherwise."""
        tables = self.tracks.get(track_id, {}).get('tables', {})
        try:
            with self._open_movie() as f:
                self._f = f
                stsd = self._payload(tables.get(b'stsd'))
        except (ValueError, OSError):
            return None
        finally:
            self._f = None
        if not stsd or len(stsd) < 16 or _u32(stsd, 4) < 1:
            return None
        entry = stsd[8:8 + _u32(stsd, 8)]
        if entry[4:8] != b'mp4a' or len(entry) < 36:
            return None
        # The audio entry's fields, longer in QuickTime versions 1 and 2
        # (an ISO version 1 entry keeps the version 0 layout); QuickTime
        # keeps the `esds` inside a `wave` box (QuickTime File Format, "Sound
        # sample description extensions").
        extension = {1: 16, 2: 36}.get(int.from_bytes(entry[16:18], 'big'), 0)
        for at in dict.fromkeys((36 + extension, 36)):
            found = _find_esds(entry, at, 2)
            if found is not None:
                return found
        return None

    def first_tfdt(self, track_id):
        """The decode time of the track's first track fragment (its `tfdt`)."""
        try:
            with open(self.path, 'rb') as f:
                self._f = f
                for _, payload, stop in self.moofs:
                    for kind, s, e in self._boxes(payload, stop):
                        if kind != b'traf':
                            continue
                        tfhd = self._payload(self._child((s, e), b'tfhd'))
                        if tfhd and _u32(tfhd, 4) == track_id:
                            tfdt = self._payload(self._child((s, e), b'tfdt'))
                            if not tfdt:
                                return None
                            return int.from_bytes(tfdt[4:12], 'big') if tfdt[0] == 1 else _u32(tfdt, 4)
        except (ValueError, IndexError):
            return None
        finally:
            self._f = None
        return None

    def sidx_end(self, track_id, timescale):
        """The end of the first segment index of the track in its timescale:
        its earliest presentation time plus its subsegment durations."""
        for p in self.sidx:
            if len(p) < 24 or _u32(p, 4) != track_id or _u32(p, 8) != timescale:
                continue
            wide = p[0] == 1
            at = 12
            earliest = int.from_bytes(p[at:at + (8 if wide else 4)], 'big')
            at += 16 if wide else 8
            count = int.from_bytes(p[at + 2:at + 4], 'big')
            at += 4
            if at + 12 * count > len(p):
                return None
            return earliest + sum(_u32(p, at + 12 * i + 4) for i in range(count))
        return None


def _descriptor(data, at):
    """(tag, payload start, payload end) of the descriptor at `at`
    (ISO/IEC 14496-1 8.3.3: a tag, then a size in 7-bit groups)."""
    if at + 2 > len(data):
        raise ValueError('short descriptor')
    tag, size, at = data[at], 0, at + 1
    for _ in range(4):
        byte = data[at]
        at += 1
        size = size << 7 | byte & 0x7f
        if not byte & 0x80:
            break
    if at + size > len(data):
        raise ValueError('descriptor past its box')
    return tag, at, at + size


def _find_esds(data, at, depth):
    """The DecoderSpecificInfo of the `esds` among the boxes of `data` from
    `at`, looking inside `wave` boxes `depth` levels down; None without one."""
    while at + 8 <= len(data):
        size, kind = _u32(data, at), data[at + 4:at + 8]
        if size < 8 or at + size > len(data):
            return None
        if kind == b'esds':
            return _decoder_specific_info(data[at + 12:at + size])
        if kind == b'wave' and depth > 0:
            found = _find_esds(data[at + 8:at + size], 0, depth - 1)
            if found is not None:
                return found
        at += size
    return None


def _decoder_specific_info(esds):
    """The DecoderSpecificInfo bytes of an `esds` payload (after its version
    and flags), or None."""
    try:
        tag, at, end = _descriptor(esds, 0)
        if tag != 0x03:
            return None
        # ES_ID, the flags, then the optional fields in their order.
        flags = esds[at + 2]
        at += 3 + (2 if flags & 0x80 else 0)
        if flags & 0x40:
            at += 1 + esds[at]
        at += 2 if flags & 0x20 else 0
        tag, at, end = _descriptor(esds, at)
        if tag != 0x04:
            return None
        tag, at, end = _descriptor(esds, at + 13)
        return esds[at:end] if tag == 0x05 else None
    except (ValueError, IndexError):
        return None


AAC_RATES = (96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350)


def evidence_plain_lc(facts, stream):
    """The file's AudioSpecificConfig for the stream is a plain AAC LC one
    (ISO/IEC 14496-3 1.6.2.1): two bytes, object type 2, the sampling rate
    CoreAV reports, and no extension (so no explicit SBR or PS signalling)."""
    if facts is None:
        return False
    asc = facts.audio_specific_config(props(stream).get('track_id'))
    if asc is None or len(asc) != 2:
        return False
    object_type, rate_index, extension = asc[0] >> 3, (asc[0] & 7) << 1 | asc[1] >> 7, asc[1] & 1
    return (object_type == 2 and rate_index < len(AAC_RATES) and AAC_RATES[rate_index] == stream.get('sample_rate')
            and not extension)


def evidence_outside_model(facts, candidate, stream):
    """The stream's edit list, as stored in the file, is outside CoreAV's
    model (docs/design/edit-lists.md, decision 1): a rate other than 1, a
    media time below -1, media edits whose windows do not increase without
    overlap, a media edit of duration 0 other than the simple model's
    open-ended edit in a movie with fragments, or several media edits in a
    movie with fragments."""
    if facts is None or not evidence_movie(facts, candidate):
        return False
    track = facts.tracks.get(props(stream).get('track_id'))
    if track is None or not facts.movie_timescale or not track['timescale']:
        return False
    entries = track['edits']
    media = [e for e in entries if e[1] != -1]
    if any(rate != 1 << 16 or media_time < -1 for _, media_time, rate in entries):
        return True
    if facts.fragmented and len(media) > 1:
        return True
    open_ended = facts.fragmented and _simple_edits(entries) is not None
    end = None
    for duration, media_time, _ in media:
        if duration == 0 and not open_ended:
            return True
        start = Fraction(media_time)
        if end is not None and start < end:
            return True
        end = start + Fraction(duration * track['timescale'], facts.movie_timescale)
    return False


def _inflate_movie(dcom, cmvd):
    """The movie box a `cmov` holds (QuickTime File Format, "Compressed
    movie resources"): `dcom` names zlib, `cmvd` gives the inflated size
    and the zlib data."""
    if not dcom or dcom[:4] != b'zlib' or not cmvd or len(cmvd) < 4:
        raise ValueError('unsupported compressed movie')
    size = _u32(cmvd, 0)
    if size > MAX_FACT_TABLE_BYTES:
        raise ValueError('compressed movie too large')
    inflater = zlib.decompressobj()
    try:
        movie = inflater.decompress(cmvd[4:], size + 1)
    except zlib.error as error:
        raise ValueError('bad compressed movie') from error
    if len(movie) != size:
        raise ValueError('compressed movie size')
    return movie


class _entries:
    """The entries of a full box's table, read in place: a count after the
    version and flags, then `count` entries of `width` bytes each."""

    def __init__(self, payload, width):
        if not payload or len(payload) < 8:
            raise ValueError('short table')
        self.payload, self.width, self.count = payload, width, _u32(payload, 4)
        if 8 + self.count * width > len(payload):
            raise ValueError('table shorter than its count')

    def __len__(self):
        return self.count

    def __getitem__(self, i):
        if not 0 <= i < self.count:
            raise IndexError('entry')
        return self.payload[8 + self.width * i:8 + self.width * (i + 1)]


class _Numbers:
    """The 32-bit numbers of a table's entries, for `bisect`."""

    def __init__(self, table):
        self.table = table

    def __len__(self):
        return len(self.table)

    def __getitem__(self, i):
        return _u32(self.table[i], 0)


def _u32(data, at):
    if at + 4 > len(data):
        raise ValueError('short box')
    return int.from_bytes(data[at:at + 4], 'big')


def _times(payload):
    """(timescale, duration) of a `mvhd` or `mdhd` payload; an all-ones
    duration (unknown) is 0, as CoreAV keeps it."""
    if not payload or len(payload) < 24:
        return None, None
    if payload[0] == 1:
        if len(payload) < 36:
            return None, None
        scale, duration, ones = _u32(payload, 20), int.from_bytes(payload[24:32], 'big'), (1 << 64) - 1
    else:
        scale, duration, ones = _u32(payload, 12), _u32(payload, 16), (1 << 32) - 1
    return scale, 0 if duration == ones else duration


def _elst(payload):
    """Edit list entries as stored: (segment_duration, media_time, rate)."""
    if not payload or len(payload) < 8:
        return []
    version, count = payload[0], _u32(payload, 4)
    width = 20 if version == 1 else 12
    edits = []
    for at in range(8, min(8 + count * width, len(payload) - width + 1), width):
        n = 8 if version == 1 else 4
        duration = int.from_bytes(payload[at:at + n], 'big')
        media_time = int.from_bytes(payload[at + n:at + 2 * n], 'big', signed=True)
        edits.append((duration, media_time, int.from_bytes(payload[at + 2 * n:at + 2 * n + 4], 'big', signed=True)))
    return edits


def read_facts(media_dir, file):
    """The file's `Facts`, or None when the reader cannot read it."""
    try:
        return Facts(pathlib.Path(media_dir) / file)
    except (OSError, ValueError, IndexError, KeyError, MemoryError, zlib.error):
        return None


def evidence_movie(facts, candidate):
    """The candidate's movie header values and brand are the file's, and no
    two of its streams (dropped ones included) claim the same track."""
    p = props(candidate)
    ids = [props(s).get('track_id') for s in candidate.get('streams', [])
           if not props(s).get('dropped')] + [d.get('track_id') for d in p.get('dropped') or []]
    return (facts is not None and p.get('movie_timescale') == facts.movie_timescale
            and p.get('raw_movie_duration_ticks') == facts.movie_duration
            and p.get('major_brand', facts.brand) == facts.brand and len(ids) == len(set(ids)))


def _simple_edits(entries):
    """CoreAV's simple edit list from the stored entries: leading empty
    edits, then one media edit at rate 1; None for another shape."""
    empty, media = 0, [e for e in entries if e[1] != -1]
    for e in entries:
        if e[1] != -1:
            break
        empty += e[0]
    if len(media) != 1 or entries[-1][1] == -1 or media[0][2] != 1 << 16:
        return None
    return {'empty': empty, 'media_time': media[0][1], 'segment': media[0][0]}


def evidence_stream(facts, candidate, stream):
    """The raw values CoreAV reports for a stream (kept or dropped, by
    `track_id`) are the file's: timescale, media header duration, edit list,
    `ctts` version and signedness, `stts` sum, first `tfdt` and segment index
    end."""
    if facts is None or not evidence_movie(facts, candidate):
        return False
    p = props(stream)
    track = facts.tracks.get(p.get('track_id'))
    if track is None or stream.get('timescale') != track['timescale'] or 'edits' not in p:
        return False
    # A kept stream reports every value checked here; a missing one is not
    # evidence.
    if not p.get('dropped') and not all(k in p for k in ('mdhd_duration_ticks', 'edit_entries', 'ctts_version',
                                                         'stts_duration_ticks', 'media_edit_duration_ticks',
                                                         'presentation_offset_ticks')):
        return False
    if 'mdhd_duration_ticks' in p and p['mdhd_duration_ticks'] != track['mdhd_duration']:
        return False
    entries = track['edits']
    if 'edit_entries' in p and [tuple(e) for e in p['edit_entries'] or []] != [tuple(e) for e in entries]:
        return False
    # The list's shape, as the edit model sorts it (docs/design/edit-lists.md,
    # decision 1): a CoreAV bug that sorts it otherwise changes the
    # duration rule that applies.
    edits = p.get('edits')
    simple = _simple_edits(entries)
    if not entries:
        kind = 'none'
    elif simple is not None:
        kind = 'simple'
    elif evidence_outside_model(facts, candidate, stream):
        kind = 'unsupported'
    else:
        kind = 'multiple'
    if (kind == 'simple') != isinstance(edits, dict) or (kind != 'simple' and edits != kind):
        return False
    if isinstance(edits, dict) and {k: edits.get(k) for k in ('empty', 'media_time', 'segment')} != simple:
        return False
    # The media edits' duration in media ticks, which Decision 13 caps the
    # stream with: reported exactly when there is a media edit of known
    # duration (not the open-ended edit of a fragmented movie, nor a list
    # outside the model), and the presentation offset of the leading empty
    # edits.
    pair = fraction(p.get('media_edit_duration_ticks'))
    media = [e for e in entries if e[1] != -1]
    open_ended = kind == 'simple' and facts.fragmented and simple['segment'] == 0
    if not facts.movie_timescale:
        return False
    if p.get('dropped'):
        # The adapter reports neither value for a dropped stream.
        pass
    elif kind in ('simple', 'multiple') and not open_ended:
        if pair != Fraction(sum(e[0] for e in media) * track['timescale'], facts.movie_timescale):
            return False
    elif pair is not None:
        # No edit list, an open-ended edit or a list outside the model: no
        # media edit duration caps the stream.
        return False
    offset = fraction(p.get('presentation_offset_ticks'))
    if kind != 'unsupported' and not p.get('dropped'):
        leading = 0
        for e in entries:
            if e[1] != -1:
                break
            leading += e[0]
        if offset != Fraction(leading * track['timescale'], facts.movie_timescale):
            return False
    if 'ctts_version' in p and p['ctts_version'] != track['ctts_version']:
        return False
    if p.get('ctts_signed') is not None and p['ctts_signed'] != (track['ctts_version'] == 1 or facts.quicktime):
        return False
    if isinstance(p.get('fragment_start_ticks'), int) and p['fragment_start_ticks'] != facts.first_tfdt(track['track_id']):
        return False
    if 'sidx_end_ticks' in p and p['sidx_end_ticks'] != facts.sidx_end(track['track_id'], track['timescale']):
        return False
    if isinstance(p.get('stts_duration_ticks'), int) and p['stts_duration_ticks'] != facts.duration_sum(
            track['track_id']):
        return False
    return True


def evidence_samples(facts, stream):
    """Every packet CoreAV lists for a stream (runs walked packet by packet,
    tools/oracle/SPEC.md "Runs") is the file's sample at that place in
    decode order: position, size, media decode and composition times,
    duration and (but for H.264 and AV1) the sync flag; and no sample is left out or
    added."""
    if facts is None:
        return False
    try:
        runs = facts.runs(props(stream).get('track_id'))
    except (ValueError, IndexError, KeyError, MemoryError):
        return False
    # The keyframe flag is the sync sample table's, except for H.264 and AV1,
    # whose flag the adapter reads from the coded data (tools/oracle/SPEC.md,
    # "Keyframes").
    stss_keyframes = stream.get('codec_name') not in ('h264', 'av1')
    # Walk both lists of runs together, a common stretch at a time.
    file_runs = iter(runs)
    head, left = None, 0
    for p in stream.get('packets', []):
        pos, size, dts, cts, duration = (p.get(k) for k in ('pos', 'size', 'media_dts', 'media_cts', 'duration'))
        count = p.get('run', 1)
        if not all(isinstance(v, int) for v in (pos, size, dts, cts, count)) or count < 1:
            return False
        if count > 1 and not isinstance(duration, int):
            return False
        while count:
            if left == 0:
                head = next(file_runs, None)
                if head is None:
                    return False
                left = head.count
                at = (head.pos, head.dts)
            if (at != (pos, dts) or head.size != size or cts - dts != head.offset
                    or (duration is not None and head.duration != duration)
                    or (stss_keyframes and p.get('keyframe') != head.sync)):
                return False
            n = min(left, count)
            step = head.duration
            at, pos, dts, cts = (at[0] + n * size, at[1] + n * step), pos + n * size, dts + n * step, cts + n * step
            left, count = left - n, count - n
    return left == 0 and next(file_runs, None) is None


# ---------------------------------------------------------------- overlay

def stream_item(detail, reference, candidate, defects, media_dir='', file='', facts=None):
    """Returns (status, kd ids, uncovered paths, covered paths) for one
    file's stream item; covered paths map each differing field to the
    entries that cover it.
    `facts` returns the file's `Facts` (read from `media_dir` when not
    given); an entry that takes raw values from CoreAV applies only to
    streams whose values the reader finds in the file."""
    ids, uncovered = set(), []
    protected = functools.partial(protected_formats, media_dir, file) if media_dir else None
    covers = functools.cache(functools.partial(cover_art, media_dir, file)) if media_dir else None
    chapters = functools.cache(functools.partial(chapter_tracks, media_dir, file)) if media_dir else None
    count = functools.cache(lambda: stream_count_entries(reference, candidate, protected, covers))
    if facts is None:
        facts = functools.cache(lambda: read_facts(media_dir, file) if media_dir else None)
    cstreams = paired(reference, candidate)

    def checked(index):
        """The candidate stream paired with reference position `index` has
        the file's raw values."""
        f = facts()
        return f is not None and index < len(cstreams) and bool(cstreams[index]) and f.stream_ok(
            candidate, cstreams[index])

    def all_checked(unplaced_ok=False):
        """Every stream, dropped ones included, has the file's raw values;
        with `unplaced_ok`, a stream CoreAV leaves unplaced needs only an
        edit list the file shows is outside the model."""
        f = facts()
        if f is None:
            return False
        for c in _with_dropped(candidate).get('streams', []):
            if unplaced_ok and props(c).get('edits') == 'unsupported':
                if not f.outside_model(candidate, c):
                    return False
            elif not f.stream_ok(candidate, c):
                return False
        if unplaced_ok:
            # Dropped streams without timing that CoreAV leaves unplaced.
            for d in props(candidate).get('dropped') or []:
                if d.get('edits') == 'unsupported' and not f.outside_model(candidate, {'properties': d}):
                    return False
        return True

    def duration_kd(index):
        for kd_id, check in (('KD-STTS-SHORT', stts_short_stream), ('KD-EDIT-DURATION', edit_duration_stream),
                             ('KD-FRAG-START', frag_start_stream), ('KD-PCM-END', pcm_end_stream),
                             ('KD-TEXT-END', lambda r, c, i: text_end(r, c, i, facts))):
            if check(reference, candidate, index):
                return kd_id if checked(index) else None
        return None

    # The entries that cover each differing field, for the compat check
    # (scripts/compat_check.py), which counts a compat key as covered only
    # where the field it follows was (#219).
    paths = {}
    for diff in detail['gating_differences']:
        path = diff['path']
        found = set()
        if path == 'probe_ok' and strict_rejection(reference, candidate, defects()):
            found = {'KD-STRICT'}
        elif path == 'streams/count' and count():
            found = set(count())
        elif path == 'format_duration_us' and duration_format(reference, candidate) and all_checked():
            found = set(duration_format(reference, candidate))
        elif path == 'format_duration_us' and unplaced_format(reference, candidate) and all_checked(True):
            found = set(unplaced_format(reference, candidate))
        elif (path.startswith('streams/') and path.endswith('/duration_ts') and path.count('/') == 2
              and duration_kd(int(path.split('/')[1]))):
            found = {duration_kd(int(path.split('/')[1]))}
        elif (path.startswith('streams/') and path.count('/') == 2
              and path.rsplit('/', 1)[1] in ('width', 'height', 'display_aspect_ratio')
              and declared_size(media_dir, file, reference, candidate, int(path.split('/')[1]))):
            found = {'KD-DECLARED-SIZE'}
        elif (path.startswith('streams/') and path.count('/') == 2 and path.rsplit('/', 1)[1] in ('kind', 'codec_name')
              and chapter_text(reference, candidate, int(path.split('/')[1]), chapters)):
            found = {'KD-CHAPTER-TEXT'}
        elif path.startswith('streams/') and path.endswith('/profile') and path.count('/') == 2:
            index = int(path.split('/')[1])
            lc = facts() is not None and facts().aac_plain_lc(by_index(candidate, index) or {})
            if lc and aac_implicit_sbr(reference, candidate, index):
                found = {'KD-AAC-IMPLICIT-SBR'}
            elif lc and aac_lc_unnamed(reference, candidate, index):
                found = {'KD-AAC-LC-UNNAMED'}
        if found:
            ids |= found
            # The values the entries were checked on, so that the compat
            # check can require its own output to be these values.
            paths[path] = {'kd': sorted(found), 'candidate': diff.get('candidate'),
                           'reference': diff.get('reference')}
        else:
            uncovered.append(path)
    return ('kd' if not uncovered else 'uncovered'), ids, uncovered, paths


def paired(reference, candidate):
    """The candidate stream paired with each reference stream, as the
    comparison pairs them: by `index` when every stream has one, else by
    position; `{}` when there is none."""
    rstreams, cstreams = reference.get('streams', []), candidate.get('streams', [])
    if all(isinstance(s.get('index'), int) for s in rstreams + cstreams):
        by = {s['index']: s for s in cstreams}
        return [by.get(s['index'], {}) for s in rstreams]
    return [cstreams[i] if i < len(cstreams) else {} for i in range(len(rstreams))]


def pair(reference_packets, candidate_packets):
    """Reference index -> candidate packet, pairing by position in occurrence order."""
    queues = collections.defaultdict(collections.deque)
    for p in candidate_packets:
        queues[p.get('pos')].append(p)
    return [queues[p.get('pos')].popleft() if queues[p.get('pos')] else None for p in reference_packets]


def packet_units(detail, reference, candidate, defects, stream_reference=None, protected=None, breakdown=None,
                 covers=None, chapters=None, edit_tracks=None, facts=None):
    """Returns (covered counts by KD id, uncovered count, notes) for one file,
    in the comparison's unit (one candidate packet per PCM sample).
    `stream_reference` returns the reference's streams-level document, whose
    stream properties the packets-level one lacks; `covers` returns the
    file's cover pictures (`cover_art`), `chapters` its chapter tracks
    (`chapter_tracks`) and `edit_tracks` a track's edit list by position
    (`edit_track`). When the differences are
    attributed to streams, `breakdown['streams']` gets each stream's
    (covered, uncovered). `facts` returns the file's `Facts`: an entry that
    takes media times from CoreAV applies only to a stream whose every
    packet the reader finds in the file (`Facts.samples_ok`)."""
    disagree = detail['packets_disagree']
    diffs = detail['gating_differences']
    if any(d['path'] == 'probe_ok' for d in diffs):
        if strict_rejection(reference, candidate, defects()):
            return {'KD-STRICT': disagree} if disagree else {}, 0, []
        return {}, disagree, ['probe_ok']
    covered, uncovered, notes = collections.Counter(), 0, []
    # The stream count is no packet unit; KD-QT-TEXT, KD-NOT-ALLOWED or
    # KD-COVER-ART explains it at the streams level. The reference packets
    # of the dropped streams and of the cover pictures, which the comparison
    # counts without listing them, are covered by the same entries here.
    if any(d['path'] == 'streams/count' for d in diffs) and stream_reference is not None:
        streams = stream_reference()
        entries = stream_count_entries(streams, candidate, protected, covers)
        if entries:
            diffs = [d for d in diffs if d['path'] != 'streams/count']
            covers_found = ((cover_art_streams(streams, candidate, covers()) or set())
                            if 'KD-COVER-ART' in entries else set())
            # Each dropped stream's packets go to the entry that explains it.
            explains = dropped_entries(without_streams(streams, covers_found), candidate, protected) or {}
            # A cover picture is one packet; anything else stays uncovered.
            if all(len(s.get('packets', [])) == 1 for s in reference.get('streams', [])
                   if s.get('index') in covers_found):
                explains.update(dict.fromkeys(covers_found, 'KD-COVER-ART'))
            for s in reference.get('streams', []):
                missing = len(s.get('packets', []))
                if s.get('index') in explains and missing:
                    covered[explains[s.get('index')]] += missing
                    disagree -= missing
    units = collections.defaultdict(list)
    # A unit stands for one packet, except the unpaired samples of a run
    # (`packets/extra_run`, tools/oracle/SPEC.md "Runs"): its count.
    weight = {}
    extra_index = collections.Counter()
    for d in diffs:
        parts = d['path'].split('/')
        if len(parts) >= 4 and parts[0] == 'streams' and parts[2] == 'packets':
            stream = int(parts[1])
            if parts[3] in ('extra', 'missing', 'extra_run'):
                extra_index[(stream, parts[3])] += 1
                key = (stream, parts[3], extra_index[(stream, parts[3])])
                units[key].append(d)
                if parts[3] == 'extra_run':
                    weight[key] = (d.get('candidate') or {}).get('unpaired', 0)
            else:
                units[(stream, 'paired', int(parts[3]))].append(d)
        else:
            return dict(covered), disagree, [d['path']]
    total = sum(weight.get(key, 1) for key in units)
    if total != disagree:
        return dict(covered), disagree, [f'unit count {total} differs from {disagree}']
    # The comparison pairs streams by index (tools/oracle, `paired_streams`);
    # a unit's stream number is the reference's position.
    rstreams, cstreams = reference.get('streams', []), paired(reference, candidate)
    facts = facts or (lambda: None)
    checked = {i for i in {k[0] for k in units}
               if i < len(cstreams) and cstreams[i] and facts() is not None
               and facts().samples_ok(candidate, cstreams[i])}
    pcm = {i for i in checked
           if i < len(rstreams) and pcm_grouping(rstreams[i], cstreams[i], candidate.get('properties'))}
    pairs = {i: pair(rstreams[i].get('packets', []), cstreams[i].get('packets', []))
             for i in {k[0] for k in units} if i < len(rstreams) and i < len(cstreams)}

    @functools.cache
    def shifted(stream, field):
        """A stream-wide shift of `field` shows on every paired packet: none
        agrees on it directly."""
        return all(r.get(field) != c.get(field) for r, c in zip(rstreams[stream].get('packets', []), pairs[stream])
                   if c is not None)
    snaps = {}
    runs = {i for i in {k[0] for k in units}
            if i < len(cstreams) and any('run' in p for p in cstreams[i].get('packets', []))}
    # Chapter text streams, of which `ffprobe` lists no packet.
    streams_level = functools.cache(stream_reference) if stream_reference is not None else None
    chapter = {i for i in checked
               if i < len(rstreams) and not rstreams[i].get('packets') and streams_level is not None
               and chapter_text(streams_level(), candidate, rstreams[i].get('index'), chapters)}
    # Streams whose edit list CoreAV applies as several edits, with every
    # packet on both sides as the two models say.
    multi = {i for i in checked
             if i < len(rstreams) and edit_tracks is not None
             and props(cstreams[i]).get('edits') == 'multiple'
             and edit_multi(rstreams[i], cstreams[i], edit_tracks(rstreams[i].get('index')))}
    by_stream = collections.defaultdict(lambda: (collections.Counter(), [0]))
    for (stream, kind, index), unit_diffs in units.items():
        n = weight.get((stream, kind, index), 1)
        before_covered, before_uncovered = collections.Counter(covered), uncovered
        if kind == 'paired' and stream not in snaps:
            snaps[stream] = (edit_snap_shift(cstreams[stream]), ctts_dts_shift(cstreams[stream]),
                             ctts_v0_shift(cstreams[stream], candidate.get('properties')),
                             trun_pts_shift(cstreams[stream]))
        if stream in pcm:
            covered['KD-PCM'] += n
        elif stream in multi:
            covered['KD-EDIT-MULTI'] += n
        elif (stream in chapter and kind == 'extra'
              and all(edit_mapped(cstreams[stream], d.get('candidate') or {}) for d in unit_diffs)):
            covered['KD-CHAPTER-TEXT'] += 1
        elif stream not in checked:
            # The media times the entries below take from CoreAV are not
            # the file's.
            uncovered += n
            if len(notes) < 5:
                notes.append(unit_diffs[0]['path'])
        elif kind == 'extra_run' and all(run_discarded(cstreams[stream], d['candidate']) for d in unit_diffs):
            covered['KD-EDIT-DISCARD'] += n
        elif stream in runs and kind != 'extra':
            # Run streams are constant-size PCM: KD-PCM, or KD-EDIT-DISCARD
            # for samples outside the edit, apply.
            uncovered += n
            if len(notes) < 5:
                notes.append(unit_diffs[0]['path'])
        elif kind == 'extra' and all(edit_discard(cstreams[stream], d['candidate']) for d in unit_diffs):
            covered['KD-EDIT-DISCARD'] += 1
        elif (kind == 'paired' and all(d['path'].endswith('/dts') for d in unit_diffs) and shifted(stream, 'dts')
              and ctts_shift(cstreams[stream], rstreams[stream]['packets'][index], pairs[stream][index],
                             snaps[stream][1])):
            covered['KD-CTTS'] += 1
        elif (kind == 'paired' and all(d['path'].endswith(('/pts', '/dts')) for d in unit_diffs)
              and shifted(stream, 'dts') and ctts_v0(cstreams[stream], rstreams[stream]['packets'][index], pairs[stream][index],
                          snaps[stream][2])):
            covered['KD-CTTS-UNSIGNED'] += 1
        elif (kind == 'paired' and all(d['path'].endswith('/pts') for d in unit_diffs) and shifted(stream, 'pts')
              and trun_shift(cstreams[stream], rstreams[stream]['packets'][index], pairs[stream][index],
                             snaps[stream][3])):
            covered['KD-TRUN-SHIFT'] += 1
        elif (kind == 'paired' and all(d['path'].endswith(('/pts', '/dts')) for d in unit_diffs)
              and shifted(stream, 'pts') and edit_snap(cstreams[stream], rstreams[stream]['packets'][index], pairs[stream][index],
                            *snaps[stream][:2])):
            covered['KD-EDIT-SNAP'] += 1
        else:
            uncovered += 1
            if len(notes) < 5:
                notes.append(unit_diffs[0]['path'])
        by_stream[stream][0].update(covered - before_covered)
        by_stream[stream][1][0] += uncovered - before_uncovered
    if breakdown is not None:
        breakdown['streams'] = {i: (dict(c), u[0]) for i, (c, u) in by_stream.items()}
    return dict(covered), uncovered, notes


def stream_weights(diffs):
    """Per stream position: (disagreeing, extra) packets in the comparison's
    unit, from its listed differences. A paired packet that differs in
    several fields is listed once per field and counts once, as in
    `packet_units`."""
    disagree, extra = collections.Counter(), collections.Counter()
    paired_seen = set()
    for d in diffs:
        parts = d['path'].split('/')
        if len(parts) >= 4 and parts[0] == 'streams' and parts[2] == 'packets' and parts[1].isdigit():
            stream = int(parts[1])
            if parts[3] in ('extra', 'missing', 'extra_run'):
                n = (d.get('candidate') or {}).get('unpaired', 0) if parts[3] == 'extra_run' else 1
                disagree[stream] += n
                if parts[3] != 'missing':
                    extra[stream] += n
            elif (stream, parts[3]) not in paired_seen:
                paired_seen.add((stream, parts[3]))
                disagree[stream] += 1
    return disagree, extra


def reference_packet_units(detail, reference, candidate, per_sample, breakdown, facts=None):
    """One file's packets in the reference's packet unit
    (docs/design/stage3-report.md, "Packet unit"): `(items, direct, covered,
    uncovered, notes)`. `per_sample` is `packet_units`'s result, or None for
    a file that agrees.

    The two units differ only for constant-size PCM streams, where the
    candidate lists runs: their per-sample counts are taken out and their
    `pcm_reference_units` put in. When `packet_units` could not attribute
    the file's differences to streams, every unit that does not agree
    directly stays uncovered, as in the per-sample count. As in
    `packet_units`, a stream whose packets the reader (`facts`) does not
    find in the file gets no known divergence."""
    covered, uncovered, notes = per_sample or ({}, 0, [])
    items, direct = detail['packets_agree'] + detail['packets_disagree'], detail['packets_agree']
    covered = collections.Counter(covered)
    rstreams, cstreams = reference.get('streams', []), paired(reference, candidate)
    runs = [i for i in range(min(len(rstreams), len(cstreams)))
            if any('run' in p for p in cstreams[i].get('packets', []))]
    if not runs:
        return items, direct, dict(covered), uncovered, notes
    attributed = per_sample is None or 'streams' in breakdown
    split = breakdown.get('streams', {})
    disagree, extra = stream_weights(detail.get('gating_differences', []))
    notes = [n for n in notes if not any(n.startswith(f'streams/{i}/') for i in runs)]
    for i in runs:
        sample_items = len(rstreams[i].get('packets', [])) + extra[i]
        items -= sample_items
        direct -= sample_items - disagree[i]
        if i in split:
            covered.subtract(split[i][0])
            uncovered -= split[i][1]
        elif per_sample is not None and not attributed:
            uncovered -= disagree[i]
        n, agree, kd, missed, examples = pcm_reference_units(dict(rstreams[i], index=i), cstreams[i],
                                                              candidate.get('properties'))
        items += n
        direct += agree
        f = facts() if facts is not None else None
        if attributed and per_sample is not None and (f is None or not f.samples_ok(candidate, cstreams[i])):
            uncovered += n - agree
            if len(notes) < 5:
                notes.append(f'streams/{i}/packets')
        elif attributed:
            covered.update(kd)
            uncovered += missed
            notes += examples[:max(0, 5 - len(notes))]
        else:
            uncovered += n - agree
    if (direct < 0 or uncovered < 0 or any(v < 0 for v in covered.values())
            or direct + sum(covered.values()) + uncovered != items):
        raise SystemExit(f"{detail['file']}: the reference packet unit does not add up")
    return items, direct, {k: v for k, v in covered.items() if v}, uncovered, notes


def overlay(report, reference_dir, candidate_dir, media_dir):
    entries = {e['id'] for e in json.loads(KD_FILE.read_text())['entries']}
    result = {'schema': 'coreav/kd-overlay/1', 'levels': {}}
    for level in report['levels']:
        name = level['level']
        groups = {scope: {'items': 0, 'direct': 0, 'via_kd': 0, 'uncovered': 0, 'by_kd': collections.Counter(),
                          'uncovered_items': [], 'covered_items': []} for scope in ('in_scope', 'all')}
        if name == 'packets':
            for g in groups.values():
                g['unit'] = 'reference_packet'
                g['per_sample'] = {'items': 0, 'direct': 0, 'via_kd': 0, 'uncovered': 0,
                                   'by_kd': collections.Counter()}
        for detail in level['details']:
            if detail.get('reference_failure'):
                continue
            file = detail['file']
            reference, candidate = load(reference_dir, file, name), load(candidate_dir, file, name)
            defects = functools.partial(media_defects, media_dir, file)
            facts = functools.cache(functools.partial(read_facts, media_dir, file))
            scopes = ['all'] + (['in_scope'] if file.endswith(IN_SCOPE) else [])
            if name == 'streams':
                if detail['agrees']:
                    status, ids, missing, covered_paths = 'direct', set(), [], {}
                else:
                    status, ids, missing, covered_paths = stream_item(detail, reference, candidate, defects,
                                                                      media_dir, file, facts)
                for scope in scopes:
                    g = groups[scope]
                    g['items'] += 1
                    g[{'direct': 'direct', 'kd': 'via_kd', 'uncovered': 'uncovered'}[status]] += 1
                    g['by_kd'].update(ids if status == 'kd' else ())
                    if status == 'kd':
                        # Read by scripts/compat_check.py (Decision 28 point 10).
                        g['covered_items'].append({'file': file, 'kd': sorted(ids), 'paths': covered_paths})
                    if status == 'uncovered':
                        g['uncovered_items'].append({'file': file, 'fields': missing})
            else:
                breakdown = {}
                per_sample = None if detail['agrees'] else packet_units(
                    detail, reference, candidate, defects,
                    functools.partial(load, reference_dir, file, 'streams'),
                    functools.partial(protected_formats, media_dir, file) if media_dir else None, breakdown,
                    functools.cache(functools.partial(cover_art, media_dir, file)) if media_dir else None,
                    functools.cache(functools.partial(chapter_tracks, media_dir, file)) if media_dir else None,
                    functools.partial(edit_track, media_dir, file) if media_dir else None, facts)
                items, direct, covered, uncovered, notes = reference_packet_units(
                    detail, reference, candidate, per_sample, breakdown, facts)
                sample_covered, sample_uncovered, _ = per_sample or ({}, 0, [])
                for scope in scopes:
                    g = groups[scope]
                    g['items'] += items
                    g['direct'] += direct
                    g['via_kd'] += sum(covered.values())
                    g['uncovered'] += uncovered
                    g['by_kd'].update(covered)
                    if uncovered:
                        g['uncovered_items'].append({'file': file, 'units': uncovered, 'examples': notes})
                    # The comparison's own unit, one candidate packet per
                    # PCM sample: reported, not gated.
                    t = g['per_sample']
                    t['items'] += detail['packets_agree'] + detail['packets_disagree']
                    t['direct'] += detail['packets_agree']
                    t['via_kd'] += sum(sample_covered.values())
                    t['uncovered'] += sample_uncovered
                    t['by_kd'].update(sample_covered)
        for g in groups.values():
            unknown = set(g['by_kd']) - entries
            if unknown:
                raise SystemExit(f'undocumented divergence ids: {sorted(unknown)}')
            g['by_kd'] = dict(sorted(g['by_kd'].items()))
            g['agreement'] = (g['direct'] + g['via_kd']) / g['items'] if g['items'] else None
            if 'per_sample' in g:
                t = g['per_sample']
                t['by_kd'] = dict(sorted(t['by_kd'].items()))
                t['agreement'] = (t['direct'] + t['via_kd']) / t['items'] if t['items'] else None
            g['passed'] = g['items'] > 0 and g['uncovered'] == 0
        result['levels'][name] = groups
    result['passed'] = all(levels['in_scope']['passed'] for levels in result['levels'].values())
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report', required=True)
    parser.add_argument('--reference', required=True)
    parser.add_argument('--candidate', required=True)
    parser.add_argument('--media', required=True, help='the fixture directory both captures were made from')
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    try:
        report = json.loads(pathlib.Path(args.report).read_text())
        result = overlay(report, args.reference, args.candidate, args.media)
    except (OSError, ValueError, KeyError) as error:
        print(json.dumps({'error': str(error)}), file=sys.stderr)
        return 2
    pathlib.Path(args.out).write_text(json.dumps(result, indent=2) + '\n')
    for level, groups in result['levels'].items():
        for scope, g in groups.items():
            print(f"{level:8} {scope:9} items {g['items']:>7}  direct {g['direct']:>7}  via KD {g['via_kd']:>7}"
                  f"  uncovered {g['uncovered']:>5}  {g['by_kd']}")
            if 'per_sample' in g:
                t = g['per_sample']
                print(f"{'':8} {'(per PCM sample)':9} items {t['items']:>7}  direct {t['direct']:>7}"
                      f"  via KD {t['via_kd']:>7}  uncovered {t['uncovered']:>5}  {t['by_kd']}")
    print('passed' if result['passed'] else 'FAILED')
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
