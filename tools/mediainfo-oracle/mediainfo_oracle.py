#!/usr/bin/env python3
"""MediaInfo as a second, informational black-box oracle.

Runs the pinned MediaInfo CLI on media files, normalizes its stream-level
fields into the vocabulary of the stream oracle (tools/oracle/SPEC.md), and
writes a three-way report: CoreAV against ffprobe against MediaInfo. The
report never gates: the command exits 0 whatever the agreement and 2 only on
usage or harness errors.

Standard library only. MediaInfo is launched without a shell; its source code
is never read. Usage:

  mediainfo_oracle.py capture --media DIR --out DIR [--mediainfo EXE] [--raw DIR]
  mediainfo_oracle.py normalize --input RAW.json --file NAME
  mediainfo_oracle.py report --ffprobe DIR --coreav DIR --mediainfo DIR
                             --out REPORT.json [--markdown REPORT.md] [--private]
  mediainfo_oracle.py corpus ...        (see corpus.py)
"""
import argparse
import collections
import json
import pathlib
import subprocess
import sys
from decimal import Decimal, InvalidOperation
from fractions import Fraction

SCHEMA = 'coreav-mediainfo/streams/1'
REPORT_SCHEMA = 'coreav-mediainfo/three-way/1'
TIMEOUT = 120

# --------------------------------------------------------------- vocabulary

KINDS = {'Video': 'video', 'Image': 'video', 'Audio': 'audio', 'Text': 'subtitle', 'Other': 'data'}

# MediaInfo `Format` -> ffprobe codec name (the codec family vocabulary).
# Entries are added only for names seen in MediaInfo output on fixtures or
# non-holdout corpus files; anything else stays `mi:<Format>`.
CODECS = {
    'AVC': 'h264',
    'HEVC': 'hevc',
    'AV1': 'av1',
    'VP9': 'vp9',
    'VP8': 'vp8',
    'ProRes': 'prores',
    'AAC': 'aac',
    'Opus': 'opus',
    'PCM': 'pcm',
    'Timed Text': 'mov_text',
    # Added from development-split output (fixtures, edge and non-holdout real
    # files), before the holdout run; frozen with the holdout report.
    'AC-3': 'ac3',
    'AC-4': 'ac4',
    'ALAC': 'alac',
    'ALS': 'mp4als',
    'ASS': 'ass',
    'Cinepak': 'cinepak',
    'DTS': 'dts',
    'DV': 'dvvideo',
    'E-AC-3': 'eac3',
    'ER AAC ELD': 'aac',
    'FLAC': 'flac',
    'H.261': 'h261',
    'H.263': 'h263',
    'JPEG': 'mjpeg',
    'JPEG 2000': 'jpeg2000',
    'MACE 3': 'mace3',
    'MACE 6': 'mace6',
    'MLP FBA': 'truehd',
    'MPEG-4 Visual': 'mpeg4',
    'PGS': 'hdmv_pgs_subtitle',
    'PNG': 'png',
    'QCELP': 'qcelp',
    'Qdesign 2': 'qdm2',
    'Sorenson 1': 'svq1',
    'Sorenson 3': 'svq3',
    'SSA': 'ssa',
    'Theora': 'theora',
    'TTA': 'tta',
    'UTF-8': 'subrip',
    'VC-1': 'vc1',
    'VC-3': 'dnxhd',
    'VobSub': 'dvd_subtitle',
    'Vorbis': 'vorbis',
    'VVC': 'vvc',
}
# MPEG audio is one `Format`; the layer is in `Format_Profile`.
MPEG_AUDIO_LAYERS = {'Layer 1': 'mp1', 'Layer 2': 'mp2', 'Layer 3': 'mp3'}

# Colour names through their ITU-T H.273 code points to ffprobe's names.
PRIMARIES = {
    'BT.709': 'bt709',             # 1
    'BT.470 System M': 'bt470m',   # 4
    'BT.601 PAL': 'bt470bg',       # 5
    'BT.601 NTSC': 'smpte170m',    # 6
    'SMPTE 240M': 'smpte240m',     # 7
    'BT.2020': 'bt2020',           # 9
    'XYZ': 'smpte428',             # 10
    'DCI P3': 'smpte431',          # 11
    'Display P3': 'smpte432',      # 12
    'EBU Tech 3213': 'ebu3213',    # 22
}
TRANSFERS = {
    'BT.709': 'bt709',                      # 1
    'BT.470 System M': 'bt470m',            # 4
    'BT.470 System B/G': 'bt470bg',         # 5
    'BT.601': 'smpte170m',                  # 6
    'SMPTE 240M': 'smpte240m',              # 7
    'Linear': 'linear',                     # 8
    'Logarithmic (100:1)': 'log100',        # 9
    'xvYCC': 'iec61966-2-4',                # 11
    'sRGB/sYCC': 'iec61966-2-1',            # 13
    'BT.2020 (10-bit)': 'bt2020-10',        # 14
    'BT.2020 (12-bit)': 'bt2020-12',        # 15
    'PQ': 'smpte2084',                      # 16
    'HLG': 'arib-std-b67',                  # 18
}
MATRICES = {
    'Identity': 'gbr',                   # 0
    'BT.709': 'bt709',                   # 1
    'BT.470 System B/G': 'bt601',        # 5
    'BT.601': 'bt601',                   # 5 or 6, same definition in H.273
    'SMPTE 240M': 'smpte240m',           # 7
    'BT.2020 non-constant': 'bt2020nc',  # 9
}
RANGES = {'Limited': 'tv', 'Full': 'pc'}
# H.273 matrix coefficients 5 (bt470bg) and 6 (smpte170m) have one definition.
MATRIX_CLASS = {'bt470bg': 'bt601', 'smpte170m': 'bt601'}
# ISO 639-2 bibliographic codes and their terminology twins (the standard
# lists both); all sides compare on the terminology code against MediaInfo.
LANGUAGE_TWINS = {
    'alb': 'sqi', 'arm': 'hye', 'baq': 'eus', 'bur': 'mya', 'chi': 'zho', 'cze': 'ces', 'dut': 'nld',
    'fre': 'fra', 'geo': 'kat', 'ger': 'deu', 'gre': 'ell', 'ice': 'isl', 'mac': 'mkd', 'mao': 'mri',
    'may': 'msa', 'per': 'fas', 'rum': 'ron', 'slo': 'slk', 'tib': 'bod', 'wel': 'cym',
}

# Stream oracle fields compared, in report order; `gating` follows the
# oracle's correctness policy.
FILE_FIELDS = [('probe_ok', True), ('stream_count', True), ('format_duration', True)]
STREAM_FIELDS = [
    ('kind', True), ('codec_fourcc', True), ('codec_name', True), ('width', True), ('height', True),
    ('rotation', True), ('duration', True), ('color_primaries', True), ('color_transfer', True),
    ('color_matrix', True), ('color_range', True), ('sample_rate', False), ('channels', False),
    ('language', False), ('sample_aspect_ratio', False), ('display_aspect_ratio', False),
]
NOT_OBSERVABLE = ['timescale', 'handler_name']
MS = Fraction(1, 1000)
ASPECT_TOLERANCE = Fraction(5, 10000)
CLASSES = ['all-agree', 'coreav-alone', 'ffprobe-alone', 'mediainfo-alone', 'mediainfo-between',
           'mediainfo-split', 'all-differ', 'not-comparable']


# ------------------------------------------------------------ normalization

def unknown(raw):
    return f'mi:{raw}'


def integer(raw):
    if raw is None:
        return None
    try:
        return int(str(raw))
    except ValueError:
        return unknown(raw)


def decimal_text(raw):
    """Keep a decimal as printed (string) so no rounding happens here."""
    if raw is None:
        return None
    try:
        Decimal(str(raw))
    except InvalidOperation:
        return unknown(raw)
    return str(raw)


def rotation(track):
    raw = track.get('Rotation')
    if raw is None:
        return 0 if track.get('@type') == 'Video' else None
    try:
        value = Decimal(str(raw))
    except InvalidOperation:
        return unknown(raw)
    if value != value.to_integral_value():
        return None
    # MediaInfo prints clockwise degrees; the stream oracle uses counter-clockwise.
    return int(-value) % 360


def codec_family(track):
    fmt = track.get('Format')
    if fmt is None:
        return None
    if fmt == 'MPEG Audio':
        return MPEG_AUDIO_LAYERS.get(track.get('Format_Profile'), unknown(f"{fmt} {track.get('Format_Profile')}"))
    return CODECS.get(fmt, unknown(fmt))


def fourcc(track):
    raw = track.get('CodecID')
    if raw is None:
        return None
    head = str(raw).split('-', 1)[0]
    return head if len(head) == 4 else None


def mapped(table, raw):
    return None if raw is None else table.get(raw, unknown(raw))


def language(track):
    raw = track.get('Language_String3') or track.get('Language')
    return None if raw in (None, 'und') else raw


def stream_order(track):
    raw = track.get('StreamOrder')
    try:
        return int(str(raw)) if raw is not None and str(raw).isdigit() else None
    except ValueError:
        return None


def normalize_track(track, index):
    kind_raw = track.get('@type')
    return {
        'index': index,
        'kind': KINDS.get(kind_raw, unknown(kind_raw)),
        'codec_fourcc': fourcc(track),
        'codec_name': codec_family(track),
        'width': integer(track.get('Width')),
        'height': integer(track.get('Height')),
        'rotation': rotation(track) if kind_raw in ('Video', 'Image') else None,
        'duration_s': decimal_text(track.get('Duration')),
        'source_duration_s': decimal_text(track.get('Source_Duration')),
        'color_primaries': mapped(PRIMARIES, track.get('colour_primaries')),
        'color_transfer': mapped(TRANSFERS, track.get('transfer_characteristics')),
        'color_matrix': mapped(MATRICES, track.get('matrix_coefficients')),
        'color_range': mapped(RANGES, track.get('colour_range')),
        'sample_rate': integer(track.get('SamplingRate')),
        'channels': integer(track.get('Channels')),
        'language': language(track),
        'sample_aspect_ratio': decimal_text(track.get('PixelAspectRatio')),
        'display_aspect_ratio': decimal_text(track.get('DisplayAspectRatio')),
    }


def has_conformance_errors(track):
    extra = track.get('extra') or {}
    return any(key.startswith('Conformance') for key in extra)


def normalize(raw, name, failure=None):
    """Turn MediaInfo `--Output=JSON --Full` output into the normalized record."""
    record = {
        'schema': SCHEMA, 'file': name, 'probe_ok': False, 'failure': failure, 'version': None,
        'format_duration_s': None, 'truncated': False, 'conformance_errors': False,
        'streams': [], 'unpaired_tracks': [],
    }
    if failure is not None:
        return record
    if not isinstance(raw, dict):
        record['failure'] = 'invalid-json'
        return record
    library = raw.get('creatingLibrary') or {}
    record['version'] = library.get('version') if isinstance(library, dict) else None
    media = raw.get('media')
    tracks = media.get('track') if isinstance(media, dict) else None
    if not isinstance(tracks, list) or not tracks or not isinstance(tracks[0], dict):
        record['failure'] = 'unrecognized'
        return record
    general = next((t for t in tracks if isinstance(t, dict) and t.get('@type') == 'General'), None)
    if general is None or general.get('Format') is None:
        record['failure'] = 'unrecognized'
        return record
    record['probe_ok'] = True
    record['format_duration_s'] = decimal_text(general.get('Duration'))
    extra = general.get('extra') or {}
    record['truncated'] = extra.get('IsTruncated') == 'Yes'
    record['conformance_errors'] = any(has_conformance_errors(t) for t in tracks if isinstance(t, dict))
    seen = set()
    for track in tracks:
        if not isinstance(track, dict) or track.get('@type') == 'General':
            continue
        order = stream_order(track)
        if order is None or order in seen:
            record['unpaired_tracks'].append({
                'type': track.get('@type'), 'stream_order': track.get('StreamOrder'),
                'format': track.get('Format'),
                'reason': 'duplicate-stream-order' if order is not None else 'no-integer-stream-order',
            })
            continue
        seen.add(order)
        record['streams'].append(normalize_track(track, order))
    record['streams'].sort(key=lambda s: s['index'])
    return record


def run_mediainfo(exe, media_file, timeout=TIMEOUT):
    """Return (raw JSON or None, failure or None). Never raises for media problems."""
    try:
        done = subprocess.run([exe, '--Output=JSON', '--Full', str(media_file)], capture_output=True,
                              timeout=timeout, check=False)
    except subprocess.TimeoutExpired:
        return None, 'timeout'
    if done.returncode < 0:
        return None, f'signal-{-done.returncode}'
    if done.returncode != 0:
        return None, f'exit-{done.returncode}'
    try:
        return json.loads(done.stdout.decode('utf-8', errors='replace')), None
    except json.JSONDecodeError:
        return None, 'invalid-json'


def capture_one(exe, media_file, name, raw_out=None):
    raw, failure = run_mediainfo(exe, media_file)
    if raw_out is not None and raw is not None:
        raw_out.parent.mkdir(parents=True, exist_ok=True)
        raw_out.write_text(json.dumps(raw, indent=1) + '\n')
    return normalize(raw, name, failure)


def media_names(media_dir):
    """Generated fixtures from MANIFEST when present, else every regular file."""
    manifest = media_dir / 'MANIFEST'
    if manifest.exists():
        names = []
        for line in manifest.read_text().splitlines():
            parts = line.split()
            if len(parts) == 2 and parts[0] == 'generated':
                names.append(parts[1])
        return names
    return sorted(str(p.relative_to(media_dir)) for p in media_dir.rglob('*')
                  if p.is_file() and p.name != 'MANIFEST' and not p.name.endswith('.log'))


# ------------------------------------------------------- comparison views

def fraction_of(text):
    if text is None or (isinstance(text, str) and text.startswith('mi:')):
        return text
    return Fraction(Decimal(text))


def view_oracle(record):
    """A view of a coreav-oracle/streams/1 record (ffprobe or CoreAV)."""
    if not record or not record.get('probe_ok'):
        return {'probe_ok': False, 'streams': {}}
    streams = {}
    for s in record.get('streams') or []:
        tb = s.get('time_base')
        duration = None
        if s.get('duration_ts') is not None:
            # (ticks, seconds): CoreAV and ffprobe compare ticks exactly, as the
            # stream oracle does; MediaInfo compares against the seconds.
            seconds = s['duration_ts'] * Fraction(tb[0], tb[1]) if tb and tb[1] else None
            duration = (s['duration_ts'], seconds)
        language_value = s.get('language')
        streams[s.get('index')] = {
            'kind': s.get('kind'), 'codec_fourcc': s.get('codec_fourcc'), 'codec_name': s.get('codec_name'),
            'width': s.get('width'), 'height': s.get('height'), 'rotation': s.get('rotation'),
            'duration': duration, 'color_primaries': s.get('color_primaries'),
            'color_transfer': s.get('color_transfer'), 'color_matrix': s.get('color_matrix'),
            'color_range': s.get('color_range'), 'sample_rate': s.get('sample_rate'),
            'channels': s.get('channels'), 'language': language_value,
            'sample_aspect_ratio': ratio(s.get('sample_aspect_ratio')),
            'display_aspect_ratio': ratio(s.get('display_aspect_ratio')),
        }
    us = record.get('format_duration_us')
    return {
        'probe_ok': True, 'stream_count': len(record.get('streams') or []),
        'format_duration': None if us is None else Fraction(us, 1_000_000), 'streams': streams,
    }


def ratio(pair):
    if not pair or not pair[1]:
        return None
    return Fraction(pair[0], pair[1])


REMAP = {'kind': KINDS, 'codec_name': CODECS, 'color_primaries': PRIMARIES, 'color_transfer': TRANSFERS,
         'color_matrix': MATRICES, 'color_range': RANGES}


def remap(field, value):
    """Apply the current tables to a value captured as `mi:<raw>`.

    Captures keep unmapped names verbatim, so extending a table changes the
    report without capturing again.
    """
    if not isinstance(value, str) or not value.startswith('mi:'):
        return value
    raw = value[3:]
    if raw in REMAP.get(field, {}):
        return REMAP[field][raw]
    if field == 'codec_name' and raw.startswith('MPEG Audio '):
        return MPEG_AUDIO_LAYERS.get(raw[len('MPEG Audio '):], value)
    return value


def view_mediainfo(record):
    if not record or not record.get('probe_ok'):
        return {'probe_ok': False, 'streams': {}}
    streams = {}
    for s in record['streams']:
        streams[s['index']] = {
            **{k: remap(k, s[k]) for k in ('kind', 'codec_fourcc', 'codec_name', 'width', 'height', 'rotation',
                                 'color_primaries', 'color_transfer', 'color_matrix', 'color_range',
                                 'sample_rate', 'channels', 'language')},
            'duration': fraction_of(s['duration_s']),
            'source_duration': fraction_of(s.get('source_duration_s')),
            'sample_aspect_ratio': fraction_of(s['sample_aspect_ratio']),
            'display_aspect_ratio': fraction_of(s['display_aspect_ratio']),
        }
    return {
        'probe_ok': True, 'stream_count': len(record['streams']),
        'format_duration': fraction_of(record['format_duration_s']), 'streams': streams,
    }


def with_mediainfo_vocabulary(field, value):
    """Map an ffprobe or CoreAV value to the vocabulary used against MediaInfo."""
    if value is None:
        return None
    if field == 'codec_name' and isinstance(value, str) and value.startswith('pcm_'):
        return 'pcm'
    if field == 'color_matrix':
        return MATRIX_CLASS.get(value, value)
    if field == 'language':
        return None if value == 'und' else LANGUAGE_TWINS.get(value, value)
    return value


def seconds_of(value):
    return value[1] if isinstance(value, tuple) else value


def equal(field, a, b, mediainfo):
    """Pairwise agreement. Without MediaInfo it is the stream oracle's exact rule."""
    if field == 'duration':
        if not mediainfo:
            return (a[0] if isinstance(a, tuple) else a) == (b[0] if isinstance(b, tuple) else b)
        a, b = seconds_of(a), seconds_of(b)
    if mediainfo:
        a = with_mediainfo_vocabulary(field, a)
        b = with_mediainfo_vocabulary(field, b)
    if a is None or b is None:
        return a is None and b is None
    numeric = isinstance(a, Fraction) and isinstance(b, Fraction)
    if mediainfo and numeric and field in ('duration', 'format_duration'):
        return abs(a - b) <= MS
    if mediainfo and numeric and field in ('sample_aspect_ratio', 'display_aspect_ratio'):
        return abs(a - b) <= ASPECT_TOLERANCE
    return a == b


NA = object()


PAIRS = ('coreav_vs_ffprobe', 'mediainfo_vs_coreav', 'mediainfo_vs_ffprobe')


def pairwise(field, c, f, m):
    """Each pair's own verdict; `None` when a side of that pair has no value."""
    def pair(a, b, mediainfo):
        return None if a is NA or b is NA else equal(field, a, b, mediainfo)
    return {'coreav_vs_ffprobe': pair(c, f, False), 'mediainfo_vs_coreav': pair(c, m, True),
            'mediainfo_vs_ffprobe': pair(f, m, True)}


def classify(field, c, f, m):
    pairs = pairwise(field, c, f, m)
    if None in pairs.values():
        return 'not-comparable'
    cf, cm, fm = (pairs[k] for k in PAIRS)
    if cf:
        if cm and fm:
            return 'all-agree'
        # CoreAV and ffprobe agree by the oracle's exact rule (for example equal
        # ticks in different time bases) while MediaInfo matches only one.
        return 'mediainfo-split' if (cm or fm) else 'mediainfo-alone'
    if cm and fm:
        return 'mediainfo-between'
    if cm:
        return 'ffprobe-alone'
    if fm:
        return 'coreav-alone'
    return 'all-differ'


def file_value(view, field):
    if field == 'probe_ok':
        return view['probe_ok']
    return view[field] if view['probe_ok'] else NA


def stream_value(view, index, field):
    stream = view['streams'].get(index)
    return NA if stream is None else stream.get(field)


def display(value):
    if value is NA:
        return 'n/a'
    if isinstance(value, tuple):
        value = value[1]
    if isinstance(value, Fraction):
        return str(float(value)) if value.denominator != 1 else str(value.numerator)
    if isinstance(value, str) and not value.isprintable():
        return json.dumps(value)[1:-1]
    return value


def make_item(name, stream, field, gating, values):
    c, f, m = values['coreav'], values['ffprobe'], values['mediainfo']
    return {'file': name, 'stream': stream, 'field': field, 'gating': gating, 'class': classify(field, c, f, m),
            'pairs': pairwise(field, c, f, m), 'values': {k: display(v) for k, v in values.items()}}


def compare_file(name, ffprobe, coreav, mediainfo):
    """Return one item per (file field) and per (stream, field)."""
    views = {'coreav': view_oracle(coreav), 'ffprobe': view_oracle(ffprobe), 'mediainfo': view_mediainfo(mediainfo)}
    items = []
    for field, gating in FILE_FIELDS:
        values = {side: file_value(v, field) for side, v in views.items()}
        items.append(make_item(name, None, field, gating, values))
    indices = sorted(set().union(*(v['streams'].keys() for v in views.values())), key=lambda i: (i is None, i))
    for index in indices:
        for field, gating in STREAM_FIELDS:
            values = {side: stream_value(v, index, field) for side, v in views.items()}
            item = make_item(name, index, field, gating, values)
            if field == 'duration' and item['class'] not in ('all-agree', 'not-comparable'):
                source = stream_value(views['mediainfo'], index, 'source_duration')
                if source not in (None, NA):
                    item['mediainfo_source_duration'] = display(source)
                    item['source_duration_matches'] = [
                        side for side in ('coreav', 'ffprobe')
                        if values[side] is not NA and equal(field, values[side], source, True)]
            items.append(item)
    notes = {
        'mediainfo_failure': (mediainfo or {}).get('failure'),
        'mediainfo_truncated': bool((mediainfo or {}).get('truncated')),
        'mediainfo_conformance_errors': bool((mediainfo or {}).get('conformance_errors')),
        'mediainfo_unpaired_tracks': len((mediainfo or {}).get('unpaired_tracks') or []),
        'coreav_probe_ok': views['coreav']['probe_ok'], 'ffprobe_probe_ok': views['ffprobe']['probe_ok'],
        'mediainfo_probe_ok': views['mediainfo']['probe_ok'],
    }
    return items, notes


# ----------------------------------------------------------------- report

def summarize(items, notes):
    """Aggregate counts only; no file names or values."""
    by_field = collections.OrderedDict()
    for field, gating in FILE_FIELDS + STREAM_FIELDS:
        by_field[field] = {'gating': gating, **{c: 0 for c in CLASSES}}
    two_way = {field: {'coreav_vs_ffprobe': [0, 0], 'mediainfo_vs_coreav': [0, 0],
                       'mediainfo_vs_ffprobe': [0, 0]} for field in by_field}
    triage = {field: collections.Counter() for field, gating in FILE_FIELDS + STREAM_FIELDS if gating}
    patterns = collections.defaultdict(collections.Counter)
    source_rescue = collections.Counter()
    for item in items:
        field = item['field']
        by_field[field][item['class']] += 1
        v = item['values']
        for key, ok in item['pairs'].items():
            if ok is not None:
                two_way[field][key][0] += ok
                two_way[field][key][1] += 1
        # Triage only real CoreAV and ffprobe disagreements, whatever MediaInfo did.
        if item['gating'] and item['pairs']['coreav_vs_ffprobe'] is False:
            triage[field][item['class']] += 1
        if item['class'] not in ('all-agree', 'not-comparable') and field in ENUMERATED:
            key = f"coreav={v['coreav']} ffprobe={v['ffprobe']} mediainfo={v['mediainfo']}"
            patterns[field][key] += 1
        if 'source_duration_matches' in item:
            source_rescue[item['class'] + ':' + ('+'.join(item['source_duration_matches']) or 'none')] += 1
    unknown_names = collections.Counter()
    for item in items:
        value = item['values']['mediainfo']
        if isinstance(value, str) and value.startswith('mi:'):
            unknown_names[f"{item['field']}={value}"] += 1
    probe = collections.Counter(
        ('coreav ok' if n['coreav_probe_ok'] else 'coreav fail') + ', '
        + ('ffprobe ok' if n['ffprobe_probe_ok'] else 'ffprobe fail') + ', '
        + ('mediainfo ok' if n['mediainfo_probe_ok'] else 'mediainfo fail') for n in notes)
    rejected = [n for n in notes if not n['coreav_probe_ok'] and n['ffprobe_probe_ok']]
    return {
        'files': len(notes),
        'probe_outcomes': dict(sorted(probe.items())),
        'mediainfo_failures': dict(collections.Counter(n['mediainfo_failure'] for n in notes if n['mediainfo_failure'])),
        'coreav_rejects_ffprobe_reads': {
            'files': len(rejected),
            'mediainfo_flags_truncated': sum(n['mediainfo_truncated'] for n in rejected),
            'mediainfo_flags_conformance_errors': sum(n['mediainfo_conformance_errors'] for n in rejected),
            'mediainfo_fails': sum(not n['mediainfo_probe_ok'] for n in rejected),
        },
        'files_with_mediainfo_unpaired_tracks': sum(1 for n in notes if n['mediainfo_unpaired_tracks']),
        'triage': {field: dict(counter) for field, counter in triage.items()},
        'by_field': by_field,
        'two_way': two_way,
        'patterns': {field: dict(counter.most_common(15)) for field, counter in patterns.items()},
        'duration_source_reading': dict(source_rescue),
        'mediainfo_unmapped_names': dict(unknown_names.most_common()),
        'not_observable': NOT_OBSERVABLE,
    }


ENUMERATED = {'probe_ok', 'kind', 'codec_fourcc', 'codec_name', 'rotation', 'color_primaries',
              'color_transfer', 'color_matrix', 'color_range', 'language'}

TRIAGE_COLUMNS = [
    ('ffprobe-alone', 'MediaInfo agrees with CoreAV'),
    ('coreav-alone', 'MediaInfo agrees with ffprobe'),
    ('mediainfo-between', 'MediaInfo within tolerance of both'),
    ('all-differ', 'MediaInfo differs from both'),
    ('not-comparable', 'MediaInfo has no value'),
]


def percent(pair):
    agree, total = pair
    return '–' if total == 0 else f'{agree}/{total} ({100 * agree / total:.1f}%)'


def markdown(summary, title):
    lines = [f'# {title}', '', f"Files: {summary['files']}.", '']
    lines += ['## Probe outcomes', '', '| CoreAV, ffprobe, MediaInfo | Files |', '|---|---|']
    lines += [f'| {k} | {v} |' for k, v in summary['probe_outcomes'].items()]
    r = summary['coreav_rejects_ffprobe_reads']
    lines += ['', f"CoreAV rejects, ffprobe reads: {r['files']} files; MediaInfo flags truncation on "
              f"{r['mediainfo_flags_truncated']}, conformance errors on {r['mediainfo_flags_conformance_errors']}, "
              f"fails on {r['mediainfo_fails']}.", '']
    lines += ['## CoreAV and ffprobe disagreements on gating fields, by MediaInfo reading', '',
              '| Field | ' + ' | '.join(label for _, label in TRIAGE_COLUMNS) + ' |',
              '|---|' + '---|' * len(TRIAGE_COLUMNS)]
    for field, counts in summary['triage'].items():
        if counts:
            lines.append(f'| `{field}` | ' + ' | '.join(str(counts.get(c, 0)) for c, _ in TRIAGE_COLUMNS) + ' |')
    lines += ['', '## Pairwise agreement per field', '',
              '| Field | Gating | CoreAV = ffprobe | MediaInfo = CoreAV | MediaInfo = ffprobe |', '|---|---|---|---|---|']
    for field, pairs in summary['two_way'].items():
        gating = 'yes' if summary['by_field'][field]['gating'] else 'no'
        lines.append(f'| `{field}` | {gating} | {percent(pairs["coreav_vs_ffprobe"])} | '
                     f'{percent(pairs["mediainfo_vs_coreav"])} | {percent(pairs["mediainfo_vs_ffprobe"])} |')
    lines += ['', '## Three-way classes per field', '', '| Field | ' + ' | '.join(CLASSES) + ' |',
              '|---|' + '---|' * len(CLASSES)]
    for field, counts in summary['by_field'].items():
        lines.append(f'| `{field}` | ' + ' | '.join(str(counts[c]) for c in CLASSES) + ' |')
    if summary['patterns']:
        lines += ['', '## Most frequent disagreement patterns (enumerated fields)', '']
        for field, patterns in summary['patterns'].items():
            lines.append(f'- `{field}`')
            lines += [f'  - {count} × {pattern}' for pattern, count in patterns.items()]
    if summary['duration_source_reading']:
        lines += ['', 'Stream duration disagreements where MediaInfo also printed `Source_Duration` '
                  '(class: sides matching that second reading):', '']
        lines += [f'- {k}: {v}' for k, v in sorted(summary['duration_source_reading'].items())]
    if summary['mediainfo_unmapped_names']:
        lines += ['', '## MediaInfo values outside the mapping tables', '']
        lines += [f'- {k}: {v}' for k, v in summary['mediainfo_unmapped_names'].items()]
    lines += ['', f"Not observable from MediaInfo: {', '.join(summary['not_observable'])}.", '']
    return '\n'.join(lines)


def load_json(path):
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def three_way(names, load):
    """`load(name)` returns (ffprobe, coreav, mediainfo) records."""
    items, notes = [], []
    for name in names:
        ffprobe, coreav, mediainfo = load(name)
        file_items, file_notes = compare_file(name, ffprobe, coreav, mediainfo)
        items += file_items
        notes.append({'file': name, **file_notes})
    return items, notes


# -------------------------------------------------------------------- CLI

def cmd_capture(args):
    media = pathlib.Path(args.media)
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    for name in media_names(media):
        raw_out = pathlib.Path(args.raw) / f'{name}.json' if args.raw else None
        record = capture_one(args.mediainfo, media / name, name, raw_out)
        target = out / f'{name}.mediainfo.json'
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(record, indent=1, sort_keys=True) + '\n')
        if not record['probe_ok']:
            print(f'mediainfo_oracle: MediaInfo did not read {name} ({record["failure"]})', file=sys.stderr)
    return 0


def cmd_normalize(args):
    raw = load_json(pathlib.Path(args.input))
    print(json.dumps(normalize(raw, args.file, None if raw is not None else 'invalid-json'), indent=1, sort_keys=True))
    return 0


def cmd_report(args):
    ffprobe_dir, coreav_dir, mi_dir = (pathlib.Path(p) for p in (args.ffprobe, args.coreav, args.mediainfo))
    names = sorted(str(p.relative_to(ffprobe_dir))[:-len('.streams.json')]
                   for p in ffprobe_dir.rglob('*.streams.json'))
    if not names:
        print('mediainfo_oracle: no ffprobe captures found', file=sys.stderr)
        return 2

    def load(name):
        return (load_json(ffprobe_dir / f'{name}.streams.json'), load_json(coreav_dir / f'{name}.streams.json'),
                load_json(mi_dir / f'{name}.mediainfo.json'))
    items, notes = three_way(names, load)
    summary = summarize(items, notes)
    report = {'schema': REPORT_SCHEMA, 'gating': False, 'summary': summary}
    if not args.private:
        report['items'] = [i for i in items if i['class'] != 'all-agree']
        report['notes'] = notes
    pathlib.Path(args.out).write_text(json.dumps(report, indent=1) + '\n')
    if args.markdown:
        text = markdown(summary, args.title)
        if not args.private:
            text += fixture_details(items)
        pathlib.Path(args.markdown).write_text(text)
    print(json.dumps(summary['triage'], sort_keys=True))
    return 0


def fixture_details(items):
    rows = [i for i in items if i['class'] not in ('all-agree', 'not-comparable')]
    if not rows:
        return ''
    lines = ['## Disagreements by file', '', '| File | Stream | Field | Class | CoreAV | ffprobe | MediaInfo |',
             '|---|---|---|---|---|---|---|']
    for i in rows:
        v = i['values']
        stream = '' if i['stream'] is None else i['stream']
        lines.append(f"| {i['file']} | {stream} | `{i['field']}` | {i['class']} | {v['coreav']} | {v['ffprobe']} | "
                     f"{v['mediainfo']} |")
    return '\n'.join(lines) + '\n'


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n', 1)[0])
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('capture')
    p.add_argument('--media', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--raw')
    p.add_argument('--mediainfo', default='mediainfo')
    p.set_defaults(run=cmd_capture)
    p = sub.add_parser('normalize')
    p.add_argument('--input', required=True)
    p.add_argument('--file', required=True)
    p.set_defaults(run=cmd_normalize)
    p = sub.add_parser('report')
    p.add_argument('--ffprobe', required=True)
    p.add_argument('--coreav', required=True)
    p.add_argument('--mediainfo', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--markdown')
    p.add_argument('--title', default='Three-way stream report: CoreAV, ffprobe, MediaInfo')
    p.add_argument('--private', action='store_true', help='aggregate numbers only, no file names or values')
    p.set_defaults(run=cmd_report)
    args = parser.parse_args(argv)
    return args.run(args)


if __name__ == '__main__':
    sys.exit(main())
