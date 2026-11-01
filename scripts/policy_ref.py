#!/usr/bin/env python3
"""A second implementation of `coreav.policy/1`, written from
docs/schema/policy-v1.md and docs/schema/verdict-v1.md only, used to check
`coreav check` (docs/design/policy-check.md).

- `evaluate(policy, doc)` returns the reasons the reference documents give
  for a `coreav.probe/1` document, or a `coreav.probe-matroska/1` document
  with the rule mapping of docs/design/policy-matroska.md, in the documented order, as dicts with
  the verdict's keys except `message` and `error`. Exact values use
  `fractions.Fraction`; the policy's `max_seconds` is read as the decimal it
  is written as (`decimal.Decimal`), independently of the Rust parser.
- `from_ffprobe(document)` maps `ffprobe -show_format -show_streams` output
  to the keys of a probe document that `ffprobe` can observe, so that the
  same evaluator gives the verdict the black-box oracle implies. Whether
  the file is fragmented comes from `fragmented(path)`, a walk over the box
  headers written from ISO/IEC 14496-12 (section 4.2, 8.8.1, 8.8.4). Rules
  that read a value neither observes (`unobservable`) are left out of that
  comparison. `from_ffprobe_matroska(document, headers, packets)` does the
  same for Matroska and WebM files, with the `DocType`, the live flag and
  the track entries from `matroska_headers(path)`, a walk over the EBML
  elements written from RFC 8794 and RFC 9559, and each stream's span and
  bytes from `ffprobe -show_packets`.

The ffprobe binary is a black box; no implementation
source is read. Uses only the Python standard library.
"""
import collections
import copy
import decimal
import json
from fractions import Fraction

PROBE_SCHEMA = 'coreav.probe/1'
MATROSKA_SCHEMA = 'coreav.probe-matroska/1'
# The codec names `coreav probe` prints for Matroska tracks; `coreav check`
# allows every one (`coreav probe --allow` with this list reads the same).
MATROSKA_CODECS = ('h264', 'hevc', 'av1', 'vp8', 'vp9', 'prores', 'mpeg1video', 'mpeg2video',
                   'mpeg4', 'theora', 'mjpeg', 'ffv1', 'aac', 'mp3', 'mp2', 'opus', 'vorbis',
                   'flac', 'ac3', 'eac3', 'dts', 'truehd', 'alac', 'pcm', 'subrip', 'ass',
                   'webvtt', 'pgs', 'vobsub', 'dvbsub')
KINDS = ('video', 'audio', 'subtitle', 'timecode', 'data')
# Read by no oracle: sample table consistency, external data references
# and the number of sample descriptions.
UNOBSERVABLE = frozenset({'/streams/invalid_tables', '/streams/external_data',
                          '/streams/multiple_descriptions'})
MAX_BOXES = 100_000


def load_policy(text):
    """A policy document whose numbers with a fraction or an exponent are
    exact Decimals of the shortest decimal that names their binary64
    value, as policy-v1.md reads `max_seconds`."""
    return json.loads(text, parse_float=lambda t: decimal.Decimal(repr(float(t))))


FAMILIES = {'h264': 'video', 'hevc': 'video', 'av1': 'video', 'vp9': 'video',
            'prores': 'video', 'aac': 'audio', 'mp3': 'audio', 'opus': 'audio', 'pcm': 'audio',
            'timed_text': 'subtitle', 'timecode': 'timecode'}
FAMILIES.update({c: 'video' for c in ('vp8', 'mpeg1video', 'mpeg2video', 'mpeg4', 'theora',
                                      'mjpeg', 'ffv1')})
FAMILIES.update({c: 'audio' for c in ('mp2', 'vorbis', 'flac', 'ac3', 'eac3', 'dts', 'truehd',
                                      'alac')})
FAMILIES.update({c: 'subtitle' for c in ('subrip', 'ass', 'webvtt', 'pgs', 'vobsub', 'dvbsub')})


def policy_kind(stream):
    """(kind, key read): the codec's family, else the probe's `kind`."""
    family = FAMILIES.get(stream.get('codec')) if isinstance(stream.get('codec'), str) else None
    if family:
        return family, 'codec'
    kind = stream.get('kind')
    return (kind if isinstance(kind, str) else None), 'kind'


def pointer(*keys):
    return ''.join('/' + str(k).replace('~', '~0').replace('/', '~1') for k in keys)


def container_format(brand):
    if brand is None or brand == 'qt  ':
        return 'mov'
    if brand.startswith('3g'):
        return '3gp'
    return 'mp4'


def _int(value):
    return value if isinstance(value, int) and not isinstance(value, bool) and value >= 0 else None


def _seconds(time):
    """(exact seconds, printed seconds) of a probe time, or None."""
    if not isinstance(time, dict):
        return None
    ticks = time.get('ticks')
    if not isinstance(ticks, dict):
        return None
    num, den, scale = ticks.get('num'), _int(ticks.get('den')), _int(time.get('timescale'))
    if not isinstance(num, int) or isinstance(num, bool) or num < 0 or not den or not scale:
        return None
    exact = Fraction(num, den * scale)
    printed = time.get('seconds')
    if not isinstance(printed, (int, float)) or isinstance(printed, bool):
        printed = float(exact)
    return exact, printed


def _bit_rate(rate):
    if not isinstance(rate, dict) or not isinstance(rate.get('exact'), dict):
        return None
    num, den = rate['exact'].get('num'), _int(rate['exact'].get('den'))
    if not isinstance(num, int) or isinstance(num, bool) or num < 0 or not den:
        return None
    exact = Fraction(num, den)
    printed = rate.get('bits_per_second')
    if not isinstance(printed, (int, float)) or isinstance(printed, bool):
        printed = float(exact)
    return exact, printed


def _exact(fraction):
    if fraction is None:
        return None
    if fraction.numerator >= 1 << 64 or fraction.denominator >= 1 << 64:
        return None
    return {'num': fraction.numerator, 'den': fraction.denominator}


def _limit_json(limit):
    """A limit as the verdict prints it: the Rust side writes a policy
    number with a fraction or an exponent as a binary64."""
    return float(limit) if isinstance(limit, decimal.Decimal) else limit


def _get(doc, path):
    node = doc
    for part in path.strip('/').split('/'):
        if isinstance(node, list):
            if not part.isdigit() or int(part) >= len(node):
                return None
            node = node[int(part)]
        elif isinstance(node, dict):
            if part not in node:
                return None
            node = node[part]
        else:
            return None
    return node


def evaluate(policy, doc):
    reasons = []

    def reason(code, rule, path, stream_index=None, value=None, exact=None, limit=None):
        reasons.append({'code': code, 'rule': rule, 'path': path, 'stream_index': stream_index,
                        'value': value, 'exact': _exact(exact), 'limit': limit})

    def maximum(rule, path, stream_index, read, limit):
        """read: (exact, printed) or None; limit: int or Decimal."""
        limit_json = _limit_json(limit)
        if read is None:
            return reason('unknown', rule, path, stream_index, None, None, limit_json)
        exact, printed = read
        if Fraction(exact) > Fraction(limit):
            is_fraction = isinstance(exact, Fraction) and not isinstance(printed, int)
            reason('above_maximum', rule, path, stream_index, printed,
                   exact if is_fraction else None, limit_json)

    if not isinstance(doc, dict) or doc.get('schema') not in (PROBE_SCHEMA, MATROSKA_SCHEMA):
        schema = doc.get('schema') if isinstance(doc, dict) else None
        reason('unknown', None, '/schema', None, schema, None, [PROBE_SCHEMA, MATROSKA_SCHEMA])
        return reasons
    mkv = doc['schema'] == MATROSKA_SCHEMA
    streams = doc.get('streams') if isinstance(doc.get('streams'), list) else []
    dropped = doc.get('dropped_streams') if isinstance(doc.get('dropped_streams'), list) else []
    rules = policy.get('streams', {})
    files = policy.get('file', {})
    size = _int(_get(doc, '/file/size'))
    duration = _seconds(doc.get('duration'))
    # policy-matroska.md: a Matroska file without Duration (`null`) takes
    # the longest stream duration when every stream has one.
    longest = None
    if mkv and 'duration' in doc and doc['duration'] is None and streams:
        reads = [_seconds(s.get('duration')) if isinstance(s, dict) else None for s in streams]
        if all(r is not None for r in reads):
            longest = max(reads, key=lambda r: r[0])
    file_duration = longest if longest is not None else duration

    if 'max_size' in files:
        maximum('/file/max_size', '/file/size', None,
                None if size is None else (size, size), files['max_size'])
    if 'max_bit_rate' in files:
        rate = None
        if size is not None and file_duration is not None and file_duration[0] > 0:
            exact = Fraction(size * 8) / file_duration[0]
            rate = (exact, float(exact))
        limit = files['max_bit_rate']
        if rate is None:
            reason('unknown', '/file/max_bit_rate', None, None, None, None, limit)
        elif rate[0] > limit:
            reason('above_maximum', '/file/max_bit_rate', None, None, rate[1], rate[0], limit)

    container = policy.get('container', {})
    if mkv:
        if 'formats' in container:
            formats = container['formats']
            doc_type = _get(doc, '/container/doc_type')
            if doc_type not in ('webm', 'matroska'):
                reason('unknown', '/container/formats', '/container/doc_type', limit=formats)
            elif doc_type not in formats:
                reason('not_allowed', '/container/formats', '/container/doc_type',
                       value=doc_type, limit=formats)
        if container.get('fragmented', True) is False:
            live = _get(doc, '/container/live')
            if not isinstance(live, bool):
                reason('unknown', '/container/fragmented', '/container/live', limit=False)
            elif live:
                reason('not_allowed', '/container/fragmented', '/container/live',
                       value=True, limit=False)
    elif 'formats' in container:
        formats = container['formats']
        brand = _get(doc, '/container')
        if not isinstance(brand, dict) or 'major_brand' not in brand or not (
                brand['major_brand'] is None or isinstance(brand['major_brand'], str)):
            reason('unknown', '/container/formats', '/container/major_brand', limit=formats)
        else:
            name = container_format(brand['major_brand'])
            if name not in formats:
                reason('not_allowed', '/container/formats', '/container/major_brand',
                       value=name, limit=formats)
    if not mkv and container.get('fragmented', True) is False:
        fragmented = _get(doc, '/container/fragmented')
        if not isinstance(fragmented, bool):
            reason('unknown', '/container/fragmented', '/container/fragmented', limit=False)
        elif fragmented:
            reason('not_allowed', '/container/fragmented', '/container/fragmented',
                   value=True, limit=False)

    max_seconds = policy.get('duration', {}).get('max_seconds')
    if max_seconds is not None:
        candidates = [] if longest is not None else [('/duration', None, duration)]
        for i, s in enumerate(streams):
            index = _int(s.get('stream_index')) if isinstance(s, dict) else None
            read = _seconds(s.get('duration')) if isinstance(s, dict) else None
            candidates.append((pointer('streams', i, 'duration'), index, read))
        chosen = next((c for c in candidates if c[2] is None), None)
        if chosen is None and candidates:
            chosen = candidates[0]
            for c in candidates[1:]:
                if c[2][0] > chosen[2][0]:
                    chosen = c
        if chosen is not None:
            maximum('/duration/max_seconds', chosen[0], chosen[1], chosen[2], max_seconds)

    if 'max_count' in rules:
        count = len(streams) + len(dropped)
        maximum('/streams/max_count', None, None, (count, count), rules['max_count'])
    for kind in KINDS:
        if kind not in rules:
            continue
        count = sum(1 for s in streams if isinstance(s, dict) and policy_kind(s)[0] == kind)
        if 'min_count' in rules[kind] and count < rules[kind]['min_count']:
            reason('below_minimum', pointer('streams', kind, 'min_count'), None, None, count,
                   None, rules[kind]['min_count'])
        if 'max_count' in rules[kind]:
            maximum(pointer('streams', kind, 'max_count'), None, None, (count, count),
                    rules[kind]['max_count'])

    allowed = [k for k in KINDS if k in rules]
    for i, s in enumerate(streams):
        s = s if isinstance(s, dict) else {}
        base = pointer('streams', i)
        index = _int(s.get('stream_index'))
        kind, key = policy_kind(s)
        kind_rules = rules.get(kind) if kind in KINDS else None
        if kind is None:
            reason('unknown', '/streams', base + '/' + key, index, limit=allowed)
        elif kind_rules is None:
            reason('not_allowed', pointer('streams', kind), base + '/' + key, index, kind,
                   limit=allowed)
        if kind_rules is not None:
            rule = lambda key: pointer('streams', kind, key)  # noqa: E731
            if 'codecs' in kind_rules:
                codec = s.get('codec')
                if not isinstance(codec, str):
                    reason('unknown', rule('codecs'), base + '/codec', index,
                           limit=kind_rules['codecs'])
                elif codec not in kind_rules['codecs']:
                    reason('not_allowed', rule('codecs'), base + '/codec', index, codec,
                           limit=kind_rules['codecs'])
            if 'max_bit_rate' in kind_rules:
                maximum(rule('max_bit_rate'), base + '/bit_rate', index,
                        _bit_rate(s.get('bit_rate')), kind_rules['max_bit_rate'])
            video = s.get('video') if isinstance(s.get('video'), dict) else {}
            audio = s.get('audio') if isinstance(s.get('audio'), dict) else {}

            def count_read(obj, key):
                value = _int(obj.get(key))
                return None if value is None else (value, value)

            if kind == 'video':
                # The larger of the declared size and the verified one
                # (docs/design/frame-sizes.md, decision 4).
                source = kind_rules.get('size_source', 'scan')
                key = {'scan': 'scanned_size', 'configuration': 'parameter_set_size'}.get(source)
                verified = video.get(key) if key else None
                verified = verified if isinstance(verified, dict) else {}

                def side(dim):
                    declared = count_read(video, dim)
                    if declared is None:
                        return None, base + '/video/' + dim
                    if key is None:
                        return declared, base + '/video/' + dim
                    value = count_read(verified, dim)
                    path = base + '/video/' + key + '/' + dim
                    if value is None:
                        return None, path
                    if value[0] > declared[0]:
                        return value, path
                    return declared, base + '/video/' + dim

                width, width_path = side('width')
                height, height_path = side('height')
                if 'max_width' in kind_rules:
                    maximum(rule('max_width'), width_path, index, width, kind_rules['max_width'])
                if 'max_height' in kind_rules:
                    maximum(rule('max_height'), height_path, index, height,
                            kind_rules['max_height'])
                if 'max_pixels' in kind_rules:
                    def area(obj):
                        w, h = count_read(obj, 'width'), count_read(obj, 'height')
                        return None if w is None or h is None else w[0] * h[0]
                    pixels = area(video)
                    if key is not None:
                        checked = area(verified)
                        pixels = None if pixels is None or checked is None else max(pixels, checked)
                    maximum(rule('max_pixels'), None, index,
                            None if pixels is None else (pixels, pixels),
                            kind_rules['max_pixels'])
                if 'rotations' in kind_rules:
                    rotations = kind_rules['rotations']
                    if 'rotation' not in video:
                        reason('unknown', rule('rotations'), base + '/video/rotation', index,
                               limit=rotations)
                    elif video['rotation'] is None:
                        reason('not_allowed', rule('rotations'), base + '/video/rotation', index,
                               None, limit=rotations)
                    elif _int(video['rotation']) is None:
                        reason('unknown', rule('rotations'), base + '/video/rotation', index,
                               limit=rotations)
                    elif video['rotation'] not in rotations:
                        reason('not_allowed', rule('rotations'), base + '/video/rotation', index,
                               video['rotation'], limit=rotations)
            if kind == 'audio':
                if 'max_channels' in kind_rules:
                    maximum(rule('max_channels'), base + '/audio/channels', index,
                            count_read(audio, 'channels'), kind_rules['max_channels'])
                if 'max_sample_rate' in kind_rules:
                    maximum(rule('max_sample_rate'), base + '/audio/sample_rate', index,
                            count_read(audio, 'sample_rate'), kind_rules['max_sample_rate'])
        if rules.get('invalid_tables', 'reject') == 'reject':
            key = '/frames_listed' if mkv else '/tables_valid'
            valid = s.get(key[1:])
            if not isinstance(valid, bool):
                reason('unknown', '/streams/invalid_tables', base + key, index, limit=True)
            elif not valid:
                reason('not_allowed', '/streams/invalid_tables', base + key, index, False,
                       limit=True)
        if mkv:
            # No external data and one CodecPrivate per Matroska track.
            continue
        if rules.get('external_data', 'reject') == 'reject':
            external = s.get('external_data')
            if not isinstance(external, bool):
                reason('unknown', '/streams/external_data', base + '/external_data', index,
                       limit=False)
            elif external:
                reason('not_allowed', '/streams/external_data', base + '/external_data', index,
                       True, limit=False)
        if rules.get('multiple_descriptions', 'reject') == 'reject':
            count = _int(s.get('sample_descriptions'))
            if count is None:
                reason('unknown', '/streams/multiple_descriptions', base + '/sample_descriptions',
                       index, limit=1)
            elif count != 1:
                reason('not_allowed', '/streams/multiple_descriptions',
                       base + '/sample_descriptions', index, count, limit=1)
    unidentified = rules.get('unidentified', 'reject')
    if unidentified != 'ignore':
        listed = unidentified if isinstance(unidentified, list) else []
        if mkv:
            # Encrypted frames, or an encoding the probe does not undo: not
            # identified, whatever the list names.
            for i, s in enumerate(streams):
                s = s if isinstance(s, dict) else {}
                if s.get('encoding') in ('none', 'header_stripping'):
                    continue
                reason('not_allowed', '/streams/unidentified', pointer('streams', i, 'encoding'),
                       _int(s.get('stream_index')), s.get('encoding'),
                       limit=['none', 'header_stripping'])
        key = 'codec_id' if mkv else 'fourcc'
        for j, d in enumerate(dropped):
            d = d if isinstance(d, dict) else {}
            if d.get(key) in listed:
                continue
            reason('not_allowed', '/streams/unidentified', pointer('dropped_streams', j, key),
                   _int(d.get('stream_index')), d.get(key), limit=listed)
    return reasons


# ------------------------------------------------------------- ffprobe side

CODECS = {
    'h264': 'h264', 'hevc': 'hevc', 'av1': 'av1', 'vp9': 'vp9', 'prores': 'prores',
    'aac': 'aac', 'mp3': 'mp3', 'opus': 'opus', 'flac': 'flac', 'alac': 'alac', 'ac3': 'ac3',
    'eac3': 'eac3', 'mov_text': 'timed_text',
}
# The sample entry types behind each CoreAV codec name: ffprobe's codec counts only with one of these tags, so that, for
# example, mu-law (`ulaw`, pcm_mulaw) is not linear PCM and QuickTime text
# (`text`, mov_text) is not 3GPP timed text. MP3 counts in `mp4a` and in
# QuickTime's `.mp3`.
TAGS = {
    'h264': {'avc1', 'avc3'}, 'hevc': {'hvc1', 'hev1'}, 'av1': {'av01'}, 'vp9': {'vp09'},
    'prores': {'apco', 'apcs', 'apcn', 'apch', 'ap4h', 'ap4x'}, 'aac': {'mp4a'},
    'mp3': {'mp4a', '.mp3'}, 'opus': {'Opus'}, 'flac': {'fLaC'}, 'alac': {'alac'},
    'ac3': {'ac-3'}, 'eac3': {'ec-3'}, 'timed_text': {'tx3g'}, 'timecode': {'tmcd'},
    'pcm': {'sowt', 'twos', 'in24', 'in32', 'fl32', 'fl64', 'lpcm', 'ipcm', 'fpcm', 'raw '},
}


def _decimal_time(text):
    """A time object for a decimal string of seconds (`10.010000`)."""
    exact = Fraction(decimal.Decimal(text))
    if exact < 0:
        return None
    return {'ticks': {'num': exact.numerator, 'den': exact.denominator}, 'timescale': 1,
            'seconds': float(exact)}


def _headers(f, start, end):
    """(type, payload start, end) of each box between start and end."""
    offset, count = start, 0
    while offset + 8 <= end and count < MAX_BOXES:
        f.seek(offset)
        head = f.read(16)
        if len(head) < 8:
            return
        size, kind = int.from_bytes(head[:4], 'big'), head[4:8]
        header = 8
        if size == 1:
            if len(head) < 16:
                return
            size, header = int.from_bytes(head[8:16], 'big'), 16
        elif size == 0:
            size = end - offset
        if size < header or offset + size > end:
            return
        yield kind, offset + header, offset + size
        offset += size
        count += 1


def _child(f, start, end, kind):
    return next(((s, e) for k, s, e in _headers(f, start, end) if k == kind), None)


def fragmented(path):
    """Whether the file has a movie extends box (`mvex`) in its movie box
    and at least one top-level movie fragment box (`moof`), from the box
    headers alone. None when the headers cannot be walked."""
    try:
        with open(path, 'rb') as f:
            end = f.seek(0, 2)
            mvex = moof = False
            for kind, body, stop in _headers(f, 0, end):
                if kind == b'moov':
                    mvex = mvex or _child(f, body, stop, b'mvex') is not None
                elif kind == b'moof':
                    moof = True
                if mvex and moof:
                    return True
            return False
    except OSError:
        return None


# Sample entry types of protected streams (ISO/IEC 14496-12, 8.12): the
# codec is named only inside the protection scheme information.
PROTECTED = {b'encv', b'enca', b'enct', b'encs', b'encm', b'encf'}


def track_entries(path):
    """(handler type, first sample entry type) of each track by position
    among the file's tracks, from the box headers: moov, trak, mdia, hdlr
    and minf, stbl, stsd (ISO/IEC 14496-12, 8.4.3 and 8.5.2)."""
    found = {}
    try:
        with open(path, 'rb') as f:
            end = f.seek(0, 2)
            moov = _child(f, 0, end, b'moov')
            if moov is None:
                return found
            index = 0
            for kind, body, stop in _headers(f, *moov):
                if kind != b'trak':
                    continue
                handler = entry = None
                mdia = _child(f, body, stop, b'mdia')
                hdlr = _child(f, *mdia, b'hdlr') if mdia else None
                if hdlr:
                    f.seek(hdlr[0] + 8)
                    handler = f.read(4).decode('latin-1')
                box = mdia
                for name in (b'minf', b'stbl', b'stsd'):
                    box = _child(f, *box, name) if box else None
                if box and box[1] - box[0] >= 16:
                    # stsd: version and flags, entry count, then the entries.
                    f.seek(box[0] + 8)
                    head = f.read(8)
                    if len(head) == 8:
                        entry = head[4:8].decode('latin-1')
                found[index] = (handler, entry)
                index += 1
    except OSError:
        pass
    return found



# Timing and bytes of each track, for KD-P-EDITED-BIT-RATE: the media header
# (8.4.2), the edit list (8.6.6), the sample sizes (8.7.3) and the `btrt` box
# of the first sample entry (8.5.2).
MAX_TIMING_TABLE = 64 << 20


def track_timing(path):
    """{track position: {'timescale', 'mdhd', 'edits', 'bytes', 'btrt'}} from
    the box headers: the media timescale and duration, the edit list's
    `(segment_duration, media_time, rate)` entries (the last `elst`, empty
    without one), the sum of the sample sizes (None when the table is
    unreadable) and the `btrt` average bit rate (None without one)."""
    found = {}
    try:
        with open(path, 'rb') as f:
            end = f.seek(0, 2)
            moov = _child(f, 0, end, b'moov')
            if moov is None:
                return found
            index = 0
            for kind, body, stop in _headers(f, *moov):
                if kind != b'trak':
                    continue
                track = {'timescale': None, 'mdhd': None, 'edits': [], 'bytes': None, 'btrt': None}
                found[index] = track
                index += 1
                for edts in [(b, e) for k, b, e in _headers(f, body, stop) if k == b'edts']:
                    for k, b, e in _headers(f, *edts):
                        if k != b'elst' or e - b > MAX_TIMING_TABLE:
                            continue
                        f.seek(b)
                        data = f.read(e - b)
                        version, count = data[0] if data else 0, _u(data, 4, 4) or 0
                        width = 20 if version == 1 else 12
                        if 8 + count * width > len(data):
                            continue
                        track['edits'] = []
                        for at in range(8, 8 + count * width, width):
                            half = (width - 4) // 2
                            track['edits'].append((
                                int.from_bytes(data[at:at + half], 'big'),
                                int.from_bytes(data[at + half:at + 2 * half], 'big', signed=True),
                                int.from_bytes(data[at + width - 4:at + width], 'big', signed=True)))
                mdia = _child(f, body, stop, b'mdia')
                mdhd = _child(f, *mdia, b'mdhd') if mdia else None
                if mdhd:
                    f.seek(mdhd[0])
                    data = f.read(min(mdhd[1] - mdhd[0], 32))
                    if data[:1] == b'\x01':
                        track['timescale'], track['mdhd'] = _u(data, 20, 4), _u(data, 24, 8)
                    else:
                        track['timescale'], track['mdhd'] = _u(data, 12, 4), _u(data, 16, 4)
                stbl = mdia
                for name in (b'minf', b'stbl'):
                    stbl = _child(f, *stbl, name) if stbl else None
                stsz = _child(f, *stbl, b'stsz') if stbl else None
                if stsz and stsz[1] - stsz[0] <= MAX_TIMING_TABLE:
                    f.seek(stsz[0])
                    data = f.read(stsz[1] - stsz[0])
                    size, count = _u(data, 4, 4), _u(data, 8, 4)
                    if size is not None and count is not None:
                        if size:
                            track['bytes'] = size * count
                        elif 12 + 4 * count <= len(data):
                            track['bytes'] = sum(int.from_bytes(data[at:at + 4], 'big')
                                                 for at in range(12, 12 + 4 * count, 4))
                stsd = _child(f, *stbl, b'stsd') if stbl else None
                if stsd and stsd[1] - stsd[0] <= 1 << 20:
                    f.seek(stsd[0])
                    data = f.read(stsd[1] - stsd[0])
                    # A btrt box in the first sample entry: size 20, type,
                    # buffer size, maximum, average.
                    first = _u(data, 8, 4) or 0
                    data = data[8:8 + first]
                    at = data.find(b'btrt')
                    if at >= 4 and _u(data, at - 4, 4) == 20:
                        track['btrt'] = _u(data, at + 12, 4)
    except OSError:
        pass
    return found


def edited_track_duration(track, movie_timescale):
    """The duration CoreAV gives a track with an edit list (docs/design/edit-lists.md decision 5), in media ticks: the media edits'
    durations capped by the media header, for a simple list (one media edit
    after at most one empty edit) or for several edits CoreAV applies
    (rate 1, windows in increasing order without overlap); None otherwise."""
    edits, scale, mdhd = track['edits'], track['timescale'], track['mdhd']
    if not edits or not scale or not mdhd or not movie_timescale:
        return None
    media = [e for e in edits if e[1] != -1]
    if not media or any(e[1] < 0 or e[2] != 0x10000 or e[0] == 0 for e in media):
        return None
    simple = len(edits) == 1 or (len(edits) == 2 and edits[0][1] == -1)
    if not simple:
        previous = None
        for duration, media_time, _ in media:
            start = Fraction(media_time)
            if previous is not None and start < previous:
                return None
            previous = start + Fraction(duration * scale, movie_timescale)
    presented = Fraction(sum(e[0] for e in media) * scale, movie_timescale)
    return min(Fraction(mdhd), presented)


def protected_streams(path, entries=None):
    """Positions among the file's tracks whose first sample entry is a
    protected one (`encv`, `enca`, ...), with that entry type."""
    entries = track_entries(path) if entries is None else entries
    return {i: entry for i, (_, entry) in entries.items()
            if entry and entry.encode('latin-1') in PROTECTED}


def _u(data, offset, size):
    return int.from_bytes(data[offset:offset + size], 'big') if len(data) >= offset + size \
        else None


def movie_headers(path):
    """What the mapping takes from the box headers: the `ftyp` major brand,
    the number of tracks, the movie timescale (`mvhd`), and the positions
    of tracks that another track's `chap` reference names (`tref`,
    ISO/IEC 14496-12 8.3.3; QuickTime chapter lists)."""
    found = {'major_brand': None, 'tracks': None, 'timescale': None, 'chapters': set()}
    try:
        with open(path, 'rb') as f:
            end = f.seek(0, 2)
            ftyp = _child(f, 0, end, b'ftyp')
            if ftyp and ftyp[1] - ftyp[0] >= 4:
                f.seek(ftyp[0])
                found['major_brand'] = f.read(4).decode('latin-1')
            moov = _child(f, 0, end, b'moov')
            if moov is None:
                return found
            mvhd = _child(f, *moov, b'mvhd')
            if mvhd:
                f.seek(mvhd[0])
                m = f.read(24)
                found['timescale'] = _u(m, 20 if m[:1] == b'\x01' else 12, 4)
            ids, targets = [], set()
            for kind, body, stop in _headers(f, *moov):
                if kind != b'trak':
                    continue
                tkhd = _child(f, body, stop, b'tkhd')
                track_id = None
                if tkhd:
                    f.seek(tkhd[0])
                    t = f.read(24)
                    track_id = _u(t, 20 if t[:1] == b'\x01' else 12, 4)
                ids.append(track_id)
                tref = _child(f, body, stop, b'tref')
                chap = _child(f, *tref, b'chap') if tref else None
                if chap:
                    f.seek(chap[0])
                    data = f.read(min(chap[1] - chap[0], 4096))
                    targets.update(_u(data, i, 4) for i in range(0, len(data) - 3, 4))
            found['tracks'] = len(ids)
            found['chapters'] = {i for i, track_id in enumerate(ids) if track_id in targets}
    except OSError:
        pass
    return found


def fragment_duration(path):
    """`fragment_duration` of the movie extends header (`mehd`, ISO/IEC
    14496-12, 8.8.2), or None when there is none."""
    try:
        with open(path, 'rb') as f:
            end = f.seek(0, 2)
            box = _child(f, 0, end, b'moov')
            for name in (b'mvex', b'mehd'):
                box = _child(f, *box, name) if box else None
            if box is None:
                return None
            f.seek(box[0])
            d = f.read(12)
            return _u(d, 4, 8) if d[:1] == b'\x01' else _u(d, 4, 4)
    except OSError:
        return None


def fragment_origins(path, max_fragments=64):
    """Seconds from 0 to the first fragment's decode time (`tfdt`,
    ISO/IEC 14496-12, 8.8.12) of each track, by position among the file's
    tracks; only tracks whose first track fragment has a `tfdt`. Track IDs
    come from `tkhd` and timescales from `mdhd` (8.3.2, 8.4.2)."""
    origins = {}
    try:
        with open(path, 'rb') as f:
            end = f.seek(0, 2)
            moov = _child(f, 0, end, b'moov')
            if moov is None:
                return origins
            tracks = {}
            index = 0
            for kind, body, stop in _headers(f, *moov):
                if kind != b'trak':
                    continue
                tkhd = _child(f, body, stop, b'tkhd')
                mdhd = None
                mdia = _child(f, body, stop, b'mdia')
                if mdia:
                    mdhd = _child(f, *mdia, b'mdhd')
                if tkhd and mdhd:
                    f.seek(tkhd[0])
                    t = f.read(24)
                    f.seek(mdhd[0])
                    m = f.read(24)
                    track_id = _u(t, 20 if t[:1] == b'\x01' else 12, 4)
                    scale = _u(m, 20 if m[:1] == b'\x01' else 12, 4)
                    if track_id is not None and scale:
                        tracks[track_id] = (index, scale)
                index += 1
            seen, fragments = set(), 0
            for kind, body, stop in _headers(f, 0, end):
                if kind != b'moof':
                    continue
                fragments += 1
                for k, tb, te in _headers(f, body, stop):
                    if k != b'traf':
                        continue
                    tfhd, tfdt = _child(f, tb, te, b'tfhd'), _child(f, tb, te, b'tfdt')
                    if not tfhd:
                        continue
                    f.seek(tfhd[0])
                    track_id = _u(f.read(8), 4, 4)
                    if track_id in seen or track_id not in tracks:
                        continue
                    seen.add(track_id)
                    if tfdt:
                        f.seek(tfdt[0])
                        d = f.read(12)
                        time = _u(d, 4, 8) if d[:1] == b'\x01' else _u(d, 4, 4)
                        position, scale = tracks[track_id]
                        if time is not None:
                            origins[position] = Fraction(time, scale)
                if fragments >= max_fragments or len(seen) == len(tracks):
                    break
    except OSError:
        pass
    return origins


def from_ffprobe(document, is_fragmented=None, headers=None, frames=None):
    """The keys of a `coreav.probe/1` document that `ffprobe -show_format
    -show_streams` output gives. Attached pictures are not tracks and are
    left out; a stream whose codec has no CoreAV name is a dropped stream.
    The duration is where the presentation ends: the latest stream end
    (`start_time` + `duration`), or the format duration if that is later,
    since `ffprobe`'s format duration starts at the earliest stream start
    rather than at 0 (unlike probe's `duration`, the declared movie header
    value; probe's `presentation_duration` is `ffprobe`'s format duration). With
    `frames` (stream index to the largest decoded frame size, from
    `ffprobe -show_frames`), the scanned size is the larger of that and the
    stream size, which `ffprobe` takes from the first parameter sets."""
    headers = headers or {}
    fmt = document.get('format', {})
    tags = fmt.get('tags', {})
    # ffprobe leaves the tag out for some files with an `ftyp` box; the box
    # walk's brand stands in only then.
    brand = tags.get('major_brand', headers.get('major_brand'))
    tracks = headers.get('tracks')
    doc = {
        'schema': PROBE_SCHEMA,
        'file': {'size': int(fmt['size']) if 'size' in fmt else None},
        'container': {'major_brand': brand, 'fragmented': is_fragmented},
        'duration': None,
        'streams': [],
        'dropped_streams': [],
    }
    end = Fraction(decimal.Decimal(fmt['duration'])) if 'duration' in fmt else None
    for s in document.get('streams', []):
        index, ctype = s.get('index'), s.get('codec_type')
        # A picture from the metadata (cover art) is not a track; ffprobe
        # lists those after the tracks. A track it marks as an attached
        # picture (chapter images) is still a track.
        is_track = tracks is not None and isinstance(index, int) and index < tracks
        if s.get('disposition', {}).get('attached_pic') == 1 and not is_track:
            continue
        tag = s.get('codec_tag_string')
        name = s.get('codec_name')
        if ctype == 'data' and tag == 'tmcd':
            kind, codec = 'timecode', 'timecode'
        elif name and name.startswith('pcm_'):
            kind, codec = ctype, 'pcm'
        else:
            kind, codec = ctype, CODECS.get(name)
        if codec is not None and tag not in TAGS.get(codec, ()):
            codec = None
        if end is not None and 'duration' in s and 'start_time' in s:
            stream_end = Fraction(decimal.Decimal(s['start_time'])) + \
                Fraction(decimal.Decimal(s['duration']))
            end = max(end, stream_end)
        duration = None
        if 'duration_ts' in s and 'time_base' in s:
            num, den = (int(x) for x in s['time_base'].split('/'))
            if den and s['duration_ts'] >= 0:
                exact = Fraction(s['duration_ts'] * num, den)
                duration = {'ticks': {'num': exact.numerator, 'den': exact.denominator},
                            'timescale': 1, 'seconds': float(exact)}
        if kind not in KINDS or codec is None:
            doc['dropped_streams'].append({'stream_index': index, 'fourcc': tag,
                                           'duration': duration})
            continue
        bit_rate = None
        if 'bit_rate' in s:
            bit_rate = {'exact': {'num': int(s['bit_rate']), 'den': 1},
                        'bits_per_second': float(int(s['bit_rate']))}
        stream = {'stream_index': index, 'kind': kind, 'codec': codec, 'duration': duration,
                  'bit_rate': bit_rate, 'video': None, 'audio': None}
        if kind == 'video':
            rotation = 0
            for side in s.get('side_data_list', []):
                if side.get('side_data_type') == 'Display Matrix' and 'rotation' in side:
                    ccw = side['rotation']
                    clockwise = (-ccw) % 360
                    rotation = clockwise if clockwise % 90 == 0 else None
            # ffprobe's size comes from the decoder: the parameter sets
            # and frame headers, which is what the verified sizes describe.
            size = None
            if isinstance(s.get('width'), int) and isinstance(s.get('height'), int):
                size = {'width': s['width'], 'height': s['height']}
            scanned = size
            if frames and index in frames and size is not None:
                fw, fh = frames[index]
                scanned = {'width': max(size['width'], fw), 'height': max(size['height'], fh)}
            stream['video'] = {'width': s.get('width'), 'height': s.get('height'),
                               'parameter_set_size': size, 'scanned_size': scanned,
                               'rotation': rotation}
        if kind == 'audio':
            stream['audio'] = {
                'channels': s.get('channels'),
                'sample_rate': int(s['sample_rate']) if 'sample_rate' in s else None,
            }
        doc['streams'].append(stream)
    if end is not None and end >= 0:
        doc['duration'] = {'ticks': {'num': end.numerator, 'den': end.denominator},
                           'timescale': 1, 'seconds': float(end)}
    return doc


def unobservable(policy, is_fragmented):
    """Rules the oracle cannot check on this file: those of UNOBSERVABLE,
    `/container/fragmented` when the box walk failed, and stream bit rates
    of fragmented files, for which `ffprobe` counts only part of the
    samples (docs/schema/probe-v1.md)."""
    rules = set(UNOBSERVABLE)
    if is_fragmented is None:
        rules.add('/container/fragmented')
    if is_fragmented is not False:
        rules.update(pointer('streams', kind, 'max_bit_rate') for kind in KINDS)
    return rules


def observable(policy, excluded):
    """The policy without the rules in `excluded`."""
    policy = copy.deepcopy(policy)
    if '/container/fragmented' in excluded:
        policy.get('container', {}).pop('fragmented', None)
    rules = policy.setdefault('streams', {})
    for rule in ('invalid_tables', 'external_data', 'multiple_descriptions'):
        if pointer('streams', rule) in excluded:
            rules[rule] = 'ignore'
    for kind in KINDS:
        if pointer('streams', kind, 'max_bit_rate') in excluded and kind in rules:
            rules[kind].pop('max_bit_rate', None)
    return policy


def outcome(reasons, excluded):
    """What the oracle comparison compares: the failed rules outside
    `excluded` with their stream, except that the duration rule is one per
    file whichever stream is longest."""
    keys = []
    for r in reasons:
        if r['rule'] in excluded:
            continue
        stream = None if r['rule'] == '/duration/max_seconds' else r['stream_index']
        keys.append((r['code'], r['rule'], stream))
    return sorted(keys, key=lambda k: (k[0], k[1] or '', -1 if k[2] is None else k[2]))


# ------------------------------------------------- Matroska, ffprobe side

MAX_ELEMENTS = 100_000
EBML_ID, SEGMENT_ID, CLUSTER_ID, TRACKS_ID = 0x1A45DFA3, 0x18538067, 0x1F43B675, 0x1654AE6B
TAGS_ID = 0x1254C367
# The integer elements of a video track's `Colour` (RFC 9559 5.1.4.1.28.16).
COLOUR = {0x55B1: 'MatrixCoefficients', 0x55B2: 'BitsPerChannel', 0x55B7: 'ChromaSitingHorz',
          0x55B8: 'ChromaSitingVert', 0x55B9: 'Range', 0x55BA: 'TransferCharacteristics', 0x55BB: 'Primaries'}
# ffprobe `codec_name` to the name `coreav probe` prints for Matroska.
MATROSKA_FFPROBE = {name: name for name in MATROSKA_CODECS}
MATROSKA_FFPROBE.update({'hdmv_pgs_subtitle': 'pgs', 'dvd_subtitle': 'vobsub',
                         'dvb_subtitle': 'dvbsub', 'ssa': 'ass', 'text': 'subrip'})


def _vint(data, at, is_id):
    """(value, length) of an EBML variable-size integer (RFC 8794 4); IDs
    keep their marker bit. The value is None for an all-ones size
    (unknown)."""
    if at >= len(data) or data[at] == 0:
        return None, 0
    first = data[at]
    length = 8 - first.bit_length() + 1
    if at + length > len(data):
        return None, 0
    raw = int.from_bytes(data[at:at + length], 'big')
    if is_id:
        return raw, length
    value = raw & ((1 << (7 * length)) - 1)
    return (None if value == (1 << (7 * length)) - 1 else value), length


def _elements(f, start, end):
    """(id, data start, data end or None for an unknown size) of each
    element between start and end."""
    at, count = start, 0
    while at < end and count < MAX_ELEMENTS:
        f.seek(at)
        head = f.read(12)
        eid, n = _vint(head, 0, True)
        if not n:
            return
        size, m = _vint(head, n, False)
        if not m:
            return
        data = at + n + m
        stop = None if size is None else data + size
        yield eid, data, stop
        if stop is None:
            return
        at, count = stop, count + 1


def _fits(data, size, end):
    return data + size <= end


def _payload(f, body, stop, end, limit):
    """The payload of an element from `body` to `stop`, read whole into
    memory when it ends within its parent's `end` (itself within the file)
    and holds at most `limit` octets; ValueError otherwise, so that a
    `_children` walk sees the whole of it."""
    if not _fits(body, stop - body, end):
        raise ValueError('an element past its parent')
    if stop - body > limit:
        raise ValueError('an element larger than the read takes')
    f.seek(body)
    return f.read(stop - body)


def _children(data):
    """(id, payload) of each element in a master element held in memory,
    all of them or ValueError: a header cut short, a child of unknown size,
    or one past its parent."""
    at = 0
    while at < len(data):
        eid, n = _vint(data, at, True)
        size, m = _vint(data, at + n, False) if n else (None, 0)
        if not n or not m:
            raise ValueError('a child header cut short')
        if size is None:
            raise ValueError('a child of unknown size')
        if not _fits(at + n + m, size, len(data)):
            raise ValueError('a child past its parent')
        yield eid, data[at + n + m:at + n + m + size]
        at += n + m + size


def _string(payload):
    """An EBML string: ASCII, possibly padded with zero octets (RFC 8794
    7.4), which are not part of the value."""
    return payload.split(b'\x00', 1)[0].decode('ascii', 'replace')


def _uint(payload):
    return int.from_bytes(payload, 'big') if payload else 0


def _encoding(entry):
    """`none`, `header_stripping`, `encrypted` or `unsupported`, from the
    track's ContentEncodings (RFC 9559 5.1.4.1.31)."""
    encodings = [p for i, p in _children(entry) if i == 0x6D80]
    if not encodings:
        return 'none'
    found = [p for i, p in _children(encodings[0]) if i == 0x6240]
    if len(found) != 1:
        return 'unsupported' if found else 'none'
    kind, algo, scope = 0, None, 1
    for i, p in _children(found[0]):
        if i == 0x5033:
            kind = _uint(p)
        elif i == 0x5032:
            scope = _uint(p)
        elif i == 0x5034:
            algo = 0
            for j, q in _children(p):
                if j == 0x4254:
                    algo = _uint(q)
    if kind == 1:
        return 'encrypted'
    if kind == 0 and algo == 3 and scope == 1:
        return 'header_stripping'
    return 'unsupported'


def matroska_headers(path, tags=False):
    """What the Matroska mapping takes from the EBML elements: the
    `DocType`, whether the Segment's size is unknown (live), and for each
    `TrackEntry` in order its `TrackNumber`, `TrackUID`, `TrackType`,
    `CodecID`, `DefaultDuration`, encoding, `PixelWidth` and `PixelHeight`
    (`pixels`), its stored `Language` and `LanguageBCP47` (`language`,
    `language_bcp47`), its `CodecPrivate` bytes (`codec_private`) and the
    integer elements of its `Colour` by name (`colour`, RFC 9559
    5.1.4.1.28.16), each None when absent.
    With `tags`, also each track's `tag_names`: the `TagName` of every
    `SimpleTag` in a `Tag` whose `Targets` name the track's `TrackUID`
    (RFC 9559 5.1.8), read from the `Tags` before any cluster of unknown
    size. None when the file does not start with an EBML header, or when
    an element read in memory (the EBML header, `Tracks`, `Tags`) ends past
    its parent or the file, is over its read limit, or holds a child that
    does not fit it."""
    found = {'doc_type': None, 'live': None, 'tracks': []}
    tag_names = {}
    try:
        with open(path, 'rb') as f:
            end = f.seek(0, 2)
            top = list(_elements(f, 0, end))
            if not top or top[0][0] != EBML_ID:
                return None
            _, data, stop = top[0]
            for i, p in _children(_payload(f, data, stop or data, end, 1 << 16)):
                if i == 0x4282:
                    found['doc_type'] = _string(p)
            segment = next((e for e in top if e[0] == SEGMENT_ID), None)
            if segment is None:
                return found
            found['live'] = segment[2] is None
            seg_end = min(segment[2] or end, end)
            for eid, body, stop in _elements(f, segment[1], seg_end):
                if eid == TRACKS_ID and stop is not None and not found['tracks']:
                    for i, entry in _children(_payload(f, body, stop, seg_end, 16 << 20)):
                        if i != 0xAE:
                            continue
                        track = {'number': None, 'uid': None, 'type': None, 'codec_id': None,
                                 'default_duration': None, 'pixels': None, 'language': None,
                                 'language_bcp47': None, 'codec_private': None, 'colour': None,
                                 'encoding': _encoding(entry)}
                        for j, q in _children(entry):
                            if j == 0xD7:
                                track['number'] = _uint(q)
                            elif j == 0x73C5:
                                track['uid'] = _uint(q)
                            elif j == 0x22B59C:
                                track['language'] = _string(q)
                            elif j == 0x22B59D:
                                track['language_bcp47'] = _string(q)
                            elif j == 0x83:
                                track['type'] = _uint(q)
                            elif j == 0x86:
                                track['codec_id'] = _string(q)
                            elif j == 0x23E383:
                                track['default_duration'] = _uint(q)
                            elif j == 0x63A2:
                                track['codec_private'] = bytes(q)
                            elif j == 0xE0:
                                video = dict(_children(q))
                                if 0xB0 in video and 0xBA in video:
                                    track['pixels'] = (_uint(video[0xB0]), _uint(video[0xBA]))
                                if 0x55B0 in video:
                                    track['colour'] = {COLOUR[k]: _uint(v) for k, v in _children(video[0x55B0])
                                                       if k in COLOUR}
                        found['tracks'].append(track)
                    if not tags:
                        break
                elif eid == TAGS_ID and tags and stop is not None:
                    for i, tag in _children(_payload(f, body, stop, seg_end, 16 << 20)):
                        if i != 0x7373:
                            continue
                        targets = [u for j, q in _children(tag) if j == 0x63C0
                                   for k, u in _children(q) if k == 0x63C5]
                        names = [_string(n) for j, q in _children(tag) if j == 0x67C8
                                 for k, n in _children(q) if k == 0x45A3]
                        for uid in targets:
                            tag_names.setdefault(_uint(uid), set()).update(names)
                if eid == CLUSTER_ID and stop is None:
                    break
    except (OSError, ValueError):
        return None
    if tags:
        for track in found['tracks']:
            track['tag_names'] = sorted(tag_names.get(track['uid'], set())) if track['uid'] else []
    return found


# The element kinds of the Matroska EBML Schema (RFC 9559 section 5, from
# ebml_matroska.xml): master elements, integers (at most 8 octets), floats
# (0, 4 or 8), dates (0 or 8), binaries of a fixed length, strings and other
# binaries (RFC 8794 7); CRC-32 (RFC 8794 11.3.2) is 4 octets.
MATROSKA_KINDS = {
    0x80: 'master', 0x83: 'int', 0x85: 'text', 0x86: 'text', 0x88: 'int', 0x89: 'int',
    0x8E: 'master', 0x8F: 'master', 0x91: 'int', 0x92: 'int', 0x96: 'int', 0x97: 'int',
    0x98: 'int', 0x9A: 'int', 0x9B: 'int', 0x9C: 'int', 0x9D: 'int', 0x9F: 'int',
    0xA0: 'master', 0xA1: 'binary', 0xA2: 'binary', 0xA3: 'binary', 0xA4: 'binary',
    0xA5: 'binary', 0xA6: 'master', 0xA7: 'int', 0xAA: 'int', 0xAB: 'int', 0xAE: 'master',
    0xAF: 'binary', 0xB0: 'int', 0xB2: 'int', 0xB3: 'int', 0xB5: 'float', 0xB6: 'master',
    0xB7: 'master', 0xB9: 'int', 0xBA: 'int', 0xBB: 'master', 0xBF: 4, 0xC0: 'int', 0xC1: 16,
    0xC4: 16, 0xC6: 'int', 0xC7: 'int', 0xC8: 'master', 0xC9: 'int', 0xCA: 'int', 0xCB: 'int',
    0xCC: 'int', 0xCD: 'int', 0xCE: 'int', 0xCF: 'int', 0xD7: 'int', 0xDB: 'master',
    0xE0: 'master', 0xE1: 'master', 0xE2: 'master', 0xE3: 'master', 0xE4: 'master',
    0xE5: 'int', 0xE6: 'int', 0xE7: 'int', 0xE8: 'master', 0xE9: 'master', 0xEA: 'int',
    0xEB: 'int', 0xED: 'int', 0xEE: 'int', 0xF0: 'int', 0xF1: 'int', 0xF7: 'int', 0xFA: 'int',
    0xFB: 'int', 0xFD: 'int', 0x41A4: 'text', 0x41E4: 'master', 0x41E7: 'int',
    0x41ED: 'binary', 0x41F0: 'int', 0x4254: 'int', 0x4255: 'binary', 0x42F2: 'int',
    0x42F3: 'int', 0x437C: 'text', 0x437D: 'text', 0x437E: 'text', 0x4444: 16, 0x4461: 'date',
    0x447A: 'text', 0x447B: 'text', 0x4484: 'int', 0x4485: 'binary', 0x4487: 'text',
    0x4489: 'float', 0x44B4: 'int', 0x450D: 'binary', 0x4520: 'master', 0x4521: 'text',
    0x4588: 'int', 0x4598: 'int', 0x45A3: 'text', 0x45B9: 'master', 0x45BC: 'int',
    0x45BD: 'int', 0x45DB: 'int', 0x45DD: 'int', 0x45E4: 'text', 0x465C: 'binary',
    0x4660: 'text', 0x4661: 'int', 0x4662: 'int', 0x466E: 'text', 0x4675: 'binary',
    0x467E: 'text', 0x46AE: 'int', 0x47E1: 'int', 0x47E2: 'binary', 0x47E3: 'binary',
    0x47E4: 'binary', 0x47E5: 'int', 0x47E6: 'int', 0x47E7: 'master', 0x47E8: 'int',
    0x4D80: 'text', 0x4DBB: 'master', 0x5031: 'int', 0x5032: 'int', 0x5033: 'int',
    0x5034: 'master', 0x5035: 'master', 0x52F1: 'int', 0x535F: 'int', 0x536E: 'text',
    0x5378: 'int', 0x537F: 'int', 0x53AB: 4, 0x53AC: 'int', 0x53B8: 'int', 0x53B9: 'int',
    0x53C0: 'int', 0x54AA: 'int', 0x54B0: 'int', 0x54B2: 'int', 0x54B3: 'int', 0x54BA: 'int',
    0x54BB: 'int', 0x54CC: 'int', 0x54DD: 'int', 0x55AA: 'int', 0x55AB: 'int', 0x55AC: 'int',
    0x55AD: 'int', 0x55AE: 'int', 0x55AF: 'int', 0x55B0: 'master', 0x55B1: 'int',
    0x55B2: 'int', 0x55B3: 'int', 0x55B4: 'int', 0x55B5: 'int', 0x55B6: 'int', 0x55B7: 'int',
    0x55B8: 'int', 0x55B9: 'int', 0x55BA: 'int', 0x55BB: 'int', 0x55BC: 'int', 0x55BD: 'int',
    0x55D0: 'master', 0x55D1: 'float', 0x55D2: 'float', 0x55D3: 'float', 0x55D4: 'float',
    0x55D5: 'float', 0x55D6: 'float', 0x55D7: 'float', 0x55D8: 'float', 0x55D9: 'float',
    0x55DA: 'float', 0x55EE: 'int', 0x5654: 'text', 0x56AA: 'int', 0x56BB: 'int',
    0x5741: 'text', 0x5854: 'master', 0x58D7: 'int', 0x61A7: 'master', 0x6240: 'master',
    0x6264: 'int', 0x63A2: 'binary', 0x63C0: 'master', 0x63C3: 'int', 0x63C4: 'int',
    0x63C5: 'int', 0x63C6: 'int', 0x63C7: 'int', 0x63C9: 'int', 0x63CA: 'text',
    0x6624: 'master', 0x66A5: 'binary', 0x66BF: 'int', 0x66FC: 'int', 0x67C8: 'master',
    0x68CA: 'int', 0x6911: 'master', 0x6922: 'int', 0x6924: 'master', 0x6933: 'binary',
    0x6944: 'master', 0x6955: 'int', 0x69A5: 'binary', 0x69BF: 'int', 0x69FC: 'int',
    0x6D80: 'master', 0x6DE7: 'int', 0x6DF8: 'int', 0x6E67: 16, 0x6EBC: 'int', 0x6FAB: 'int',
    0x7373: 'master', 0x7384: 'text', 0x73A4: 16, 0x73C4: 'int', 0x73C5: 'int', 0x7446: 'int',
    0x75A1: 'master', 0x75A2: 'int', 0x7670: 'master', 0x7671: 'int', 0x7672: 'binary',
    0x7673: 'float', 0x7674: 'float', 0x7675: 'float', 0x78B5: 'float', 0x7BA9: 'text',
    0x7D7B: 'binary', 0x22B59C: 'text', 0x22B59D: 'text', 0x23314F: 'float', 0x234E7A: 'int',
    0x2383E3: 'float', 0x23E383: 'int', 0x258688: 'text', 0x26B240: 'text', 0x2AD7B1: 'int',
    0x2EB524: 4, 0x2FB523: 'float', 0x3A9697: 'text', 0x3B4040: 'text', 0x3C83AB: 'text',
    0x3CB923: 16, 0x3E83BB: 'text', 0x3EB923: 16, 0x1043A770: 'master', 0x114D9B74: 'master',
    0x1254C367: 'master', 0x1549A966: 'master', 0x1654AE6B: 'master', 0x18538067: 'master',
    0x1941A469: 'master', 0x1C53BB6B: 'master', 0x1F43B675: 'master',
}
# Sizes the audit allows for strings and binaries (docs/design/
# policy-matroska.md, decision 8), with coreav-mkv's default limits:
# max_element_bytes, an attached file, max_packet_bytes for frames.
MAX_ELEMENT = 1 << 20
MAX_ATTACHED_FILE = 64 << 20
MAX_FRAME = 128 << 20


def _length_allowed(eid, length):
    kind = MATROSKA_KINDS.get(eid)
    if kind in ('text', 'binary'):
        if eid == 0x465C:
            return length <= MAX_ATTACHED_FILE
        if eid in (0xA5, 0xA3, 0xA1):
            return length <= MAX_FRAME
        return length <= MAX_ELEMENT
    if kind == 'int':
        return length <= 8
    if kind == 'float':
        return length in (0, 4, 8)
    if kind == 'date':
        return length in (0, 8)
    if isinstance(kind, int):
        return length == kind
    return True


TOP_LEVEL = {0x114D9B74, 0x1549A966, CLUSTER_ID, TRACKS_ID, 0x1C53BB6B, 0x1941A469, 0x1043A770,
             0x1254C367}


def matroska_audit(path):
    """A reason the file's elements hide frames from a block listing that
    stops where the structure is unclear, or None (docs/design/
    policy-matroska.md, decision 8): bytes after the Segment's declared
    end, an EBML header or an unknown element among the Segment's
    children, an element running past its parent or the file, a second
    Info or Tracks, a BlockGroup with two Blocks, a block of a track no
    TrackEntry defines, an element whose length its schema type does not
    allow, anywhere below the Segment but in block payloads. Written from
    RFC 8794 and RFC 9559."""
    try:
        with open(path, 'rb') as f:
            size = f.seek(0, 2)

            def header(at, limit):
                f.seek(at)
                head = f.read(min(12, max(0, limit - at)))
                eid, n = _vint(head, 0, True)
                if not n:
                    return None
                length, m = _vint(head, n, False)
                if not m:
                    return None
                return eid, at + n + m, (None if length is None else at + n + m + length)

            def nested(body, end, depth):
                """The first problem among the elements from body to end."""
                at = body
                while at < end:
                    if depth > 12:
                        return 'elements nested too deep'
                    h = header(at, end)
                    if h is None or h[2] is None or h[2] > end:
                        return 'element past its parent or the file'
                    eid, cbody, cend = h
                    problem = checked(eid, cbody, cend, depth)
                    if problem:
                        return problem
                    at = cend
                return None

            def checked(eid, body, end, depth):
                """The problem with one element's length or children."""
                if not _length_allowed(eid, end - body):
                    return 'element length its type does not allow'
                if MATROSKA_KINDS.get(eid) == 'master':
                    return nested(body, end, depth + 1)
                return None

            first = header(0, size)
            if first is None or first[0] != EBML_ID or first[2] is None:
                return 'no EBML header'
            seg = header(first[2], size)
            if seg is None or seg[0] != SEGMENT_ID:
                return 'no Segment'
            numbers = {t['number'] for t in (matroska_headers(path) or {}).get('tracks', [])}
            declared = seg[2]
            limit = min(declared, size) if declared is not None else size
            at, seen, elements = seg[1], collections.Counter(), 0
            while at < max(size, declared or 0):
                elements += 1
                if elements > 10_000_000:
                    return 'too many elements'
                if declared is not None and at >= declared:
                    return 'data after the Segment'
                h = header(at, size)
                if h is None:
                    return 'truncated element header'
                eid, body, end = h
                if eid == EBML_ID:
                    return 'EBML header inside the Segment'
                if eid not in TOP_LEVEL | {0xEC, 0xBF}:
                    return 'unknown element in the Segment'
                if eid in (0x1549A966, TRACKS_ID):
                    seen[eid] += 1
                    if seen[eid] > 1:
                        return 'second Info or Tracks'
                if eid != CLUSTER_ID:
                    if end is None or end > limit:
                        return 'element past its parent or the file'
                    problem = checked(eid, body, end, 0)
                    if problem:
                        return problem
                    at = end
                    continue
                if end is not None and end > limit:
                    return 'Cluster past the Segment or the file'
                stop = end if end is not None else limit
                inner = body
                while inner < stop:
                    elements += 1
                    c = header(inner, stop)
                    if c is None:
                        return 'truncated element header'
                    cid, cbody, cend = c
                    if end is None and (cid == EBML_ID or cid in TOP_LEVEL):
                        break
                    if cend is None or cend > stop:
                        return 'element past its parent or the file'
                    blocks = []
                    if cid == 0xA3:
                        if not _length_allowed(cid, cend - cbody):
                            return 'string or binary longer than allowed'
                        blocks = [(cbody, cend)]
                    elif cid != 0xA0:
                        problem = checked(cid, cbody, cend, 1)
                        if problem:
                            return problem
                    else:
                        g = cbody
                        while g < cend:
                            gh = header(g, cend)
                            if gh is None or gh[2] is None or gh[2] > cend:
                                return 'element past its parent or the file'
                            if gh[0] == 0xA1:
                                if not _length_allowed(gh[0], gh[2] - gh[1]):
                                    return 'string or binary longer than allowed'
                                blocks.append((gh[1], gh[2]))
                            else:
                                problem = checked(gh[0], gh[1], gh[2], 2)
                                if problem:
                                    return problem
                            g = gh[2]
                        if len(blocks) > 1:
                            return 'BlockGroup with two Blocks'
                    for bbody, bend in blocks:
                        f.seek(bbody)
                        number, n = _vint(f.read(min(8, bend - bbody)), 0, False)
                        if not n or number not in numbers:
                            return 'block of an undefined track'
                    inner = cend
                at = inner if end is None else end
            return None
    except OSError:
        return 'unreadable'


def laced_tracks(path):
    """The TrackNumbers that have a laced SimpleBlock or Block (lacing bits
    of the block header's flags, RFC 9559 10.2), from the element headers
    of every Cluster."""
    found = set()
    try:
        with open(path, 'rb') as f:
            end = f.seek(0, 2)
            top = list(_elements(f, 0, end))
            segment = next((e for e in top if e[0] == SEGMENT_ID), None)
            if segment is None:
                return found
            seg_end = min(segment[2] or end, end)
            for eid, body, stop in _elements(f, segment[1], seg_end):
                if eid != CLUSTER_ID:
                    continue
                for cid, cbody, cstop in _elements(f, body, stop or seg_end):
                    blocks = [(cid, cbody)]
                    if cid == 0xA0 and cstop is not None:
                        blocks = [(g, gb) for g, gb, _ in _elements(f, cbody, cstop) if g == 0xA1]
                    for bid, bbody in blocks:
                        if bid not in (0xA3, 0xA1):
                            continue
                        f.seek(bbody)
                        head = f.read(12)
                        number, n = _vint(head, 0, False)
                        if n and len(head) > n + 2 and (head[n + 2] >> 1) & 3:
                            found.add(number)
                    if cstop is None:
                        break
    except OSError:
        pass
    return found



# The largest block of the track asked for that matroska_frames reads.
MAX_BLOCK = 1 << 24


def _complete(f, start, end, unknown=False):
    """(id, data start, data end) of each element between start and end, the
    whole range or ValueError: a header cut short, an element past its
    parent, more than MAX_ELEMENTS, or one of unknown size, which only with
    `unknown` ends the range."""
    at, count = start, 0
    while at < end:
        count += 1
        if count > MAX_ELEMENTS:
            raise ValueError('more elements than the walk reads')
        f.seek(at)
        head = f.read(min(12, end - at))
        eid, n = _vint(head, 0, True)
        size, m = _vint(head, n, False) if n else (None, 0)
        if not n or not m:
            raise ValueError('an element header cut short')
        data = at + n + m
        if size is None:
            if not unknown:
                raise ValueError('an element of unknown size')
            yield eid, data, end
            return
        if not _fits(data, size, end):
            raise ValueError('an element past its parent')
        yield eid, data, data + size
        at = data + size


def matroska_block_frames(path, number):
    """(block data offset, frames) for each block of the track numbered
    `number`, in file order. The offset is the file position of the
    TrackNumber vint in the SimpleBlock or Block; frames are its data,
    a lace split into its frames (RFC 9559 10.3 to 10.6). Every element of
    the Segment is walked, each within its parent and the file, so that no
    frame is left unread;
    only the Segment may be of unknown size. Raises ValueError for a walk
    that cannot reach every block, a block or lace that does not fit its
    element, or a block of the track over MAX_BLOCK, OSError for an
    unreadable file."""
    with open(path, 'rb') as f:
        end = f.seek(0, 2)
        segment = next((e for e in _complete(f, 0, end, unknown=True) if e[0] == SEGMENT_ID), None)
        if segment is None:
            return
        for eid, body, stop in _complete(f, segment[1], segment[2]):
            if eid != CLUSTER_ID:
                continue
            for cid, cbody, cstop in _complete(f, body, stop):
                blocks = [(cid, cbody, cstop)]
                if cid == 0xA0:
                    blocks = [b for b in _complete(f, cbody, cstop) if b[0] == 0xA1]
                for bid, bbody, bstop in blocks:
                    if bid not in (0xA3, 0xA1):
                        continue
                    f.seek(bbody)
                    head = f.read(min(12, bstop - bbody))
                    track, n = _vint(head, 0, False)
                    if not n or len(head) < n + 3:
                        raise ValueError('a block header cut short')
                    if track != number:
                        continue
                    if bstop - bbody > MAX_BLOCK:
                        raise ValueError('a block larger than the walk reads')
                    f.seek(bbody)
                    block = f.read(bstop - bbody)
                    if len(block) != bstop - bbody:
                        raise ValueError('a block cut short')
                    yield bbody, _laced(block[n + 3:], block[n + 2] >> 1 & 3)


def matroska_frames(path, number):
    """The frames of the track numbered `number`, in file order, from the
    same complete, bounded walk as `matroska_block_frames`."""
    for _, frames in matroska_block_frames(path, number):
        yield from frames


def _laced(data, lacing):
    """The frames of a block's data under its lacing bits (RFC 9559 10.3:
    none, Xiph, fixed-size or EBML)."""
    if not lacing:
        return [data]
    if not data:
        raise ValueError('a lace without its frame count')
    count, at, sizes = data[0] + 1, 1, []
    if lacing == 1:
        for _ in range(count - 1):
            size = 0
            while True:
                if at >= len(data):
                    raise ValueError('Xiph lace sizes cut short')
                size, at = size + data[at], at + 1
                if data[at - 1] != 255:
                    break
            sizes.append(size)
    elif lacing == 3:
        for i in range(count - 1):
            value, n = _vint(data, at, False)
            if not n or value is None:
                raise ValueError('an EBML lace size that cannot be read')
            at += n
            sizes.append(value if i == 0 else sizes[-1] + value - ((1 << (7 * n - 1)) - 1))
    elif (len(data) - at) % count:
        raise ValueError('a fixed-size lace that does not divide its data')
    else:
        sizes = [(len(data) - at) // count] * (count - 1)
    last = len(data) - at - sum(sizes)
    if any(size < 0 for size in sizes) or last < 0:
        raise ValueError('lace sizes past the block')
    frames = []
    for size in sizes + [last]:
        frames.append(data[at:at + size])
        at += size
    return frames

def packet_spans(rows, time_bases):
    """Per stream index: (span in seconds, bytes, span of the starts) from
    ffprobe packet rows (stream_index, pts, duration, size), the span
    running from the earliest pts to the latest pts + duration, the span of
    the starts to the latest pts; both are None when a packet has no
    pts."""
    spans = {}
    for index, pts, duration, size in rows:
        base = time_bases.get(index)
        if base is None:
            continue
        start, end, last, total, timed = spans.get(index, (None, None, None, 0, True))
        total += size if isinstance(size, int) else 0
        if pts is None:
            timed = False
        else:
            t0 = Fraction(pts) * base
            t1 = t0 + Fraction(duration or 0) * base
            start = t0 if start is None else min(start, t0)
            end = t1 if end is None else max(end, t1)
            last = t0 if last is None else max(last, t0)
        spans[index] = (start, end, last, total, timed)
    out = {}
    for index, (start, end, last, total, timed) in spans.items():
        known = timed and start is not None
        out[index] = ((end - start) if known else None, total, (last - start) if known else None)
    return out


def _time(exact):
    return {'ticks': {'num': exact.numerator, 'den': exact.denominator}, 'timescale': 1,
            'seconds': float(exact)}


def from_ffprobe_matroska(document, headers, spans=None, frames=None):
    """The keys of a `coreav.probe-matroska/1` document that `ffprobe
    -show_format -show_streams` output, the EBML walk (`headers`) and the
    packet listing (`spans`, from `packet_spans`) give. Attachments are
    files, not tracks; a track whose codec has no CoreAV name is a dropped
    stream with its `CodecID`. Each stream's duration is its packets' span
    and its bit rate their bytes over it; the file duration is ffprobe's
    format duration (`Duration`), `null` when it has none."""
    spans = spans or {}
    fmt = document.get('format', {})
    tracks = headers.get('tracks') or []
    doc = {
        'schema': MATROSKA_SCHEMA,
        'file': {'size': int(fmt['size']) if 'size' in fmt else None},
        'container': {'doc_type': headers.get('doc_type'), 'live': headers.get('live')},
        'duration': _decimal_time(fmt['duration']) if 'duration' in fmt else None,
        'streams': [],
        'dropped_streams': [],
    }
    position = 0
    for s in document.get('streams', []):
        index, ctype = s.get('index'), s.get('codec_type')
        # Attachments come after the tracks; ffprobe lists image ones (cover
        # art) as video streams.
        if ctype == 'attachment' or position >= len(tracks):
            continue
        track = tracks[position]
        position += 1
        name = s.get('codec_name')
        codec = 'pcm' if name and name.startswith('pcm_') else MATROSKA_FFPROBE.get(name)
        if codec is None:
            doc['dropped_streams'].append({'stream_index': index,
                                           'codec_id': track.get('codec_id')})
            continue
        # A stream ffprobe lists no packet for has no frames: a span of 0.
        span, total, _ = spans.get(index, (Fraction(0), 0, Fraction(0)))
        duration = bit_rate = None
        if span is not None:
            duration = _time(span)
            if span > 0:
                rate = Fraction(total * 8) / span
                bit_rate = {'exact': {'num': rate.numerator, 'den': rate.denominator},
                            'bits_per_second': float(rate)}
        encoding = track.get('encoding', 'none')
        stream = {'stream_index': index, 'kind': ctype, 'codec': codec,
                  'codec_id': track.get('codec_id'), 'encoding': encoding,
                  'packets_available': encoding != 'unsupported',
                  'duration': duration, 'bit_rate': bit_rate, 'video': None, 'audio': None}
        if ctype == 'video':
            rotation = 0
            for side in s.get('side_data_list', []):
                if side.get('side_data_type') == 'Display Matrix' and 'rotation' in side:
                    clockwise = (-side['rotation']) % 360
                    rotation = clockwise if clockwise % 90 == 0 else None
            size = None
            if isinstance(s.get('width'), int) and isinstance(s.get('height'), int):
                size = {'width': s['width'], 'height': s['height']}
            scanned = size
            if frames and index in frames and size is not None:
                fw, fh = frames[index]
                scanned = {'width': max(size['width'], fw), 'height': max(size['height'], fh)}
            stream['video'] = {'width': s.get('width'), 'height': s.get('height'),
                               'parameter_set_size': size, 'scanned_size': scanned,
                               'rotation': rotation}
        if ctype == 'audio':
            stream['audio'] = {
                'channels': s.get('channels'),
                'sample_rate': int(s['sample_rate']) if 'sample_rate' in s else None,
            }
        doc['streams'].append(stream)
    return doc


# ------------------------------------------------------ known divergences
#
# docs/known-divergences-policy.md. Each entry rechecks its condition from
# the file's box headers or the oracle's own output and, when it holds,
# adjusts the ffprobe-derived document the way the entry describes. A
# disagreement counts as covered only when the adjusted document gives the
# same outcome; covered cases are reported apart from direct agreement.

def _kd_protected(mapped, evidence):
    """KD-P-PROTECTED: streams whose first sample entry is a protected one
    are unidentified for CoreAV; ffprobe names the original format."""
    protected = evidence.get('protected') or {}
    moved = [s for s in mapped['streams'] if s['stream_index'] in protected]
    if not moved:
        return None
    adjusted = copy.deepcopy(mapped)
    adjusted['streams'] = [s for s in mapped['streams'] if s['stream_index'] not in protected]
    adjusted['dropped_streams'] = sorted(
        adjusted['dropped_streams'] + [{'stream_index': s['stream_index'],
                                        'fourcc': protected[s['stream_index']]} for s in moved],
        key=lambda d: d['stream_index'] if d['stream_index'] is not None else -1)
    return adjusted


def _kd_fragment_origin(mapped, evidence):
    """KD-P-FRAGMENT-ORIGIN: ffprobe's durations of a fragmented stream run
    from 0 to its end, counting the first fragment's decode time; CoreAV's
    stream duration is the media's length."""
    origins = {k: v for k, v in (evidence.get('origins') or {}).items() if v > 0}
    if not evidence.get('fragmented') or not origins:
        return None
    adjusted = copy.deepcopy(mapped)
    ends = []
    for stream in adjusted['streams']:
        origin = origins.get(stream['stream_index'])
        time = stream['duration']
        if origin is None or time is None:
            return None
        length = Fraction(time['ticks']['num'], time['ticks']['den'] * time['timescale']) - origin
        if length < 0:
            return None
        stream['duration'] = {'ticks': {'num': length.numerator, 'den': length.denominator},
                              'timescale': 1, 'seconds': float(length)}
        ends.append(length)
    if not ends:
        return None
    longest = max(ends)
    adjusted['duration'] = {'ticks': {'num': longest.numerator, 'den': longest.denominator},
                            'timescale': 1, 'seconds': float(longest)}
    return adjusted


def _kd_unidentified_duration(mapped, evidence):
    """KD-P-UNIDENTIFIED-DURATION: in a fragmented file without a movie
    extends header duration whose streams are all unidentified, CoreAV
    knows no duration; ffprobe gives one."""
    if not evidence.get('fragmented') or evidence.get('mehd') or mapped['streams'] \
            or mapped['duration'] is None:
        return None
    adjusted = copy.deepcopy(mapped)
    adjusted['duration'] = None
    return adjusted


def _kd_chapter_text(mapped, evidence):
    """KD-P-CHAPTER-TEXT: a text track that a `chap` reference names is a
    data stream for ffprobe; CoreAV describes it by its handler and sample
    entry, like any other track."""
    chapters = (evidence.get('headers') or {}).get('chapters') or set()
    entries = evidence.get('entries') or {}
    moved = [d for d in mapped['dropped_streams'] if d['stream_index'] in chapters
             and (entries.get(d['stream_index']) or (None,))[0] in ('text', 'sbtl', 'subt')]
    if not moved:
        return None
    adjusted = copy.deepcopy(mapped)
    adjusted['dropped_streams'] = [d for d in mapped['dropped_streams'] if d not in moved]
    for d in moved:
        if (entries.get(d['stream_index']) or (None, None))[1] == 'tx3g':
            adjusted['streams'].append({'stream_index': d['stream_index'], 'kind': 'subtitle',
                                        'codec': 'timed_text', 'duration': d.get('duration'),
                                        'bit_rate': None, 'video': None, 'audio': None})
        else:
            adjusted['dropped_streams'].append(d)
    adjusted['streams'].sort(key=lambda s: s['stream_index'])
    adjusted['dropped_streams'].sort(key=lambda s: s['stream_index'])
    return adjusted


def _kd_declared_duration(mapped, evidence):
    """KD-P-DECLARED-DURATION: CoreAV takes a fragmented file's duration
    from `mehd`; ffprobe measures the fragments present."""
    mehd = evidence.get('mehd')
    scale = (evidence.get('headers') or {}).get('timescale')
    if not evidence.get('fragmented') or not mehd or not scale:
        return None
    declared = Fraction(mehd, scale)
    adjusted = copy.deepcopy(mapped)
    adjusted['duration'] = {'ticks': {'num': declared.numerator, 'den': declared.denominator},
                            'timescale': 1, 'seconds': float(declared)}
    return adjusted


def _kd_edited_bit_rate(mapped, evidence):
    """KD-P-EDITED-BIT-RATE: for a track whose edit list presents less than
    its media, CoreAV's bit rate is every sample's bytes over the duration
    the edit list presents; ffprobe reports
    the stored rate, the `btrt` average or the bytes over the media header
    duration. Both are recomputed from the file."""
    timing = evidence.get('timing') or {}
    movie_timescale = (evidence.get('headers') or {}).get('timescale')
    if evidence.get('fragmented'):
        return None
    moved = {}
    for stream in mapped['streams']:
        track = timing.get(stream['stream_index'])
        rate = stream.get('bit_rate')
        if not track or not rate or not track['bytes'] or not track['mdhd']:
            continue
        duration = edited_track_duration(track, movie_timescale)
        if duration is None or duration <= 0 or duration >= track['mdhd']:
            continue
        theirs = Fraction(rate['exact']['num'], rate['exact']['den'])
        stored = Fraction(track['bytes'] * 8 * track['timescale'], track['mdhd'])
        if theirs != track['btrt'] and abs(theirs - stored) > 1:
            continue
        moved[stream['stream_index']] = Fraction(track['bytes'] * 8 * track['timescale']) / duration
    if not moved:
        return None
    adjusted = copy.deepcopy(mapped)
    for stream in adjusted['streams']:
        if stream['stream_index'] in moved:
            ours = moved[stream['stream_index']]
            stream['bit_rate'] = {'exact': {'num': ours.numerator, 'den': ours.denominator},
                                  'bits_per_second': float(ours)}
    return adjusted


KNOWN_DIVERGENCES = {
    'KD-P-PROTECTED': _kd_protected,
    'KD-P-CHAPTER-TEXT': _kd_chapter_text,
    'KD-P-DECLARED-DURATION': _kd_declared_duration,
    'KD-P-FRAGMENT-ORIGIN': _kd_fragment_origin,
    'KD-P-UNIDENTIFIED-DURATION': _kd_unidentified_duration,
    'KD-P-EDITED-BIT-RATE': _kd_edited_bit_rate,
}


# Video codecs whose frames the Matroska scan does not read for a picture
# size (docs/design/policy-matroska.md, decision 3).
UNSIZED = frozenset({'mpeg1video', 'mpeg2video', 'mpeg4', 'theora', 'mjpeg', 'ffv1'})


def _kd_mkv_unsized(mapped, evidence):
    """KD-P-MKV-UNSIZED: CoreAV reads no picture size from the frames of
    MPEG-1, MPEG-2, MPEG-4 Part 2, Theora, MJPEG and FFV1 video, so their
    verified sizes are unknown; ffprobe's decoder gives a size."""
    moved = [s for s in mapped['streams'] if s['codec'] in UNSIZED and s.get('video')
             and (s['video'].get('scanned_size') or s['video'].get('parameter_set_size'))]
    if not moved:
        return None
    adjusted = copy.deepcopy(mapped)
    for stream in adjusted['streams']:
        if stream['codec'] in UNSIZED and stream.get('video'):
            stream['video']['parameter_set_size'] = None
            stream['video']['scanned_size'] = None
    return adjusted


def _with_span(stream, span, total):
    stream['duration'] = None if span is None else _time(span)
    stream['bit_rate'] = None
    if span is not None and span > 0:
        rate = Fraction(total * 8) / span
        stream['bit_rate'] = {'exact': {'num': rate.numerator, 'den': rate.denominator},
                              'bits_per_second': float(rate)}


def _tracks_by_stream(mapped, evidence):
    """(stream, its walked TrackEntry) of each mapped stream, by position
    among the TrackEntry elements (`stream_index`)."""
    tracks = (evidence.get('matroska') or {}).get('tracks') or []
    for stream in mapped['streams']:
        index = stream['stream_index']
        if isinstance(index, int) and 0 <= index < len(tracks):
            yield stream, tracks[index]


def _kd_mkv_lace_times(mapped, evidence):
    """KD-P-MKV-LACE-TIMES: a later frame of a lace in a track without
    DefaultDuration has no time in Matroska unless its codec states the
    frames' durations (CoreAV reads Opus's); CoreAV leaves the stream's
    duration unknown, ffprobe infers one."""
    laced = evidence.get('laced') or set()
    hit = [stream['stream_index'] for stream, track in _tracks_by_stream(mapped, evidence)
           if track.get('number') in laced and track.get('default_duration') is None
           and stream['codec'] != 'opus' and stream['duration'] is not None]
    if not hit:
        return None
    adjusted = copy.deepcopy(mapped)
    for stream in adjusted['streams']:
        if stream['stream_index'] in hit:
            _with_span(stream, None, 0)
    return adjusted


def _kd_mkv_frame_end(mapped, evidence):
    """KD-P-MKV-FRAME-END: an audio or video frame without BlockDuration
    or DefaultDuration ends where it starts for CoreAV, unless its codec
    states its duration (Opus); ffprobe adds the duration its parser
    derives, so the stream's span is one frame longer."""
    spans = evidence.get('spans') or {}
    hit = {}
    for stream, track in _tracks_by_stream(mapped, evidence):
        index = stream['stream_index']
        if stream['kind'] in ('audio', 'video') and track.get('default_duration') is None \
                and stream['codec'] != 'opus' and stream['duration'] is not None \
                and index in spans \
                and spans[index][2] is not None and spans[index][2] != spans[index][0]:
            hit[index] = spans[index]
    if not hit:
        return None
    adjusted = copy.deepcopy(mapped)
    for stream in adjusted['streams']:
        if stream['stream_index'] in hit:
            _, total, starts = hit[stream['stream_index']]
            _with_span(stream, starts, total)
    return adjusted


def _kd_mkv_unlisted(mapped, evidence):
    """KD-P-MKV-UNLISTED: the frames of a track compressed by an encoding
    CoreAV does not undo (zlib and others) are not listed, so its duration
    and bit rate are unknown; ffprobe decompresses and times them."""
    hit = [stream['stream_index'] for stream, track in _tracks_by_stream(mapped, evidence)
           if track.get('encoding') == 'unsupported'
           and (stream['duration'] is not None or stream['bit_rate'] is not None)]
    if not hit:
        return None
    adjusted = copy.deepcopy(mapped)
    for stream in adjusted['streams']:
        if stream['stream_index'] in hit:
            _with_span(stream, None, 0)
    return adjusted


def _kd_mkv_unverified(mapped, evidence):
    """KD-P-MKV-UNVERIFIED: a structure that may hide frames from the block
    listing (bytes after the Segment, a second Tracks, two Blocks in a
    BlockGroup and the like) fails CoreAV's scan, so no stream's duration,
    bit rate or scanned size is known; ffprobe reads past it."""
    if not evidence.get('audit') or not mapped['streams']:
        return None
    adjusted = copy.deepcopy(mapped)
    for stream in adjusted['streams']:
        _with_span(stream, None, 0)
        if stream.get('video'):
            stream['video']['scanned_size'] = None
    return adjusted


# docs/known-divergences-policy.md, Matroska entries.
MATROSKA_DIVERGENCES = {
    'KD-P-MKV-UNVERIFIED': _kd_mkv_unverified,
    'KD-P-MKV-UNLISTED': _kd_mkv_unlisted,
    'KD-P-MKV-UNSIZED': _kd_mkv_unsized,
    'KD-P-MKV-LACE-TIMES': _kd_mkv_lace_times,
    'KD-P-MKV-FRAME-END': _kd_mkv_frame_end,
}


def covered(policy, excluded, mapped, evidence, ours, matroska=False):
    """The ids of the known divergences that, applied together, make the
    oracle's outcome equal `ours`, without any that the agreement does not
    need; None when they do not."""
    entries = MATROSKA_DIVERGENCES if matroska else KNOWN_DIVERGENCES

    def agrees(ids):
        doc = mapped
        for kd in ids:
            doc = entries[kd](doc, evidence) or doc
        return outcome(evaluate(observable(policy, excluded), doc), excluded) == ours

    applicable = [kd for kd, adjust in entries.items()
                  if adjust(mapped, evidence) is not None]
    if not applicable or not agrees(applicable):
        return None
    needed = list(applicable)
    for kd in applicable:
        fewer = [k for k in needed if k != kd]
        if fewer and agrees(fewer):
            needed = fewer
    return needed
