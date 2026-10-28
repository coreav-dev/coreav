#!/usr/bin/env python3
"""Adversarial check of the known-divergence overlay (docs/measurements/kd-audit.md).

For every file the overlay covers through a known divergence, injects one
bug-shaped change at a time into CoreAV's captures (a shifted timestamp, a
wrong duration, a lost or doubled packet, a dropped stream, a value moved
together with CoreAV's own evidence for it, ...) and replays the comparison
and the overlay. A real CoreAV bug of that shape must stay visible: the
change must add an uncovered item, or change nothing the comparison gates
on. A change the overlay covers is a loophole, and the run fails.

    kd_audit.py --reference DIR --candidate DIR --media DIR --oracle BIN [--overlay isobmff|ts]

--overlay ts audits the transport stream overlay (scripts/oracle_kd_ts.py)
with the same mutations and those of its own evidence.
"""
import argparse
import copy
import importlib.util
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
from fractions import Fraction

ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('oracle_kd', ROOT / 'scripts' / 'oracle_kd.py')
kd = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(kd)
TS_SPEC = importlib.util.spec_from_file_location('oracle_kd_ts', ROOT / 'scripts' / 'oracle_kd_ts.py')
kd_ts = importlib.util.module_from_spec(TS_SPEC)
TS_SPEC.loader.exec_module(kd_ts)
LEVELS = ('streams', 'packets')
# The overlay under audit: `kd.overlay` (one report) or the transport stream
# overlay (a list of reports); main() sets it.
OVERLAY = {'apply': lambda report, ref, cand, media: kd.overlay(report, ref, cand, media)}


def replay(oracle, profile, reference_dir, media_dir, name, documents):
    """The overlay's counts per level for one file with these candidate
    documents, {level: (items, direct, uncovered, by_kd)}, and the set of
    the comparison's gating differences."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = pathlib.Path(tmp)
        ref, cand = tmp / 'reference', tmp / 'candidate'
        ref.mkdir()
        cand.mkdir()
        for level in LEVELS:
            source = pathlib.Path(reference_dir) / f'{name}.{level}.json'
            if source.exists():
                # A copy: the comparison lists regular files only.
                shutil.copyfile(source, ref / source.name)
            if documents.get(level) is not None:
                (cand / f'{name}.{level}.json').write_text(json.dumps(documents[level]))
        done = subprocess.run([oracle, 'compare', '--reference', ref, '--candidate', cand, '--levels',
                               ','.join(LEVELS), '--profile', profile, '--max-diffs', '1000000',
                               '--report', tmp / 'report.json'], capture_output=True)
        if done.returncode not in (0, 1):
            raise SystemExit(f'kd_audit: compare failed on {name}: {done.stderr.decode(errors="replace")[-400:]}')
        report = json.loads((tmp / 'report.json').read_text())
        diffs = frozenset(json.dumps([level['level'], d], sort_keys=True) for level in report['levels']
                          for detail in level['details'] for d in detail.get('gating_differences', []))
        levels = OVERLAY['apply'](report, ref, cand, media_dir)['levels']
        return ({level: (g['all']['items'], g['all']['direct'], g['all']['uncovered'], dict(g['all']['by_kd']))
                 for level, g in levels.items()}, diffs)


def streams_of(document):
    return document.get('streams', []) if document else []


def same_stream(document, index):
    return [s for s in streams_of(document) if s.get('index') == index]


def mutations(documents):
    """(label, function(documents)) pairs; each function changes the
    candidate documents in place."""
    out = []
    sdoc, pdoc = documents.get('streams'), documents.get('packets')
    for s in streams_of(sdoc)[:4]:
        i = s.get('index')

        def field(name, change, i=i):
            def m(docs):
                for st in same_stream(docs.get('streams'), i):
                    st[name] = change(st.get(name))
            return m
        if isinstance(s.get('duration_ts'), int):
            for d in (1, -1, 1000):
                out.append((f'stream {i}: duration_ts {d:+}', field('duration_ts', lambda v, d=d: v + d)))
        if s.get('kind') == 'audio':
            out.append((f'stream {i}: one more channel', field('channels', lambda v: (v or 0) + 1)))
            out.append((f'stream {i}: sample rate doubled', field('sample_rate', lambda v: (v or 1) * 2)))
        if s.get('kind') == 'video':
            out.append((f'stream {i}: width + 2', field('width', lambda v: (v or 0) + 2)))
        out.append((f'stream {i}: other codec', field('codec_name', lambda v: 'h263' if v != 'h263' else 'mpeg4')))
        if s.get('codec_name') != 'mov_text':
            # A stream of another codec (a non-text one, in the KD-TEXT-END
            # pattern) reported as timed text.
            out.append((f'stream {i}: named mov_text', field('codec_name', lambda v: 'mov_text')))
        if isinstance(s.get('properties', {}).get('mdhd_duration_ticks'), int):
            def raw(docs, i=i):
                # The media header's duration reported as the stream's.
                for st in same_stream(docs.get('streams'), i):
                    st['duration_ts'] = st['properties']['mdhd_duration_ticks']
            out.append((f"stream {i}: duration_ts the media header's", raw))
        if s.get('profile') is not None:
            out.append((f'stream {i}: other profile', field('profile', lambda v: 'Main' if v != 'Main' else 'High')))
        unit = s.get('properties', {}).get('end_unit_ticks')
        if isinstance(unit, int) and isinstance(s.get('duration_ts'), int):
            # A transport stream end misread with CoreAV's own evidence for
            # it: one more unit, or a unit misread and the end with it.
            def end_moved(docs, i=i, unit=unit):
                for level in LEVELS:
                    for st in same_stream(docs.get(level), i):
                        st['duration_ts'] += unit
            out.append((f'stream {i}: duration_ts + one end unit', end_moved))

            def unit_misread(docs, i=i):
                for level in LEVELS:
                    for st in same_stream(docs.get(level), i):
                        st['properties']['end_unit_ticks'] += 900
                        st['duration_ts'] += 900
            out.append((f'stream {i}: end_unit_ticks and duration_ts + 900', unit_misread))

        def drop(docs, i=i, listed=False, fourcc=None):
            for level in LEVELS:
                doc = docs.get(level)
                if doc is None:
                    continue
                gone = same_stream(doc, i)
                doc['streams'] = [x for x in streams_of(doc) if x.get('index') != i]
                if listed and gone:
                    doc.setdefault('properties', {}).setdefault('dropped', []).append(
                        {'stream_index': i, 'track_id': gone[0].get('properties', {}).get('track_id'),
                         'codec_fourcc': fourcc or gone[0].get('codec_fourcc'), 'reason': 'CodecNotAllowed'})
        out.append((f'stream {i}: left out', drop))
        out.append((f'stream {i}: listed as not allowed', lambda docs, i=i: drop(docs, i, True)))
        out.append((f'stream {i}: listed as not allowed, as a disallowed code',
                    lambda docs, i=i: drop(docs, i, True, 'xxxx')))
        # A parsing bug moves a value and CoreAV's evidence for it together.
        props = s.get('properties', {})
        for prop in ('mdhd_duration_ticks', 'stts_duration_ticks', 'fragment_start_ticks', 'sidx_end_ticks'):
            if isinstance(props.get(prop), int):
                def with_evidence(docs, i=i, prop=prop):
                    for level in LEVELS:
                        for st in same_stream(docs.get(level), i):
                            st['properties'][prop] += 500
                            if isinstance(st.get('duration_ts'), int):
                                st['duration_ts'] += 500
                out.append((f'stream {i}: {prop} and duration_ts + 500', with_evidence))
        pair = props.get('media_edit_duration_ticks')
        if isinstance(pair, list) and len(pair) == 2 and all(isinstance(v, int) for v in pair):
            def edit_duration(docs, i=i):
                # A misread media edit, moved with the duration it caps.
                for level in LEVELS:
                    for st in same_stream(docs.get(level), i):
                        n, d = st['properties']['media_edit_duration_ticks']
                        st['properties']['media_edit_duration_ticks'] = [n + 500 * d, d]
                        if isinstance(st.get('duration_ts'), int):
                            st['duration_ts'] += 500
            out.append((f'stream {i}: media_edit_duration_ticks and duration_ts + 500', edit_duration))

            def no_cap(docs, i=i):
                # The media edit left unreported: no duration cap, so the duration is the media header's.
                for level in LEVELS:
                    for st in same_stream(docs.get(level), i):
                        st['properties']['media_edit_duration_ticks'] = None
                        if isinstance(st['properties'].get('mdhd_duration_ticks'), int):
                            st['duration_ts'] = st['properties']['mdhd_duration_ticks']
            out.append((f'stream {i}: media edit unreported, duration_ts the media header\'s', no_cap))
        offset = props.get('presentation_offset_ticks')
        if isinstance(offset, list) and len(offset) == 2 and all(isinstance(v, int) for v in offset):
            def shifted_offset(docs, i=i):
                # A misread empty edit: the offset and every timestamp move.
                for level in LEVELS:
                    for st in same_stream(docs.get(level), i):
                        n, d = st['properties']['presentation_offset_ticks']
                        st['properties']['presentation_offset_ticks'] = [n + d, d]
                        for pk in st.get('packets') or []:
                            for f in ('pts', 'dts'):
                                if isinstance(pk.get(f), int):
                                    pk[f] += 1
            out.append((f'stream {i}: presentation offset and every timestamp + 1', shifted_offset))
        if isinstance(props.get('edits'), dict):
            def unplaced(docs, i=i):
                # CoreAV wrongly calls a simple list unsupported and leaves
                # the stream out of its span.
                for level in LEVELS:
                    for st in same_stream(docs.get(level), i):
                        st['properties']['edits'] = 'unsupported'
                rest = [x for x in streams_of(docs['streams']) if x.get('index') != i
                        and isinstance(x.get('duration_ts'), int) and x.get('timescale')]
                if rest:
                    docs['streams']['format_duration_us'] = max(
                        round(Fraction(x['duration_ts'], x['timescale']) * 1_000_000) for x in rest)
            out.append((f'stream {i}: simple edit list called unsupported', unplaced))
        if isinstance(props.get('edits'), dict):
            def edit(docs, i=i):
                for level in LEVELS:
                    for st in same_stream(docs.get(level), i):
                        p = st['properties']
                        p['edits'] = dict(p['edits'], segment=p['edits']['segment'] + 100)
                        if p.get('edit_entries'):
                            p['edit_entries'] = [list(e) for e in p['edit_entries']]
                            p['edit_entries'][-1][0] += 100
            out.append((f'stream {i}: media edit + 100 in its evidence', edit))
        if s.get('codec_name') == 'mov_text' and isinstance(props.get('mdhd_duration_ticks'), int):
            # KD-TEXT-END: a misread media header, CoreAV's evidence and
            # duration moving with it; and the same stream as a non-text one.
            def text_header(docs, i=i):
                for level in LEVELS:
                    for st in same_stream(docs.get(level), i):
                        st['properties']['mdhd_duration_ticks'] += 500
                        if isinstance(st.get('duration_ts'), int):
                            st['duration_ts'] += 500
            out.append((f'stream {i}: text media header misread + 500, with its evidence', text_header))

            def not_text(docs, i=i):
                for level in LEVELS:
                    for st in same_stream(docs.get(level), i):
                        st.update(codec_name='aac', kind='audio', codec_fourcc='mp4a')
            out.append((f'stream {i}: text stream reported as audio, same end pattern', not_text))
    if isinstance((sdoc or {}).get('format_duration_us'), int):
        for d in (1, 1000):
            def fmt(docs, d=d):
                docs['streams']['format_duration_us'] += d
            out.append((f'format_duration_us {d:+}', fmt))

    def movie(docs):
        for level in LEVELS:
            p = (docs.get(level) or {}).get('properties', {})
            if isinstance(p.get('raw_movie_duration_ticks'), int):
                p['raw_movie_duration_ticks'] += 7
    out.append(('movie header duration + 7', movie))

    def reject(docs):
        for level in LEVELS:
            if docs.get(level) is not None:
                docs[level].update(probe_ok=False, streams=[])
                docs[level].setdefault('properties', {})['rejection'] = 'invalid_structure'
    out.append(('valid file rejected', reject))
    for s in streams_of(pdoc)[:4]:
        i, packets = s.get('index'), s.get('packets') or []
        if not packets:
            continue

        def at(j, change, i=i):
            def m(docs):
                for st in same_stream(docs['packets'], i):
                    change(st['packets'][j])
            return m

        def add(fields, d):
            def change(p):
                for f in fields:
                    if isinstance(p.get(f), int):
                        p[f] += d
            return change
        for j, where in ((0, 'first'), (len(packets) // 2, 'middle'), (len(packets) - 1, 'last')):
            out.append((f'stream {i}: {where} packet pts + 1', at(j, add(('pts',), 1))))
            out.append((f'stream {i}: {where} packet dts + 1', at(j, add(('dts',), 1))))
            out.append((f'stream {i}: {where} packet pts + 1 with media_cts', at(j, add(('pts', 'media_cts'), 1))))
            out.append((f'stream {i}: {where} packet dts + 1 with media_dts', at(j, add(('dts', 'media_dts'), 1))))
            out.append((f'stream {i}: {where} packet size + 1', at(j, add(('size',), 1))))
        middle = len(packets) // 2
        out.append((f'stream {i}: middle packet pts - 512 with media_cts', at(middle, add(('pts', 'media_cts'), -512))))
        out.append((f'stream {i}: middle keyframe flag flipped',
                    at(middle, lambda p: p.update(keyframe=not p.get('keyframe')))))

        def every(fields, i=i):
            def m(docs):
                for st in same_stream(docs['packets'], i):
                    for p in st['packets']:
                        add(fields, 1)(p)
            return m
        out.append((f'stream {i}: every pts + 1', every(('pts',))))
        out.append((f'stream {i}: every pts and dts + 1', every(('pts', 'dts'))))
        out.append((f'stream {i}: every timestamp and media time + 1', every(('pts', 'dts', 'media_cts', 'media_dts'))))

        def remove(j, i=i):
            def m(docs):
                for st in same_stream(docs['packets'], i):
                    del st['packets'][j]
            return m
        out.append((f'stream {i}: middle packet lost', remove(middle)))
        out.append((f'stream {i}: last packet lost', remove(len(packets) - 1)))

        def invented(docs, i=i):
            # Samples past the track's data that CoreAV made up, listed as
            # discarded after the edit.
            for st in same_stream(docs['packets'], i):
                last = st['packets'][-1]
                count, step = last.get('run', 1), last.get('duration') or 1
                extra = dict(last, discard=True, run=100) if 'run' in last else dict(last, discard=True)
                for f in ('pos',):
                    if isinstance(last.get(f), int):
                        extra[f] = last[f] + count * (last.get('size') or 0)
                for f in ('pts', 'dts', 'media_cts', 'media_dts'):
                    if isinstance(last.get(f), int):
                        extra[f] = last[f] + count * step
                st['packets'].append(extra)
        out.append((f'stream {i}: invented discarded samples after the last', invented))

        pid = s.get('properties', {}).get('pid')
        if isinstance(pid, int):
            def claimed(docs, i=i, pid=pid, j=middle):
                # A duplicate CoreAV claims but the file does not hold, and a
                # packet that then differs.
                p = docs['packets'].setdefault('properties', {})
                p['packet_warnings'] = list(p.get('packet_warnings') or []) + [f'Duplicate {{ pid: {pid}, offset: 0 }}']
                for st in same_stream(docs['packets'], i):
                    st['packets'][j]['size'] += 1
            out.append((f'stream {i}: a duplicate claimed, middle packet size + 1', claimed))

        def double(docs, i=i, j=middle):
            for st in same_stream(docs['packets'], i):
                st['packets'].insert(j, copy.deepcopy(st['packets'][j]))
        out.append((f'stream {i}: middle packet listed twice', double))
    # Two streams whose packets are each other's, and two whose records
    # are: on a file of several programs, where an entry excuses an order
    # of its own (KD-TS-STREAM-ORDER), both look like a reordering.
    listed = streams_of(pdoc)
    if len(listed) >= 2:
        for a, b in ((0, 1), (0, len(listed) - 1)):
            if a == b:
                continue

            def swap_packets(docs, a=a, b=b):
                doc = docs.get('packets')
                at = streams_of(doc)
                if len(at) > max(a, b):
                    at[a]['packets'], at[b]['packets'] = at[b]['packets'], at[a]['packets']

            def swap_records(docs, a=a, b=b):
                for level in LEVELS:
                    at = streams_of(docs.get(level))
                    if len(at) > max(a, b):
                        keep = {'index', 'packets'}
                        first = {k: v for k, v in at[a].items() if k not in keep}
                        second = {k: v for k, v in at[b].items() if k not in keep}
                        for key in set(first) | set(second):
                            at[a].pop(key, None)
                            at[b].pop(key, None)
                        at[a].update(second)
                        at[b].update(first)
            out.append((f'streams {a} and {b}: packets of each other', swap_packets))
            out.append((f'streams {a} and {b}: records of each other', swap_records))
    return out


def audit(reference_dir, candidate_dir, media_dir, oracle, profile, only=()):
    """Yields (file, label, verdict) for each mutation of each file: those a
    known divergence covers, and those that agree directly, which a change
    could pull into one. The verdicts are in `verdict_of`."""
    for path in sorted(pathlib.Path(candidate_dir).glob('*.streams.json')):
        name = path.name[:-len('.streams.json')]
        if only and not any(name.startswith(o) for o in only):
            continue
        documents = {level: json.loads((pathlib.Path(candidate_dir) / f'{name}.{level}.json').read_text())
                     for level in LEVELS if (pathlib.Path(candidate_dir) / f'{name}.{level}.json').exists()}
        base = replay(oracle, profile, reference_dir, media_dir, name, documents)
        for label, mutate in mutations(documents):
            changed = copy.deepcopy(documents)
            try:
                mutate(changed)
            except (IndexError, KeyError, TypeError):
                continue
            if changed == documents:
                continue
            result = replay(oracle, profile, reference_dir, media_dir, name, changed)
            yield name, label, verdict_of(base, result)


def verdict_of(base, result):
    """The verdict of one change, from the overlay's counts and the
    comparison's gating differences before (`base`) and after (`result`):
    - 'caught': the change adds an uncovered item;
    - 'COVERED': nothing new is uncovered, yet a known divergence covers
      more than before, or the comparison shows a difference it did not
      show before (a loophole: an entry absorbed it);
    - 'unlisted': the change only removed differences, and items the
      reference does not list (a packet `ffprobe` leaves out, such as one
      outside the edit, was lost): a limit of the comparison, not of the
      overlay;
    - 'agrees': the change only removed differences;
    - 'no effect': the comparison and the overlay are unchanged."""
    (counts, diffs), (after, after_diffs) = base, result
    if any(after[level][2] > counts[level][2] for level in counts if level in after):
        return 'caught'
    grown = {level: {k: n - counts[level][3].get(k, 0) for k, n in after[level][3].items()
                     if n > counts[level][3].get(k, 0)} for level in counts if level in after}
    grown = {level: g for level, g in grown.items() if g}
    if grown:
        return 'COVERED ' + json.dumps(grown)
    new = after_diffs - diffs
    if new:
        return 'COVERED absorbed ' + sorted(new)[0][:200]
    if after == counts and after_diffs == diffs:
        return 'no effect'
    if any(after[level][0] < counts[level][0] for level in counts if level in after):
        return 'unlisted'
    return 'agrees'



def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--reference', required=True)
    parser.add_argument('--candidate', required=True)
    parser.add_argument('--media', required=True)
    parser.add_argument('--oracle', required=True, help='the coreav-oracle binary')
    parser.add_argument('--profile', default='fixtures')
    parser.add_argument('--only', default='', help='comma-separated file name prefixes')
    parser.add_argument('--out', help='write every verdict as JSON lines')
    parser.add_argument('--overlay', choices=('isobmff', 'ts'), default='isobmff')
    args = parser.parse_args()
    if args.overlay == 'ts':
        OVERLAY['apply'] = lambda report, ref, cand, media: kd_ts.overlay([report], ref, cand, media)
    counts, covered, lines = {}, [], []
    for name, label, verdict in audit(args.reference, args.candidate, args.media, args.oracle, args.profile,
                                      tuple(o for o in args.only.split(',') if o)):
        kind = 'COVERED' if verdict.startswith('COVERED') else verdict
        counts[kind] = counts.get(kind, 0) + 1
        lines.append(json.dumps({'file': name, 'mutation': label, 'verdict': verdict}))
        if kind == 'COVERED':
            covered.append(f'{name}: {label}: {verdict}')
    if args.out:
        pathlib.Path(args.out).write_text('\n'.join(lines) + ('\n' if lines else ''))
    print('kd_audit:', ', '.join(f'{k} {v}' for k, v in sorted(counts.items())) or 'no file with a known divergence')
    for line in covered:
        print('  ' + line)
    return 1 if covered else 0


if __name__ == '__main__':
    sys.exit(main())
