#!/usr/bin/env python3
"""The edit-list oracle of `coreav decode-audio` (docs/design/audio-decode.md,
"Edit list bars", bars 1 and 2). Per MP4 input:

1. The presentation model: `ffmpeg -ignore_editlist 1` (every sample of the
   track, decoded) cut by the edit list as the oracle's independent reader
   reads it from the file (`Facts`, scripts/oracle_kd.py): empty edits are
   silence, media edits sample-exact windows, each boundary the exact sum of
   the durations before it rounded once (halves up). CoreAV's output must
   equal it (FLAC bit-exact, MP3 within the MP3 agreement tolerance, the
   length exact), and its summary's `timeline` must report it.
2. `ffmpeg`'s default output (edits applied its own way): every difference
   from CoreAV's is one of the known divergences of
   docs/known-divergences-decode-audio.md. An entry applies to an edit when
   the raw edit list and the track's packets meet its condition; it then
   says what `ffmpeg` writes for that edit, in terms of CoreAV's samples and
   `ffmpeg`'s full decode, and which of CoreAV's samples it drops or
   replaces, which are checked (silence must be zeros, dropped media must
   be the media's samples). The input is covered only when `ffmpeg`'s output
   is exactly that, so no entry is a tolerance.

The three decodes are compared in one streaming pass, so no output is held
in memory whole. `--audit` mutates CoreAV's output (a sample changed, added
or dropped, a silence span moved, the leading silence dropped as `ffmpeg`
drops it, the output of a misread edit list) and its summary, and requires
each mutation to fail the check and no entry to cover it (0 covered), as
tools/oracle/kd_audit.py requires of the probe entries. The audit holds the
decodes in memory: it runs on inputs whose full decode is at most
AUDIT_LIMIT bytes.

usage: decode_audio_check.py --coreav PATH FILE... [--ffmpeg PATH]
                             [--ffprobe PATH] [--audit] [--out RESULTS.json]
Results carry file hashes, never names.
"""
import argparse
import array
import hashlib
import importlib.util
import io
import json
import math
import pathlib
import struct
import subprocess
import sys
import tempfile
from fractions import Fraction

HERE = pathlib.Path(__file__).resolve().parent
TIMEOUT = 3600
CHUNK = 1 << 22
AUDIT_LIMIT = 256 << 20
# The MP3 agreement tolerance of docs/design/audio-decode.md ("MP3 bars"):
# twice ISO/IEC 11172-4's full accuracy limits.
MP3_RMS = 2 * 2.0 ** -15 / math.sqrt(12)
MP3_MAX = 2 * 2.0 ** -14
UNIT_RATE = 0x0001_0000

# Every entry of docs/known-divergences-decode-audio.json was written after
# the bars ("Edit list bars"), in the commit named.
ADDED_AFTER_THE_BARS = {
    'KD-A-LEADING-EMPTY': '9cac71f',
    'KD-A-EDIT-END': '9cac71f',
    'KD-A-LATER-EDIT-START': '9cac71f',
    'KD-A-INNER-EMPTY': '9cac71f',
}


def oracle_kd():
    """scripts/oracle_kd.py, for its independent reader."""
    spec = importlib.util.spec_from_file_location('oracle_kd', HERE / 'oracle_kd.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def known_divergences():
    path = HERE.parent / 'docs/known-divergences-decode-audio.json'
    return {e['id']: e for e in json.loads(path.read_text())['entries']}


# ---- the presentation model


def to_samples(ticks, rate, timescale):
    """`ticks` at `timescale` in samples at `rate`, to the nearest, halves up."""
    return math.floor(Fraction(ticks * rate, timescale) + Fraction(1, 2))


def applies(edits, timescale, movie_timescale, fragmented):
    """Whether the list has a shape decode-audio applies (docs/design/
    audio-decode.md, "Edit lists"; docs/design/edit-lists.md): one media
    edit at rate 1, after at most one empty edit, lasting to the end of the
    media (duration 0) only in a movie with fragments; or, without
    fragments, empty edits and media edits at rate 1 of nonzero duration
    whose windows increase without overlap."""
    if not edits or not timescale or not movie_timescale:
        return False
    media = [e for e in edits if e[1] != -1]
    simple = len(media) == 1 and edits[-1] is media[0] and len(edits) <= 2
    if simple and media[0][1] >= 0 and media[0][2] == UNIT_RATE and (media[0][0] or fragmented):
        return True
    if fragmented or not media or sum(e[0] for e in edits) >= 1 << 64:
        return False
    if any(e[1] < 0 or e[2] != UNIT_RATE or e[0] == 0 for e in media):
        return False
    windows = [(e[1] * movie_timescale, e[1] * movie_timescale + e[0] * timescale) for e in media]
    return all(b[0] >= a[1] for a, b in zip(windows, windows[1:]))


def pieces(edits, first_dts, rate, timescale, movie_timescale, fragmented=False):
    """The timeline of the raw entries `(duration, media_time, rate)`:
    ('silence', samples) and ('media', from, to) in samples decoded from the
    first packet, or None for no list or one outside the applied shapes
    (all the samples are written then, with a warning)."""
    if not applies(edits, timescale, movie_timescale, fragmented):
        return None
    out, elapsed = [], 0
    for duration, media_time, _ in edits:
        begin = to_samples(elapsed, rate, movie_timescale)
        elapsed += duration
        length = to_samples(elapsed, rate, movie_timescale) - begin
        if media_time == -1:
            out.append(('silence', length))
        else:
            start = to_samples(media_time - first_dts, rate, timescale)
            # A media edit of duration 0 runs to the end of the media.
            out.append(('media', start, start + length if duration else 1 << 62))
    return out


def effective(plan, total):
    """The plan with every window inside the media's `total` samples (the
    whole media for no plan): ('silence', n) and ('media', a, b)."""
    if plan is None:
        return [('media', 0, total)]
    out = []
    for piece in plan:
        if piece[0] == 'silence':
            out.append(piece)
        else:
            a, b = min(max(piece[1], 0), total), min(piece[2], total)
            out.append(('media', a, max(a, b)))
    return out


def length(piece):
    return piece[1] if piece[0] == 'silence' else piece[2] - piece[1]


def timeline(plan, total):
    """The model's summary timeline: the first media sample's output sample
    and the spans of silence."""
    written, start, spans = 0, None, []
    for piece in effective(plan, total):
        n = length(piece)
        if piece[0] == 'silence' and n:
            spans.append({'at': written, 'samples': n})
        elif piece[0] == 'media' and n and start is None:
            start = written
        written += n
    return {'start': written if start is None else start, 'silence': spans}


def model(full, frame, plan):
    """The model's output bytes, for a decode held in memory."""
    out = bytearray()
    for piece in effective(plan, len(full) // frame):
        out += bytes(piece[1] * frame) if piece[0] == 'silence' else full[piece[1] * frame:piece[2] * frame]
    return bytes(out)


# ---- the known divergences: what ffmpeg writes for each edit
#
# For each piece of the plan an entry may rewrite `ops`, what ffmpeg writes
# for it in order: ('cv', offset, n) CoreAV's samples of the piece from
# `offset`, ('full', at, n) the full decode's from sample `at`, ('loose',
# at, n) as many samples compared in length only; and add `checks` on
# CoreAV's samples of the piece: ('zero', offset, n) silence, ('full',
# offset, n, at) the full decode's from `at`. Entries see only the raw edit
# list (as the plan) and the packets, never the samples.


def packet_edges(facts, track_id, rate, timescale):
    """The track's packet starts, and the end of the last, in samples
    decoded from the first packet."""
    runs = facts.runs(track_id)
    first = runs[0].dts if runs else 0
    edges = []
    for r in runs:
        edges.extend(to_samples(r.dts + k * r.duration - first, rate, timescale) for k in range(r.count))
    if runs:
        last = runs[-1]
        edges.append(to_samples(last.dts + last.count * last.duration - first, rate, timescale))
    return edges


def leading_empty(piece, ops, ctx):
    """KD-A-LEADING-EMPTY: an empty edit before the first media edit that
    presents a sample is not written."""
    if ctx['seen_media'] or not piece[1]:
        return None
    return [], [('zero', 0, piece[1])]


def inner_empty(piece, ops, ctx):
    """KD-A-INNER-EMPTY: an empty edit after a media edit is the media's first
    whole packets, those that start before its length. For a lossy codec
    the first packet's samples are compared in length only: its decoder
    state carries over from the packet before, so they are not the fresh
    decode's."""
    if not ctx['seen_media'] or not piece[1]:
        return None
    edges, total = ctx['edges'], ctx['total']
    end = min(next((e for e in edges[1:] if e >= piece[1]), edges[-1] if edges else 0), total)
    first = min(edges[1], end) if ctx['lossy'] and len(edges) > 1 else 0
    fill = ([('loose', 0, first)] if first else []) + ([('full', first, end - first)] if end > first else [])
    return fill, [('zero', 0, piece[1])]


def later_edit_start(piece, ops, ctx):
    """KD-A-LATER-EDIT-START: a media edit after the first, whose start falls
    inside a packet, starts at the next packet."""
    a, b = piece[1], piece[2]
    if not ctx['seen_media'] or a == b:
        return None
    start = min(next((e for e in ctx['edges'] if e >= a), a), b)
    if start == a:
        return None
    cut = start - a
    rest = [('cv', o + cut, n - cut) if kind == 'cv' and o == 0 else (kind, o, n)
            for kind, o, n in ops if not (kind == 'cv' and o == 0 and n <= cut)]
    return rest, [('full', 0, cut, a)]


def edit_end(piece, ops, ctx):
    """KD-A-EDIT-END: a media edit whose end falls inside a packet, before the
    end of the media, runs to the end of that packet."""
    a, b = piece[1], piece[2]
    if a == b or ctx['raw_end'] >= ctx['total']:
        return None
    end = min(next((e for e in ctx['edges'] if e >= b), b), ctx['total'])
    if end == b:
        return None
    return ops + [('full', b, end - b)], []


PREDICATES = {
    'leading_empty': leading_empty,
    'inner_empty': inner_empty,
    'edit_end': edit_end,
    'later_edit_start': later_edit_start,
}
# The entries that may apply to each kind of piece, in the order they apply.
APPLY = {
    'silence': ('KD-A-LEADING-EMPTY', 'KD-A-INNER-EMPTY'),
    'media': ('KD-A-LATER-EDIT-START', 'KD-A-EDIT-END'),
}


def ffmpeg_ops(ref, entries):
    """Per piece of the effective plan, (ops, checks), and the entries that
    apply; None for a list that is not applied (no entry applies there)."""
    if ref['plan'] is None:
        return None
    total = ref['total']
    ctx = {'seen_media': False, 'edges': ref['edges'], 'total': total, 'lossy': ref['lossy']}
    out, applied = [], set()
    for raw, piece in zip(ref['plan'], effective(ref['plan'], total)):
        ops, checks = [('cv', 0, length(piece))] if length(piece) else [], []
        ctx['raw_end'] = raw[2] if raw[0] == 'media' else 0
        for kd in APPLY[piece[0]]:
            moved = PREDICATES[entries[kd]['predicate']](piece, ops, ctx)
            if moved is not None:
                ops, more = moved
                checks += more
                applied.add(kd)
        if piece[0] == 'media' and length(piece):
            ctx['seen_media'] = True
        out.append((ops, checks))
    return out, sorted(applied)


# ---- streaming comparison


class Source:
    """A byte stream read at positions that only move forward, but for
    reads at most `keep` bytes behind the furthest one; its first `prefix`
    bytes stay readable."""

    def __init__(self, stream, keep=0, prefix=0):
        self.stream, self.keep, self.prefix = stream, keep, prefix
        self.buf, self.start, self.head, self.eof = bytearray(), 0, bytearray(), False

    def _fill(self, end, floor):
        while self.start + len(self.buf) < end and not self.eof:
            data = self.stream.read(CHUNK)
            if not data:
                self.eof = True
                break
            at = self.start + len(self.buf)
            if at < self.prefix:
                self.head += data[:self.prefix - at]
            self.buf += data
            if floor > self.start:
                drop = min(floor - self.start, len(self.buf))
                del self.buf[:drop]
                self.start += drop

    def take(self, at, n):
        """Bytes [at, at + n), fewer at the end of the stream."""
        if at < self.start:
            if at + n <= len(self.head):
                return bytes(self.head[at:at + n])
            raise ValueError('read behind the kept bytes')
        self._fill(at + n, at - self.keep)
        out = bytes(self.buf[at - self.start:at - self.start + n])
        drop = min(at + len(out) - self.keep - self.start, len(self.buf))
        if drop > 0:
            del self.buf[:drop]
            self.start += drop
        return out

    def ends_at(self, at):
        """Whether the stream ends exactly at `at`."""
        if at < self.start:
            return False
        self._fill(at + 1, at - self.keep)
        return self.eof and self.start + len(self.buf) == at


class Compare:
    """The comparison of two sample streams, chunk by chunk: equal bytes, or
    for MP3 (f32) equal lengths within the MP3 agreement tolerance over the
    whole stream."""

    def __init__(self, lossy):
        self.lossy, self.ok_so_far, self.square, self.worst, self.count = lossy, True, 0.0, 0.0, 0

    def add(self, a, b):
        if len(a) != len(b):
            self.ok_so_far = False
        elif not self.lossy:
            self.ok_so_far &= a == b
        elif a:
            square, worst = differences(a, b)
            if not math.isfinite(square):
                self.ok_so_far = False
            else:
                self.square, self.worst = self.square + square, max(self.worst, worst)
                self.count += len(a) // 4

    def ok(self):
        if not self.ok_so_far:
            return False
        if not self.lossy or not self.count:
            return True
        return math.sqrt(self.square / self.count) <= MP3_RMS and self.worst <= MP3_MAX


def differences(a, b):
    """The sum of squared differences of two f32 sample runs, and the
    largest difference."""
    try:
        import numpy as np
    except ImportError:
        x, y = array.array('f', a), array.array('f', b)
        if sys.byteorder != 'little':
            x.byteswap()
            y.byteswap()
        worst, square = 0.0, 0.0
        for p, q in zip(x, y):
            d = abs(p - q)
            worst, square = max(worst, d), square + d * d
        return square, worst
    d = np.frombuffer(a, '<f4').astype(np.float64) - np.frombuffer(b, '<f4').astype(np.float64)
    if not np.isfinite(d).all():
        return math.inf, math.inf
    return float(np.dot(d, d)), float(np.abs(d).max())


def stream_compare(cv, full, ffmpeg, ref, ops):
    """One pass over CoreAV's output (`cv`), the full decode and `ffmpeg`'s
    output (Sources): (model, ffmpeg) agreement, the second under `ops`
    (per piece, (ops, checks)); None for the comparison with CoreAV's output
    as it is."""
    frame, lossy, total = ref['frame'], ref['lossy'], ref['total']
    plan = effective(ref['plan'], total)
    if ops is None:
        ops = [([('cv', 0, length(p))], []) for p in plan]
    bar1, bar2, checks = Compare(lossy), Compare(lossy), Compare(lossy)
    # Whether CoreAV wrote the plan's samples, no fewer and no more: an entry
    # rewrites CoreAV's pieces, so it covers nothing else.
    zeros_ok = lengths_ok = True
    c_at = d_at = 0
    for piece, (piece_ops, piece_checks) in zip(plan, ops):
        n = length(piece)
        emits = [op for op in piece_ops if op[0] == 'cv']
        after = [op for op in piece_ops if op[0] != 'cv']
        if piece_ops[:len(emits)] != emits:
            raise ValueError('CoreAV samples after others in a piece')
        o = 0
        while o < n:
            m = min(CHUNK // frame, n - o)
            data = cv.take((c_at + o) * frame, m * frame)
            lengths_ok &= len(data) == m * frame
            if piece[0] == 'silence':
                expected = bytes(m * frame)
            else:
                expected = full.take((piece[1] + o) * frame, m * frame)
            bar1.add(data, expected)
            for kind, off, k, *at in piece_checks:
                lo, hi = max(off, o), min(off + k, o + m)
                if lo >= hi:
                    continue
                part = data[(lo - o) * frame:(hi - o) * frame]
                if kind == 'zero':
                    zeros_ok &= part.count(0) == len(part)
                elif piece[0] == 'media' and at[0] == piece[1] + off:
                    # Dropped media: the full decode's samples where the
                    # model has them.
                    checks.add(part, expected[(lo - o) * frame:(hi - o) * frame])
                else:
                    raise ValueError('a check of samples the model does not hold')
            for _, off, k in emits:
                lo, hi = max(off, o), min(off + k, o + m)
                if lo < hi:
                    part = data[(lo - o) * frame:(hi - o) * frame]
                    bar2.add(part, ffmpeg.take(d_at * frame, len(part)))
                    d_at += hi - lo
            o += m
        c_at += n
        for kind, at, k in after:
            done = 0
            while done < k:
                m = min(CHUNK // frame, k - done)
                theirs = ffmpeg.take(d_at * frame, m * frame)
                if kind == 'loose':
                    bar2.ok_so_far &= len(theirs) == m * frame
                else:
                    bar2.add(full.take((at + done) * frame, m * frame), theirs)
                d_at += m
                done += m
    lengths_ok &= cv.ends_at(c_at * frame)
    model_ok = bar1.ok() and lengths_ok
    ffmpeg_ok = bar2.ok() and checks.ok() and zeros_ok and lengths_ok and ffmpeg.ends_at(d_at * frame)
    return model_ok, ffmpeg_ok


def verdicts(open_streams, ref, entries, summary):
    """The verdicts on one input: `open_streams()` gives fresh Sources
    (CoreAV's output, the full decode, ffmpeg's output) for each pass."""
    planned = ffmpeg_ops(ref, entries)
    ops, applied = planned if planned else (None, [])
    if not applied:
        ops = None
    model_ok, ffmpeg_ok = stream_compare(*open_streams(), ref, ops)
    result = {'model': model_ok, 'timeline': summary == timeline(ref['plan'], ref['total'])}
    if ref['ffmpeg_failed']:
        result['ffmpeg'] = 'not decoded'
    elif ops is None:
        result['ffmpeg'] = 'equal' if ffmpeg_ok else 'uncovered'
    elif ffmpeg_ok:
        result['ffmpeg'] = 'covered'
        result['entries'] = applied
    else:
        # Not as the entries say: as it is, or uncovered.
        _, equal = stream_compare(*open_streams(), ref, None)
        result['ffmpeg'] = 'equal' if equal else 'uncovered'
    return result


def passed(result):
    return result['model'] and result['timeline'] and result['ffmpeg'] in ('equal', 'covered')


def in_memory(cv, full, ffmpeg, ref):
    """`open_streams` for decodes held in memory."""
    keep = keep_bytes(ref)
    return lambda: (Source(io.BytesIO(cv)), Source(io.BytesIO(full), keep, prefix_bytes(ref)),
                    Source(io.BytesIO(ffmpeg)))


def keep_bytes(ref):
    """How far behind its furthest read the full decode is read: a chunk and
    two packets."""
    edges = ref['edges']
    packet = max((b - a for a, b in zip(edges, edges[1:])), default=0)
    return CHUNK + 2 * packet * ref['frame']


def prefix_bytes(ref):
    """The full decode's first bytes that inner empty edits' fills read."""
    planned = ffmpeg_ops(ref, known_divergences())
    if not planned:
        return 0
    ends = [at + n for piece, (ops, _) in zip(effective(ref['plan'], ref['total']), planned[0])
            if piece[0] == 'silence' for kind, at, n in ops if kind == 'full']
    return max(ends, default=0) * ref['frame']


# ---- the mutation audit


def flipped(out, at, ref):
    """`out` with sample `at` changed: a low bit of an integer sample; a float
    sample moved by 0.25, far outside the MP3 tolerance."""
    frame = ref['frame']
    sample = bytearray(out[at * frame:(at + 1) * frame])
    if ref['lossy']:
        value = struct.unpack_from('<f', sample)[0]
        struct.pack_into('<f', sample, 0, value - 0.25 if value > 0 else value + 0.25)
    else:
        sample[1] ^= 0x01
    return out[:at * frame] + bytes(sample) + out[(at + 1) * frame:]


def same(a, b, lossy):
    """Whether two outputs held in memory compare equal: bytes, or within the
    MP3 tolerance."""
    compare = Compare(lossy)
    for at in range(0, max(len(a), len(b)), CHUNK):
        compare.add(a[at:at + CHUNK], b[at:at + CHUNK])
    return compare.ok()


def mutations(out, summary, ref, full):
    """Bug-shaped changes of CoreAV's output and summary: (name, output,
    summary)."""
    frame = ref['frame']
    n = len(out) // frame
    yield 'one sample fewer at the start', out[frame:], summary
    yield 'one sample fewer at the end', out[:-frame], summary
    yield 'one sample more at the end', out + (out[-frame:] if out else bytes(frame)), summary
    # A sample changed in the middle, and at both ends of every media piece
    # (where the entries drop samples or add after them).
    at, marks = 0, {n // 2}
    for piece in effective(ref['plan'], ref['total']):
        if length(piece) and piece[0] == 'media':
            marks |= {at, at + length(piece) - 1}
        at += length(piece)
    for at in sorted(m for m in marks if 0 <= m < n):
        yield f'a sample changed ({at}/{n})', flipped(out, at, ref), summary
    for k, span in enumerate(summary['silence']):
        at, count = span['at'], span['samples']
        yield f'silence span {k} one sample longer', out[:at * frame] + bytes(frame) + out[at * frame:], summary
        yield f'silence span {k} one sample shorter', out[:at * frame] + out[(at + 1) * frame:], summary
        yield f'silence span {k} not silent', flipped(out, at + count // 2, ref), summary
    if summary['start']:
        lead = summary['start']
        yield 'the leading silence dropped, as ffmpeg drops it', out[lead * frame:], \
            {'start': 0, 'silence': [{'at': s['at'] - lead, 'samples': s['samples']}
                                     for s in summary['silence'] if s['at'] >= lead]}
    yield 'the summary start moved by one', out, {**summary, 'start': summary['start'] + 1}
    if summary['silence']:
        yield 'a silence span unreported', out, {**summary, 'silence': summary['silence'][1:]}
    # The output of a misread edit list, reported consistently with itself:
    # each raw value of each entry off by one tick either way, and each
    # entry dropped.
    edits = ref['edits']
    for k, (duration, media_time, rate) in enumerate(edits):
        wrong = [(duration + 1, media_time, rate), (max(duration - 1, 0), media_time, rate)]
        if media_time != -1:
            wrong += [(duration, media_time + 1, rate), (duration, max(media_time - 1, 0), rate)]
        variants = [edits[:k] + [w] + edits[k + 1:] for w in wrong] + [edits[:k] + edits[k + 1:]]
        for v, misread in enumerate(variants):
            plan = pieces(misread, ref['first_dts'], ref['rate'], ref['timescale'], ref['movie_timescale'],
                          ref['fragmented'])
            yield f'edit {k} misread ({v})', model(full, frame, plan), timeline(plan, ref['total'])


def audit(out, summary, full, ffmpeg, ref, entries):
    """Each mutation must fail the check, and no entry may cover it. A
    mutation whose output compares equal to CoreAV's (for MP3, within the
    tolerance: a shift of one sample in quiet audio) is outside what the
    comparison can see, with or without the entries: reported as within
    the tolerance, not judged."""
    found = {}
    for name, data, mutated in mutations(out, summary, ref, full):
        same_output = same(data, out, ref['lossy'])
        if same_output and mutated == summary:
            if data != out:
                found[name] = 'within the tolerance'
            continue
        r = verdicts(in_memory(data, full, ffmpeg, ref), ref, entries, mutated)
        # A change of the summary alone is outside the comparison with
        # ffmpeg, which gates on samples only: the timeline check must catch
        # it.
        if r['ffmpeg'] == 'covered' and not same_output:
            found[name] = 'COVERED'
        elif passed(r):
            found[name] = 'MISSED'
        else:
            found[name] = 'caught'
    return found


# ---- running


class Wav:
    """The data of a WAV file read from a stream (its size unknown when
    streamed): read() as of a file object."""

    def __init__(self, stream):
        self.stream, self.data, self.left = stream, None, None

    def _header(self):
        head = self.stream.read(12)
        if len(head) != 12 or head[:4] != b'RIFF' or head[8:12] != b'WAVE':
            raise ValueError('not a WAV file')
        while True:
            chunk = self.stream.read(8)
            if len(chunk) != 8:
                raise ValueError('no data chunk')
            kind, size = chunk[:4], int.from_bytes(chunk[4:], 'little')
            if kind == b'data':
                self.left = None if size == 0xFFFF_FFFF else size
                return
            self.stream.read(size + (size & 1))

    def read(self, n):
        if self.left is None and self.data is None:
            self._header()
            self.data = True
        if self.left is not None:
            n = min(n, self.left)
            self.left -= n
        return self.stream.read(n) if n else b''


def wav_data(blob):
    """The data chunk of a WAV file held in memory."""
    try:
        return Wav(io.BytesIO(blob)).read(len(blob))
    except ValueError:
        return None


class Process:
    """A command whose standard output is read as a stream, its standard
    error kept in a file."""

    def __init__(self, args):
        self.err = tempfile.TemporaryFile()
        self.proc = subprocess.Popen([str(a) for a in args], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                     stderr=self.err)

    def read(self, n):
        return self.proc.stdout.read(n)

    def close(self):
        """(exit status, standard error), the rest of the output discarded."""
        while self.proc.stdout.read(CHUNK):
            pass
        code = self.proc.wait(timeout=TIMEOUT)
        self.err.seek(0)
        err = self.err.read().decode(errors='replace')
        self.err.close()
        return code, err


def decode_args(tools, path, ref, ignore):
    return [tools.ffmpeg, '-v', 'error', '-nostdin', *(['-ignore_editlist', '1'] if ignore else []), '-i', path,
            '-map', f"0:{ref['index']}", '-f', 'f32le' if ref['lossy'] else 's32le', '-']


def coreav_args(tools, path, ref):
    return [tools.coreav, 'decode-audio', '--format', ref['format'], path, '-']


def reference(path, tools):
    """What the independent reader and `ffprobe` give of the first audio
    stream, or None when either cannot read it."""
    facts = oracle_kd().read_facts(path.parent, path.name)
    done = subprocess.run([str(a) for a in (tools.ffprobe, '-v', 'error', '-select_streams', 'a:0',
                                            '-show_entries', 'stream=index,id,codec_name,sample_rate,channels',
                                            '-of', 'json', path)], capture_output=True, timeout=TIMEOUT)
    streams = json.loads(done.stdout or b'{}').get('streams', []) if done.returncode == 0 else []
    if facts is None or not streams:
        return None
    info = streams[0]
    try:
        track = facts.tracks.get(int(info.get('id', ''), 16))
        runs = facts.runs(track['track_id'])
        edges = packet_edges(facts, track['track_id'], int(info['sample_rate']), track['timescale'])
    except (ValueError, IndexError, KeyError, TypeError):
        return None
    lossy = info.get('codec_name') != 'flac'
    rate, first, fragmented = int(info['sample_rate']), runs[0].dts if runs else 0, bool(facts.fragmented)
    return {'plan': pieces(track['edits'], first, rate, track['timescale'], facts.movie_timescale, fragmented),
            'frame': 4 * int(info['channels']), 'lossy': lossy, 'edges': edges, 'index': info['index'],
            'edits': [tuple(e) for e in track['edits']], 'first_dts': first, 'rate': rate,
            'timescale': track['timescale'], 'movie_timescale': facts.movie_timescale, 'fragmented': fragmented,
            'format': 'f32' if lossy else 's32', 'codec': info.get('codec_name')}


def check(path, tools, entries):
    digest = hashlib.sha256()
    with open(path, 'rb') as f:
        while block := f.read(CHUNK):
            digest.update(block)
    record = {'sha256': digest.hexdigest()}
    ref = reference(path, tools)
    if ref is None:
        return {**record, 'status': 'not read'}
    # The full decode's length; kept when small enough for the audit.
    full = Process(decode_args(tools, path, ref, True))
    size, kept = 0, bytearray()
    while block := full.read(CHUNK):
        size += len(block)
        if size <= AUDIT_LIMIT:
            kept += block
    code, err = full.close()
    if code != 0:
        return {**record, 'status': 'not read'}
    ref['total'] = size // ref['frame']
    # The last packet ends where its decode does: a writer may shorten its
    # stored duration to the stream's length (an encoder's padding).
    if ref['edges'] and ref['edges'][-1] < ref['total']:
        ref['edges'][-1] = ref['total']
    probe = Process(decode_args(tools, path, ref, False))
    code2, err2 = probe.close()
    ref['ffmpeg_failed'] = code2 != 0
    decode_errors = bool(err or err2)
    summary = Process(coreav_args(tools, path, ref))
    code3, err3 = summary.close()
    if code3 != 0:
        # Counted apart when ffmpeg cannot decode the input either.
        return {**record, 'status': 'coreav failed', 'error': err3[-300:],
                'decode_errors': decode_errors or ref['ffmpeg_failed']}
    summary = json.loads(err3)['timeline']
    processes = []

    def open_streams():
        streams = [Process(coreav_args(tools, path, ref)), Process(decode_args(tools, path, ref, True)),
                   Process(decode_args(tools, path, ref, False))]
        processes.extend(streams)
        return (Source(Wav(streams[0])), Source(streams[1], keep_bytes(ref), prefix_bytes(ref)),
                Source(streams[2]))
    try:
        result = verdicts(open_streams, ref, entries, summary)
    finally:
        for p in processes:
            p.close()
    record |= {'status': 'ok', 'codec': ref['codec'], 'edits': [list(e) for e in ref['edits']],
               'applied': ref['plan'] is not None, 'decode_errors': decode_errors, **result}
    if tools.audit and ref['edits'] and size <= AUDIT_LIMIT:
        cv = subprocess.run([str(a) for a in coreav_args(tools, path, ref)], capture_output=True, timeout=TIMEOUT)
        ff = subprocess.run([str(a) for a in decode_args(tools, path, ref, False)], capture_output=True,
                            timeout=TIMEOUT)
        record['audit'] = audit(wav_data(cv.stdout), summary, bytes(kept), ff.stdout, ref, entries)
    elif tools.audit and ref['edits']:
        record['audit_skipped'] = 'full decode above AUDIT_LIMIT'
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('files', nargs='+', type=pathlib.Path)
    parser.add_argument('--coreav', required=True)
    parser.add_argument('--ffmpeg', default='ffmpeg')
    parser.add_argument('--ffprobe', default='ffprobe')
    parser.add_argument('--audit', action='store_true')
    parser.add_argument('--out', type=pathlib.Path)
    tools = parser.parse_args()
    entries = known_divergences()
    results = [check(f, tools, entries) for f in tools.files]
    ok = [r for r in results if r['status'] == 'ok']
    # Bars 1 and 2 count the inputs ffmpeg decodes without an error.
    clean = [r for r in ok if not r['decode_errors'] and r['ffmpeg'] != 'not decoded']
    audits = [v for r in ok for v in r.get('audit', {}).values()]
    summary = {
        'inputs': len(results), 'ok': len(ok), 'not_read': sum(r['status'] == 'not read' for r in results),
        'coreav_failed': sum(r['status'] == 'coreav failed' and not r['decode_errors'] for r in results),
        'both_failed': sum(r['status'] == 'coreav failed' and r['decode_errors'] for r in results),
        'with_edit_lists': sum(bool(r['edits']) for r in ok),
        'decoded_without_errors': len(clean),
        'model': sum(r['model'] and r['timeline'] for r in clean),
        'ffmpeg_equal': sum(r['ffmpeg'] == 'equal' for r in clean),
        'ffmpeg_covered': sum(r['ffmpeg'] == 'covered' for r in clean),
        'ffmpeg_uncovered': sum(r['ffmpeg'] == 'uncovered' for r in clean),
        'entries': {k: sum(k in r.get('entries', []) for r in clean) for k in ADDED_AFTER_THE_BARS},
        'audited': sum('audit' in r for r in ok), 'audit_skipped': sum('audit_skipped' in r for r in ok),
        'audit': {v: audits.count(v) for v in ('caught', 'COVERED', 'MISSED', 'within the tolerance')},
    }
    if tools.out:
        tools.out.write_text(json.dumps({'summary': summary, 'results': results}, indent=1) + '\n')
    print(json.dumps(summary, indent=1))
    bad = len(clean) - summary['model'] or summary['ffmpeg_uncovered'] or summary['coreav_failed'] \
        or summary['audit']['COVERED'] or summary['audit']['MISSED']
    return 1 if bad else 0


if __name__ == '__main__':
    sys.exit(main())
