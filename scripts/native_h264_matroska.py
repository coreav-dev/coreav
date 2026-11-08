"""Independent generated-output checks from RFC 9559 and ISO/IEC 14496-15."""
from fractions import Fraction
import hashlib

import track_boxes


def require(condition, label):
    if not condition:
        raise ValueError(label)


def round_half_up(value):
    value = Fraction(value)
    return (2 * value.numerator + value.denominator) // (2 * value.denominator)


def round_timestamp(value):
    return -round_half_up(-value) if value < 0 else round_half_up(value)


def vint(data, at, *, identifier=False):
    require(at < len(data) and data[at] != 0, 'EBML invalid variable integer')
    width = 9 - data[at].bit_length()
    require(width <= (4 if identifier else 8) and at + width <= len(data),
            'EBML truncated variable integer')
    value = int.from_bytes(data[at:at + width], 'big')
    if not identifier:
        value &= (1 << (7 * width)) - 1
        require(value != (1 << (7 * width)) - 1, 'EBML unknown element size')
    return value, at + width


def elements(data):
    at = 0
    while at < len(data):
        kind, at = vint(data, at, identifier=True)
        size, at = vint(data, at)
        require(at + size <= len(data), 'EBML element exceeds parent')
        yield kind, data[at:at + size]
        at += size


def one(children, kind, label, default=None):
    found = [body for key, body in children if key == kind]
    require(len(found) <= 1, f'{label}: duplicate element')
    require(found or default is not None, f'{label}: missing element')
    return found[0] if found else default


def uint(data):
    require(0 < len(data) <= 8, 'EBML invalid unsigned integer')
    return int.from_bytes(data, 'big')


def avcc(data):
    require(len(data) >= 7 and data[0] == 1 and data[4] & 0xfc == 0xfc
            and data[5] & 0xe0 == 0xe0, 'CodecPrivate: invalid AVC configuration')
    length_size = (data[4] & 3) + 1
    require(length_size in (1, 2, 4), 'CodecPrivate: invalid NAL length size')
    at, parameters = 6, []
    for kind, count in ((7, data[5] & 31), (8, None)):
        if count is None:
            require(at < len(data), 'CodecPrivate: missing PPS count')
            count, at = data[at], at + 1
        require(count > 0, 'CodecPrivate: missing parameter sets')
        for _ in range(count):
            require(at + 2 <= len(data), 'CodecPrivate: truncated parameter length')
            size = int.from_bytes(data[at:at + 2], 'big')
            at += 2
            require(size > 0 and at + size <= len(data), 'CodecPrivate: truncated parameter set')
            nal, at = data[at:at + size], at + size
            require(nal[0] & 31 == kind, 'CodecPrivate: incorrect parameter type')
            parameters.append(nal)
    require(at == len(data), 'CodecPrivate: trailing bytes')
    return length_size, parameters


def access_unit(data, length_size):
    at, nals = 0, []
    while at < len(data):
        require(at + length_size <= len(data), 'access unit: truncated NAL length')
        size = int.from_bytes(data[at:at + length_size], 'big')
        at += length_size
        require(size > 0 and at + size <= len(data), 'access unit: truncated NAL')
        nals.append(data[at:at + size])
        at += size
    require(nals, 'access unit: empty packet')
    return nals


def read(data):
    top = list(elements(data))
    one(top, 0x1a45dfa3, 'EBML header')
    segment = list(elements(one(top, 0x18538067, 'Segment')))
    info = list(elements(one(segment, 0x1549a966, 'Info')))
    scale = uint(one(info, 0x2ad7b1, 'TimestampScale'))
    tracks = [list(elements(body)) for kind, body in elements(one(segment, 0x1654ae6b, 'Tracks'))
              if kind == 0xae]
    video = [track for track in tracks if uint(one(track, 0x83, 'TrackType')) == 1]
    require(len(video) == 1, 'video: expected one track')
    track, = video
    number = uint(one(track, 0xd7, 'TrackNumber'))
    require(number > 0, 'video: zero track number')
    require(one(track, 0x86, 'CodecID') == b'V_MPEG4/ISO/AVC', 'video: incorrect CodecID')
    private = one(track, 0x63a2, 'CodecPrivate')
    length_size, parameters = avcc(private)
    duration = uint(one(track, 0x23e383, 'DefaultDuration'))
    dimensions = list(elements(one(track, 0xe0, 'Video')))
    width = uint(one(dimensions, 0xb0, 'PixelWidth'))
    height = uint(one(dimensions, 0xba, 'PixelHeight'))
    require(width > 0 and height > 0, 'video: zero geometry')
    display_unit = uint(one(dimensions, 0x54b2, 'DisplayUnit', b'\0'))
    require(display_unit in (0, 3), 'video: unsupported display unit')
    dw = uint(one(dimensions, 0x54b0, 'DisplayWidth', width.to_bytes(8, 'big')))
    dh = uint(one(dimensions, 0x54ba, 'DisplayHeight', height.to_bytes(8, 'big')))
    require(dw > 0 and dh > 0, 'video: zero display geometry')
    packets = []
    for kind, body in segment:
        if kind != 0x1f43b675:
            continue
        cluster = list(elements(body))
        timestamp = uint(one(cluster, 0xe7, 'Cluster Timestamp'))
        for kind, payload in cluster:
            if kind not in (0xa3, 0xa0):
                continue
            group = list(elements(payload)) if kind == 0xa0 else []
            block = one(group, 0xa1, 'Block') if group else payload
            track_number, at = vint(block, 0)
            require(at + 3 <= len(block), 'Block: truncated header')
            if track_number != number:
                continue
            require(not any(key == 0x9b for key, _ in group), 'video: conflicting BlockDuration')
            flags = block[at + 2]
            require(flags & 6 == 0, 'video: unexpected lacing')
            pts = timestamp + int.from_bytes(block[at:at + 2], 'big', signed=True)
            key = not any(key == 0xfb for key, _ in group) if group else bool(flags & 0x80)
            payload = block[at + 3:]
            nals = access_unit(payload, length_size)
            require(key == any(nal[0] & 31 == 5 for nal in nals), 'video: keyframe flag disagrees with IDR')
            packets.append(dict(pts=pts, key=key, data=payload, nals=nals))
    require(packets, 'video: missing packets')
    return dict(scale=scale, duration=duration, width=width, height=height,
                dar=Fraction(dw, dh), private=private, parameters=parameters, packets=packets)


def mp4_video(path):
    movie = track_boxes.read_movie(path)
    video = [track for track in movie['tracks'] if track['handler'] == 'vide']
    require(len(video) == 1, 'MP4: expected one video track')
    track, = video
    entry = track['entry']
    require(entry['type'] == 'avc1', 'MP4: expected AVC sample entry')
    private_box = track_boxes.child(entry['payload'], (78, len(entry['payload'])), b'avcC')
    require(private_box is not None, 'MP4: missing avcC')
    private = entry['payload'][private_box[0]:private_box[1]]
    samples = track['samples']
    packets = list(track_boxes.iter_sample_bytes(path, samples['offsets'], samples['sizes'], 0,
                                                len(samples['sizes'])))
    return private, packets, list(samples['sync'])


def opus_mp4_source(path):
    original = path.read_bytes()
    movie = track_boxes.read_movie(path)
    tracks = [track for track in movie['tracks'] if track['handler'] == 'soun'
              and track['entry']['type'] == 'Opus']
    require(len(tracks) == 1, 'source Opus: expected one MP4 audio track')
    track, = tracks
    require(track['timescale'] == 48_000, 'source Opus: expected 48 kHz media clock')
    require(track['edits'] is not None and len(track['edits']) == 1,
            'source Opus: expected one original presentation edit')
    duration, start, speed = track['edits'][0]
    require(start >= 0 and duration > 0 and speed == 1, 'source Opus: invalid original presentation edit')
    samples = Fraction(duration * 48_000, movie['movie_timescale'])
    require(samples.denominator == 1, 'source Opus: fractional presentation sample')
    channels = track['entry']['channels']
    moov = track_boxes.child(original, (0, len(original)), b'moov')
    offsets = []
    for trak in track_boxes.children(original, moov, b'trak'):
        mdia = track_boxes.child(original, trak, b'mdia')
        hdlr = track_boxes.child(original, mdia, b'hdlr')
        if hdlr is None or original[hdlr[0] + 8:hdlr[0] + 12] != b'soun':
            continue
        parent = track_boxes.child(original, mdia, b'minf')
        parent = track_boxes.child(original, parent, b'stbl')
        parent = track_boxes.child(original, parent, b'stsd')
        require(parent is not None, 'source Opus: missing original sample entry')
        entry = track_boxes.child(original, (parent[0] + 8, parent[1]), b'Opus')
        if entry is None:
            continue
        dops = track_boxes.child(original, (entry[0] + 28, entry[1]), b'dOps')
        require(dops is not None and dops[1] - dops[0] >= 11, 'source Opus: missing original dOps')
        require(original[dops[0]] == 0 and original[dops[0] + 1] == channels,
                'source Opus: invalid original dOps')
        offsets.append(dops[0] + 2)
    require(len(offsets) == 1, 'source Opus: expected one original dOps pre-skip')
    offset, = offsets
    pre_skip = int.from_bytes(original[offset:offset + 2], 'big')
    clone = bytearray(original)
    clone[offset:offset + 2] = b'\0\0'
    table = track['samples']
    packets = list(track_boxes.iter_sample_bytes(path, table['offsets'], table['sizes'], 0, len(table['sizes'])))
    clocks, dts = [], table['base_dts']
    for duration, cto in zip(table['durations'], table['cto']):
        clocks.append(Fraction(dts + cto - start, 48_000))
        dts += duration
    return dict(clone=bytes(clone), channels=channels, start_samples=start, declared_samples=int(samples),
                pre_skip=pre_skip, source_sha256=hashlib.sha256(original).hexdigest(),
                packet_sha256=[hashlib.sha256(packet).hexdigest() for packet in packets],
                clocks=clocks, original_edit=list(track['edits'][0]))


def opus_packet_samples(packet):
    require(packet, 'source Opus: empty packet')
    configuration = packet[0] >> 3
    if configuration < 12:
        per_frame = (480, 960, 1920, 2880)[configuration & 3]
    elif configuration < 16:
        per_frame = (480, 960)[configuration & 1]
    else:
        per_frame = (120, 240, 480, 960)[configuration & 3]
    code = packet[0] & 3
    require(code != 3 or len(packet) >= 2, 'source Opus: missing frame count')
    frames = (1, 2, 2, packet[1] & 63 if len(packet) >= 2 else 0)[code]
    require(0 < frames * per_frame <= 5760, 'source Opus: invalid packet duration')
    return frames * per_frame


def opus_matroska_source(data):
    segment = list(elements(one(list(elements(data)), 0x18538067, 'source Segment')))
    info = list(elements(one(segment, 0x1549a966, 'source Info')))
    scale = uint(one(info, 0x2ad7b1, 'source TimestampScale', b'\x0f\x42\x40'))
    tracks = [list(elements(body)) for kind, body in elements(one(segment, 0x1654ae6b, 'source Tracks'))
              if kind == 0xae]
    tracks = [track for track in tracks if uint(one(track, 0x83, 'source TrackType')) == 2
              and one(track, 0x86, 'source CodecID') == b'A_OPUS']
    require(len(tracks) == 1, 'source Opus: expected one Matroska audio track')
    track, = tracks
    number = uint(one(track, 0xd7, 'source TrackNumber'))
    private = one(track, 0x63a2, 'source CodecPrivate')
    require(len(private) >= 19 and private[:8] == b'OpusHead', 'source Opus: invalid original OpusHead')
    channels = private[9]
    require(channels > 0, 'source Opus: zero channels')
    delay = uint(one(track, 0x56aa, 'source CodecDelay', b'\0'))
    delay_samples = Fraction(delay * 48_000, 1_000_000_000)
    require(delay_samples.denominator == 1, 'source Opus: fractional codec delay sample')
    packets, clocks, padding_samples = [], [], 0
    for kind, body in segment:
        if kind != 0x1f43b675:
            continue
        cluster = list(elements(body))
        timestamp = uint(one(cluster, 0xe7, 'source Cluster Timestamp'))
        for kind, payload in cluster:
            if kind not in (0xa3, 0xa0):
                continue
            group = list(elements(payload)) if kind == 0xa0 else []
            block = one(group, 0xa1, 'source Block') if group else payload
            track_number, at = vint(block, 0)
            require(at + 3 <= len(block), 'source Block: truncated header')
            if track_number != number:
                continue
            require(block[at + 2] & 6 == 0, 'source Opus: unexpected lacing')
            relative = int.from_bytes(block[at:at + 2], 'big', signed=True)
            packets.append(block[at + 3:])
            clocks.append(Fraction((timestamp + relative) * scale - delay, 1_000_000_000))
            padding = one(group, 0x75a2, 'source DiscardPadding', b'\0')
            padding = Fraction(int.from_bytes(padding, 'big', signed=True) * 48_000, 1_000_000_000)
            require(padding.denominator == 1 and padding >= 0, 'source Opus: invalid discard padding samples')
            padding_samples += int(padding)
    samples = sum(opus_packet_samples(packet) for packet in packets) - int(delay_samples) - padding_samples
    require(samples > 0, 'source Opus: empty declared presentation')
    return dict(channels=channels, declared_samples=samples, pre_skip=int.from_bytes(private[10:12], 'little'),
                codec_delay_samples=int(delay_samples), discard_padding_samples=padding_samples,
                source_sha256=hashlib.sha256(data).hexdigest(),
                packet_sha256=[hashlib.sha256(packet).hexdigest() for packet in packets], clocks=clocks)


def opus_presentation_slice(decoded, source):
    stride = source['channels'] * 4
    require(stride > 0 and len(decoded) % stride == 0, 'source Opus: incomplete decoded sample')
    start = source['start_samples'] * stride
    end = start + source['declared_samples'] * stride
    require(0 <= start < end <= len(decoded), 'source Opus: original edit exceeds decoded samples')
    return decoded[start:end]


def compare_opus_pcm(actual, expected, source):
    size = source['declared_samples'] * source['channels'] * 4
    require(len(expected) == size, 'source Opus: reference does not match original declared presentation')
    require(len(actual) == size, 'copied Opus: original declared sample count differs')
    require(actual == expected, 'copied Opus: complete original-source PCM differs')


def compare_opus_matroska_timeline(output, reported_pts, source):
    for field in ('clocks', 'codec_delay_samples', 'discard_padding_samples', 'declared_samples', 'pre_skip'):
        require(output[field] == source[field], f'copied Opus: original Matroska {field} differs')
    require(reported_pts == [Fraction(value) for value in source['reported_pts']],
            'copied Opus: original Matroska oracle PTS readback differs')


def validate(data, *, rate, sar, width, height, times, sps_reader, mp4):
    result = read(data)
    require(result['scale'] == 1_000_000, 'TimestampScale: expected one millisecond')
    require(result['duration'] == round_half_up(Fraction(1_000_000_000, 1) / rate),
            'DefaultDuration: exact nominal period rounded half up')
    require((result['width'], result['height']) == (width, height), 'video: pixel geometry differs')
    require(result['dar'] == Fraction(width, height) * sar, 'video: display DAR differs')
    packets = result['packets']
    expected_pts = [round_timestamp(time * 1000) for time in times]
    require(len(packets) == len(times), 'video: picture count differs')
    require([p['pts'] for p in packets] == expected_pts, 'video: absolute rounded PTS differs')
    require(all(a < b for a, b in zip(expected_pts, expected_pts[1:])), 'video: rounded PTS collision')
    for packet, time in zip(packets, times):
        require(abs(Fraction(packet['pts'], 1000) - time) <= Fraction(1, 2000),
                'video: PTS quantization exceeds half a millisecond')
    for nal in result['parameters'] + [n for p in packets for n in p['nals'] if n[0] & 31 == 7]:
        if nal[0] & 31 == 7:
            require(sps_reader(nal) == (rate, None if sar == 1 else sar), 'SPS: exact timing or SAR differs')
    private, expected_packets, keys = mp4
    require(result['private'] == private, 'CodecPrivate: same-head MP4 differs')
    require([p['data'] for p in packets] == expected_packets, 'access units: same-head MP4 differs')
    require([int(p['key']) for p in packets] == keys, 'video: same-head MP4 key flags differ')
    return result
