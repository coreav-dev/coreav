#!/usr/bin/env python3
"""Apply the documented Matroska known divergences to coreav-oracle reports.

Under the oracle and correctness policy, a mismatch counts as agreement
only when a documented entry's predicate covers it; covered items
stay in the denominator and are reported apart from direct agreement. The
entries live in docs/known-divergences-mkv.md with the twin
docs/known-divergences-mkv.json; the captures come from tools/mkv-adapter
and scripts/oracle-ffprobe.sh. Predicates read only the two captures, never
file names, except KD-MKV-DECLARED-SIZE, which also reads the file's
track header and its first decoded frame (with --media, the directory of
the captured files; without it the entry covers nothing), and
KD-MKV-TAG-LANGUAGE, which reads the track entries and `Tags` and which
scripts/compat_check.py applies to `--ffprobe-compat` output
(`COMPAT_ENTRIES`), not this overlay. Captures are read, never rewritten.

Usage: oracle_kd_mkv.py --report R [--report R2 ...] --reference DIR
                        --candidate DIR --out OUT [--media DIR]
Several reports (for example one per corpus file) are merged. Exit status: 0
when every Matroska item agrees directly or via a documented divergence, 1
otherwise, 2 for usage or input errors.
"""
import argparse
import collections
import json
import pathlib
import subprocess
import sys
from fractions import Fraction

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'scripts'))
import policy_ref  # noqa: E402
KD_FILE = ROOT / 'docs' / 'known-divergences-mkv.json'
IN_SCOPE = ('.mkv', '.webm', '.mka', '.mk3d')

# Video codecs whose decode order can differ from presentation order: a
# decode time cannot be read from Matroska for them (RFC 9559 11.2).
REORDERING = {'h264', 'hevc', 'mpeg1video', 'mpeg2video', 'mpeg4', 'theora'}
# Codecs whose profile, level and color CoreAV does not read from the
# bitstream (no stored configuration record to read them from).
UNREAD_BITSTREAMS = {'mpeg1video', 'mpeg2video', 'mpeg4', 'theora', 'mjpeg', 'ffv1', 'dts', 'truehd'}
IMAGE_CODECS = {'mjpeg', 'png', 'bmp', 'gif', 'webp', 'tiff'}


def load(directory, name, level):
    path = pathlib.Path(directory) / f'{name}.{level}.json'
    return json.loads(path.read_text()) if path.exists() else {}


def props(item):
    return (item or {}).get('properties') or {}


def round_half_away(value):
    value = Fraction(value)
    sign = -1 if value < 0 else 1
    return sign * int((abs(value) * 2 + 1) // 2)


def floor(value):
    return Fraction(value).numerator // Fraction(value).denominator


# ---------------------------------------------------------------- predicates

def dts_unknown(cstream, rpacket, cpacket):
    """Matroska stores no decode times; CoreAV leaves them unknown for codecs
    that can reorder frames, ffprobe derives one."""
    return (cstream.get('codec_name') in REORDERING and cpacket is not None
            and cpacket.get('dts') is None and rpacket.get('dts') is not None)


def lace_without_duration(cstream, rpacket, cpacket):
    """A later frame of a lace with no DefaultDuration has no timestamp in
    Matroska (RFC 9559 10.3.5); ffprobe derives one from the codec."""
    return (cpacket is not None and cpacket.get('lace', 0) > 0
            and props(cstream).get('default_duration_ns') is None
            and cpacket.get('pts') is None and cpacket.get('dts') is None
            and rpacket.get('pts') is not None)


def lace_split(cstream, rpacket, cpacket, timestamp_scale):
    """A later frame of a lace starts at the block time plus its position
    times DefaultDuration; CoreAV truncates that exact time to the tick,
    ffprobe first rounds the whole lace's duration down and splits it."""
    p = props(cstream)
    if cpacket is None or cpacket.get('lace', 0) <= 0 or p.get('default_duration_ns') is None:
        return False
    if not timestamp_scale or cpacket.get('pts') is None or rpacket.get('pts') is None:
        return False
    block, i, n = cpacket['block_timestamp'], cpacket['lace'], cpacket['frames_in_block']
    dd, scale = p['default_duration_ns'], timestamp_scale
    delay = round_half_away(Fraction(p.get('codec_delay_ns', 0), scale))
    ours = block + floor(Fraction(i * dd, scale)) - delay
    theirs = block + floor(Fraction(i * floor(Fraction(n * dd, scale)), n)) - delay
    return (cpacket['pts'] == ours and rpacket['pts'] == theirs and abs(ours - theirs) == 1
            and rpacket.get('dts') in (rpacket['pts'], None) and cpacket.get('dts') in (cpacket['pts'], None))


def audio_keyframe(cstream, rpacket, cpacket):
    """CoreAV reports the keyframe flag as stored; ffprobe marks every audio
    frame a keyframe."""
    return (cstream.get('kind') == 'audio' and cpacket is not None
            and cpacket.get('keyframe') is False and rpacket.get('keyframe') is True)


def vfw_timestamps(cstream, rpacket, cpacket):
    """A Video for Windows track's block timestamps are presentation times
    in Matroska (RFC 9559 11.2); ffprobe treats them as decode times."""
    return (props(cstream).get('codec_id') == 'V_MS/VFW/FOURCC' and cpacket is not None
            and rpacket.get('pts') is None and cpacket.get('dts') is None
            and rpacket.get('dts') == cpacket.get('pts') == cpacket.get('block_timestamp'))


def webm_webvtt_size(cstream, rpacket, cpacket):
    """A WebM WebVTT frame holds the cue identifier and settings lines before
    the payload; CoreAV reports the frame as stored, ffprobe the payload."""
    return (str(props(cstream).get('codec_id', '')).startswith('D_WEBVTT/') and cpacket is not None
            and cpacket.get('size', 0) >= rpacket.get('size', 0) + 2
            and cpacket.get('pts') == rpacket.get('pts'))


def encrypted_extra(cstream, cpacket):
    """CoreAV lists every stored frame of an encrypted track; ffprobe omits
    the frames whose signal byte marks them encrypted."""
    return props(cstream).get('encoding') == 'encrypted' and cpacket is not None


def segment_duration(reference, rstream, cstream):
    """Matroska stores no stream duration; ffprobe gives some streams
    (subtitle and Video for Windows tracks) the segment duration."""
    if rstream is None or cstream is None:
        return False
    ticks, base = rstream.get('duration_ts'), rstream.get('time_base')
    total = reference.get('format_duration_us')
    if cstream.get('duration_ts') is not None or ticks is None or not base or total is None:
        return False
    return round_half_away(Fraction(ticks * base[0] * 1_000_000, base[1])) == total


def format_rounding(reference, candidate):
    """CoreAV rounds Duration × TimestampScale to the nearest microsecond;
    ffprobe truncates it."""
    p = props(candidate)
    raw, scale = p.get('raw_duration'), p.get('timestamp_scale')
    if raw is None or not scale:
        return False
    nanoseconds = round_half_away(Fraction(raw) * scale)
    exact = Fraction(nanoseconds, 1000)
    return (candidate.get('format_duration_us') == round_half_away(exact)
            and reference.get('format_duration_us') == floor(exact)
            and exact.denominator != 1)


def unread_bitstream(cstream, rstream, field):
    """Profile, level and color of codecs whose bitstream CoreAV does not
    read (no stored configuration record holds them)."""
    return (cstream is not None and rstream is not None
            and cstream.get('codec_name') in UNREAD_BITSTREAMS and cstream.get(field) is None
            and rstream.get(field) is not None)


def hevc_unsignalled_range(cstream, rstream):
    """An HEVC parameter set without a video signal type; ffprobe reports the
    studio range."""
    return (cstream is not None and rstream is not None and cstream.get('codec_name') == 'hevc'
            and cstream.get('color_range') is None and rstream.get('color_range') == 'tv'
            and all(cstream.get(k) is None for k in ('color_primaries', 'color_transfer', 'color_matrix')))


def attachments(reference, candidate):
    """ffprobe lists attached files as streams after the tracks; CoreAV
    reports them as attachments."""
    rstreams, cstreams = reference.get('streams', []), candidate.get('streams', [])
    files = props(candidate).get('attachments') or []
    extra = rstreams[len(cstreams):]
    if not extra or len(extra) != len(files):
        return False
    images = any(str(f.get('media_type', '')).startswith('image/') for f in files)
    return all(s.get('kind') == 'attachment'
               or (images and s.get('kind') == 'video' and s.get('codec_name') in IMAGE_CODECS)
               for s in extra)


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


def declared_size(media_dir, file, index, rstream, cstream):
    """CoreAV's width and height are the track header's `PixelWidth` and
    `PixelHeight`; ffprobe's are the decoded pictures', which the codec's
    parameter sets or frame headers make larger or smaller."""
    if not media_dir or rstream is None or cstream is None:
        return False
    path = pathlib.Path(media_dir) / file
    headers = policy_ref.matroska_headers(path) or {}
    tracks = headers.get('tracks') or []
    declared = tracks[index].get('pixels') if 0 <= index < len(tracks) else None
    ours = (cstream.get('width'), cstream.get('height'))
    theirs = (rstream.get('width'), rstream.get('height'))
    return (declared is not None and ours == declared and theirs != declared
            and _first_frame_size(path, index) == theirs)


def tag_language(media_dir, file, index, rstream, cstream):
    """A track whose `Tags` hold a `LANGUAGE` SimpleTag (any case): ffprobe
    lists that tag under its own name and gives the stream no `language`
    tag; CoreAV's compat output prints the track's stored `Language`
    element, which the track has no `LanguageBCP47` to override. Both are
    checked against the file's EBML elements."""
    if not media_dir or rstream is None or cstream is None:
        return False
    ours = (cstream.get('tags') or {}).get('language')
    theirs = rstream.get('tags') or {}
    if ours is None or 'language' in theirs or not any(k.lower() == 'language' for k in theirs):
        return False
    headers = policy_ref.matroska_headers(pathlib.Path(media_dir) / file, tags=True) or {}
    tracks = headers.get('tracks') or []
    if not 0 <= index < len(tracks):
        return False
    track = tracks[index]
    # With `LanguageBCP47`, `Language` is to be ignored (RFC 9559
    # 5.1.4.1.19): no specification basis for printing it then.
    return (track.get('language') == ours and track.get('language_bcp47') is None
            and any(name.lower() == 'language' for name in track.get('tag_names', [])))


PREDICATES = {
    'tag_language': tag_language,
    'declared_size': declared_size,
    'dts_unknown': dts_unknown,
    'lace_without_duration': lace_without_duration,
    'lace_split': lace_split,
    'audio_keyframe': audio_keyframe,
    'vfw_timestamps': vfw_timestamps,
    'webm_webvtt_size': webm_webvtt_size,
    'encrypted_extra': encrypted_extra,
    'segment_duration': segment_duration,
    'format_rounding': format_rounding,
    'unread_bitstream': unread_bitstream,
    'hevc_unsignalled_range': hevc_unsignalled_range,
    'attachments': attachments,
}


# Entries for keys the stream oracle does not gate, which
# scripts/compat_check.py applies to `--ffprobe-compat` output:
# compat key -> (entry, predicate with the signature of `tag_language`).
COMPAT_ENTRIES = {'streams.tags.language': ('KD-MKV-TAG-LANGUAGE', tag_language)}


# ---------------------------------------------------------------- overlay

def stream_at(document, index):
    streams = document.get('streams', [])
    return streams[index] if 0 <= index < len(streams) else None


def stream_item(detail, reference, candidate, media_dir=None):
    # The entries that cover each differing field, for the compat check
    # (scripts/compat_check.py).
    ids, uncovered, paths = set(), [], {}
    for diff in detail['gating_differences']:
        path = diff['path']
        parts = path.split('/')
        if path == 'streams/count' and attachments(reference, candidate):
            ids.add('KD-MKV-ATTACHMENT')
            paths[path] = {'kd': ['KD-MKV-ATTACHMENT'], 'candidate': diff.get('candidate'), 'reference': diff.get('reference')}
            continue
        if path == 'format_duration_us' and format_rounding(reference, candidate):
            ids.add('KD-MKV-FMT-ROUNDING')
            paths[path] = {'kd': ['KD-MKV-FMT-ROUNDING'], 'candidate': diff.get('candidate'), 'reference': diff.get('reference')}
            continue
        if len(parts) == 3 and parts[0] == 'streams':
            index, field = int(parts[1]), parts[2]
            r, c = stream_at(reference, index), stream_at(candidate, index)
            if field == 'duration_ts' and segment_duration(reference, r, c):
                ids.add('KD-MKV-STREAM-DURATION')
                paths[path] = {'kd': ['KD-MKV-STREAM-DURATION'], 'candidate': diff.get('candidate'), 'reference': diff.get('reference')}
                continue
            if field in ('profile', 'level', 'color_range', 'color_matrix', 'color_primaries',
                         'color_transfer') and unread_bitstream(c, r, field):
                ids.add('KD-MKV-UNREAD-BITSTREAM')
                paths[path] = {'kd': ['KD-MKV-UNREAD-BITSTREAM'], 'candidate': diff.get('candidate'), 'reference': diff.get('reference')}
                continue
            if field == 'color_range' and hevc_unsignalled_range(c, r):
                ids.add('KD-MKV-HEVC-RANGE')
                paths[path] = {'kd': ['KD-MKV-HEVC-RANGE'], 'candidate': diff.get('candidate'), 'reference': diff.get('reference')}
                continue
            if field in ('width', 'height') and declared_size(media_dir, detail['file'], index,
                                                              r, c):
                ids.add('KD-MKV-DECLARED-SIZE')
                paths[path] = {'kd': ['KD-MKV-DECLARED-SIZE'], 'candidate': diff.get('candidate'), 'reference': diff.get('reference')}
                continue
        uncovered.append(path)
    return ('kd' if not uncovered else 'uncovered'), ids, uncovered, paths


def pair(reference_packets, candidate_packets):
    queues = collections.defaultdict(collections.deque)
    for p in candidate_packets:
        queues[p.get('pos')].append(p)
    return [queues[p.get('pos')].popleft() if queues[p.get('pos')] else None for p in reference_packets]


def packet_kd(cstream, rpacket, cpacket, fields, timestamp_scale):
    """The KD id covering every differing field of one paired packet."""
    timing = {'pts', 'dts'}
    if fields <= {'dts'} and dts_unknown(cstream, rpacket, cpacket):
        return 'KD-MKV-DTS'
    if fields <= timing and lace_without_duration(cstream, rpacket, cpacket):
        return 'KD-MKV-LACE-UNTIMED'
    if fields <= timing and lace_split(cstream, rpacket, cpacket, timestamp_scale):
        return 'KD-MKV-LACE-SPLIT'
    if fields <= {'keyframe'} and audio_keyframe(cstream, rpacket, cpacket):
        return 'KD-MKV-AUDIO-KEYFRAME'
    if fields <= timing and vfw_timestamps(cstream, rpacket, cpacket):
        return 'KD-MKV-VFW-TIMESTAMPS'
    if fields <= {'size'} and webm_webvtt_size(cstream, rpacket, cpacket):
        return 'KD-MKV-WEBM-WEBVTT'
    return None


def packet_units(detail, reference, candidate, stream_documents):
    """`stream_documents` are the two `streams` captures, which name the
    stream kinds the attachment predicate needs."""
    disagree = detail['packets_disagree']
    diffs = detail['gating_differences']
    if any(d['path'] == 'probe_ok' for d in diffs):
        return {}, disagree, ['probe_ok']
    attached = any(d['path'] == 'streams/count' for d in diffs) and attachments(*stream_documents)
    units = collections.defaultdict(list)
    extra_index = collections.Counter()
    for d in diffs:
        parts = d['path'].split('/')
        if d['path'] == 'streams/count' and attached:
            continue
        if len(parts) >= 4 and parts[0] == 'streams' and parts[2] == 'packets':
            stream = int(parts[1])
            if parts[3] in ('extra', 'missing'):
                extra_index[(stream, parts[3])] += 1
                units[(stream, parts[3], extra_index[(stream, parts[3])])].append(d)
            else:
                units[(stream, 'paired', int(parts[3]))].append(d)
        elif len(parts) == 3 and parts[0] == 'streams' and parts[2] == 'packets':
            units[(int(parts[1]), 'stream', 0)].append(d)
        else:
            return {}, disagree, [d['path']]
    rstreams, cstreams = reference.get('streams', []), candidate.get('streams', [])
    scale = props(candidate).get('timestamp_scale')
    pairs = {i: pair(rstreams[i].get('packets', []), cstreams[i].get('packets', []))
             for i in {k[0] for k in units} if i < len(rstreams) and i < len(cstreams)}
    covered, uncovered, notes = collections.Counter(), 0, []
    for (stream, kind, index), unit_diffs in units.items():
        kd, count = None, 1
        if stream >= len(cstreams) and attached:
            kd, count = 'KD-MKV-ATTACHMENT', max(1, len(unit_diffs))
        elif kind == 'extra' and stream < len(cstreams) and all(
                encrypted_extra(cstreams[stream], d['candidate']) for d in unit_diffs):
            kd = 'KD-MKV-ENCRYPTED'
        elif kind == 'paired' and stream in pairs:
            fields = {d['path'].split('/')[4] for d in unit_diffs if len(d['path'].split('/')) > 4}
            rpacket = rstreams[stream]['packets'][index]
            kd = packet_kd(cstreams[stream], rpacket, pairs[stream][index], fields, scale)
        if kd:
            covered[kd] += count
        else:
            uncovered += count
            if len(notes) < 5:
                notes.append(unit_diffs[0]['path'])
    total = sum(covered.values()) + uncovered
    if total != disagree:
        return {}, disagree, [f'unit count {total} differs from {disagree}']
    return dict(covered), uncovered, notes


def overlay(reports, reference_dir, candidate_dir, media_dir=None):
    entries = {e['id'] for e in json.loads(KD_FILE.read_text())['entries']}
    result = {'schema': 'coreav/kd-overlay/1', 'levels': {}}
    details = collections.defaultdict(list)
    for report in reports:
        for level in report['levels']:
            details[level['level']].extend(level['details'])
    for name, level_details in details.items():
        groups = {scope: {'items': 0, 'direct': 0, 'via_kd': 0, 'uncovered': 0, 'by_kd': collections.Counter(),
                          'reference_failures': 0, 'uncovered_items': [], 'covered_items': []}
                  for scope in ('in_scope', 'all')}
        for detail in level_details:
            file = detail['file']
            scopes = ['all'] + (['in_scope'] if file.endswith(IN_SCOPE) else [])
            if detail.get('reference_failure'):
                for scope in scopes:
                    groups[scope]['reference_failures'] += 1
                continue
            reference, candidate = load(reference_dir, file, name), load(candidate_dir, file, name)
            if name == 'streams':
                if detail['agrees']:
                    status, ids, missing, covered_paths = 'direct', set(), [], {}
                else:
                    status, ids, missing, covered_paths = stream_item(detail, reference, candidate, media_dir)
                for scope in scopes:
                    g = groups[scope]
                    g['items'] += 1
                    g[{'direct': 'direct', 'kd': 'via_kd', 'uncovered': 'uncovered'}[status]] += 1
                    g['by_kd'].update(ids if status == 'kd' else ())
                    if status == 'kd':
                        g['covered_items'].append({'file': file, 'kd': sorted(ids), 'paths': covered_paths})
                    if status == 'uncovered':
                        g['uncovered_items'].append({'file': file, 'fields': missing})
            else:
                stream_documents = (load(reference_dir, file, 'streams'), load(candidate_dir, file, 'streams'))
                covered, uncovered, notes = ({}, 0, []) if detail['agrees'] else packet_units(
                    detail, reference, candidate, stream_documents)
                direct = detail['packets_agree']
                for scope in scopes:
                    g = groups[scope]
                    g['items'] += direct + detail['packets_disagree']
                    g['direct'] += direct
                    g['via_kd'] += sum(covered.values())
                    g['uncovered'] += uncovered
                    g['by_kd'].update(covered)
                    if uncovered:
                        g['uncovered_items'].append({'file': file, 'units': uncovered, 'examples': notes})
        for g in groups.values():
            unknown = set(g['by_kd']) - entries
            if unknown:
                raise SystemExit(f'undocumented divergence ids: {sorted(unknown)}')
            g['by_kd'] = dict(sorted(g['by_kd'].items()))
            g['agreement'] = (g['direct'] + g['via_kd']) / g['items'] if g['items'] else None
            g['passed'] = g['items'] > 0 and g['uncovered'] == 0
        result['levels'][name] = groups
    result['passed'] = all(levels['in_scope']['passed'] for levels in result['levels'].values())
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report', required=True, action='append')
    parser.add_argument('--reference', required=True)
    parser.add_argument('--candidate', required=True)
    parser.add_argument('--out', required=True)
    parser.add_argument('--media', help='directory of the captured files (KD-MKV-DECLARED-SIZE)')
    args = parser.parse_args()
    try:
        reports = [json.loads(pathlib.Path(r).read_text()) for r in args.report]
        result = overlay(reports, args.reference, args.candidate, args.media)
    except (OSError, ValueError, KeyError) as error:
        print(json.dumps({'error': str(error)}), file=sys.stderr)
        return 2
    pathlib.Path(args.out).write_text(json.dumps(result, indent=2) + '\n')
    for level, groups in result['levels'].items():
        for scope, g in groups.items():
            print(f"{level:8} {scope:9} items {g['items']:>8}  direct {g['direct']:>8}  via KD {g['via_kd']:>8}"
                  f"  uncovered {g['uncovered']:>6}  {g['by_kd']}")
    print('passed' if result['passed'] else 'FAILED')
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
