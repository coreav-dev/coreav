"""Exact decoded-output comparisons for current fixture CI, not frozen rounds."""
import fractions
import hashlib
import json
import os
import pathlib
import re
import signal
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / 'scripts'))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT

SECONDS = 120
AUDIO = ('sample_fmt', 'sample_rate', 'channels', 'channel_layout', 'bits_per_raw_sample')
VIDEO = ('width', 'height', 'pix_fmt', 'sample_aspect_ratio', 'color_range', 'color_space',
         'color_transfer', 'color_primaries', 'chroma_location', 'field_order')
UNKNOWN = (None, '', 'unknown', 'unspecified', 'N/A', '0:1', '0')
LOSSLESS = {'flac', 'png', 'bmp', 'rawvideo', 'ffv1', 'huffyuv'}
PCM = re.compile(r'pcm_(?:[us]8|[us](?:16|24|32)(?:le|be)|f(?:32|64)(?:le|be))\Z')


def invoke(argv, env, cwd=None, output=None):
    sink = open(output, 'wb') if output else None
    try:
        with subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                              stdout=sink if sink else subprocess.PIPE,
                              stderr=subprocess.PIPE, start_new_session=True) as child:
            try:
                stdout, stderr = child.communicate(timeout=SECONDS)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                child.communicate()
                return None, b'', b'timeout'
            return child.returncode, stdout or b'', stderr
    finally:
        if sink:
            sink.close()


def probe(path, ffprobe, env, inputs=()):
    code, data, err = invoke([ffprobe, '-v', 'error', *inputs, *AUDIO_DECODE_INPUT, '-i', str(path),
                              '-show_streams', '-show_frames', '-of', 'json'], env)
    if code != 0 or err:
        return None
    try:
        result = json.loads(data)
        if not isinstance(result, dict) or not isinstance(result.get('streams'), list):
            return None
        if not result['streams'] or any(not isinstance(s, dict) for s in result['streams']):
            return None
        if not isinstance(result.get('frames', []), list):
            return None
        return result
    except (ValueError, TypeError):
        return None


def value(item):
    return None if item in UNKNOWN else item


def time(stream, key):
    number = stream.get(key)
    if number in (None, 'N/A'):
        return None
    return int(number) * fractions.Fraction(stream['time_base'])


def metadata(mine, reference):
    kind = reference.get('codec_type')
    if kind not in ('audio', 'video'):
        return False
    fields = ('codec_type', 'codec_name') + (AUDIO if kind == 'audio' else VIDEO)
    required = ('sample_fmt', 'sample_rate', 'channels') if kind == 'audio' else ('width', 'height', 'pix_fmt')
    if any(value(reference.get(k)) is None or value(mine.get(k)) is None for k in required):
        return False
    try:
        numeric = ('sample_rate', 'channels') if kind == 'audio' else ('width', 'height')
        if any(int(s[k]) <= 0 for s in (mine, reference) for k in numeric):
            return False
    except (TypeError, ValueError, KeyError):
        return False
    if any(value(mine.get(k)) != value(reference.get(k)) for k in fields):
        return False
    try:
        return all(time(mine, key) == time(reference, key) for key in ('start_pts', 'duration_ts'))
    except (KeyError, ValueError, TypeError, ZeroDivisionError):
        return False


def same_files(a, b):
    with open(a, 'rb') as left, open(b, 'rb') as right:
        while True:
            x, y = left.read(1024 * 1024), right.read(1024 * 1024)
            if x != y:
                return False
            if not x:
                return True


def raw_inputs(args, source):
    last_input = max((i for i, a in enumerate(args) if a == '-i'), default=-1)
    output = args[last_input + 2:]
    options = dict(zip(output, output[1:]))
    form = options.get('-f')
    def selected(kind):
        streams = [s for s in source['streams'] if s.get('codec_type') == kind
                   and (kind != 'video' or not s.get('disposition', {}).get('attached_pic'))]
        maps = [output[i + 1] for i, a in enumerate(output[:-1]) if a == '-map']
        if maps:
            if len(maps) != 1:
                raise ValueError('raw output maps')
            match = re.fullmatch(r'0:(\d+|[av](?::\d+)?)', maps[0])
            if not match:
                raise ValueError('raw output map')
            spec = match.group(1)
            if spec.isdigit():
                streams = [s for s in streams if s.get('index') == int(spec)]
            elif spec[0] != kind[0]:
                streams = []
            else:
                index = int(spec.split(':')[1]) if ':' in spec else 0
                streams = streams[index:index + 1]
        elif kind == 'audio':
            streams = sorted(streams, key=lambda s: -int(s.get('channels', 0)))
        else:
            streams = sorted(streams, key=lambda s: -int(s.get('width', 0)) * int(s.get('height', 0)))
        if not streams:
            raise ValueError('raw source stream')
        return streams[0]
    if form in ('s16le', 'rawvideo'):
        if args.count('-i') != 1 or any(a in args[:last_input] for a in ('-r', '-framerate', '-video_size')):
            raise ValueError('raw input selection')
    if form == 's16le':
        audio = selected('audio')
        rate = int(options.get('-ar', audio.get('sample_rate', 0)))
        channels = int(options.get('-ac', audio.get('channels', 0)))
        if rate <= 0 or channels not in (1, 2):
            raise ValueError('raw audio description')
        return ['-f', form, '-sample_rate', str(rate), '-ch_layout', 'mono' if channels == 1 else 'stereo']
    if form == 'rawvideo':
        video = selected('video')
        size = options.get('-s', f'{video.get("width")}x{video.get("height")}')
        if not re.fullmatch(r'[1-9][0-9]*x[1-9][0-9]*', size):
            raise ValueError('raw video geometry')
        pixel = options.get('-pix_fmt', video.get('pix_fmt'))
        rate = options.get('-r', video.get('r_frame_rate'))
        if not pixel or not rate or fractions.Fraction(rate) <= 0:
            raise ValueError('raw video format')
        return ['-f', form, '-pixel_format', pixel, '-video_size', size, '-framerate', rate]
    return []


def video_command(ffmpeg, source, inputs, index, decoded):
    return [ffmpeg, '-nostdin', '-v', 'error', '-xerror', '-noautorotate', *inputs,
            '-i', str(source), '-map', f'0:{index}', '-noautoscale', *decoded, '-']


def compare(mine, reference, ffmpeg, ffprobe, env, scratch, inputs=(), stream=None, copied_streams=(),
            registered_codecs=(), exact_frame_timeline=False):
    expected = probe(reference, ffprobe, env, inputs)
    if expected is None:
        return 'unverified', 'reference probe'
    indices = list(range(len(expected['streams']))) if stream is None else [stream]
    if any(index < 0 or index >= len(expected['streams']) for index in indices):
        return 'unverified', 'reference stream mapping'
    for index in indices:
        description = expected['streams'][index]
        codec = description.get('codec_name', '')
        copied = index in copied_streams and codec in ('h264', 'aac', 'vp9', 'opus')
        if not copied and codec not in LOSSLESS | {'mp3'} | set(registered_codecs) and not PCM.fullmatch(codec):
            return 'unverified', f'unregistered codec {codec}'
        if description.get('codec_type') == 'audio' and description.get('sample_fmt') not in (
                'u8', 'u8p', 's16', 's16p', 's32', 's32p', 'flt', 'fltp', 'dbl', 'dblp'):
            return 'unverified', 'reference sample format'
        if description.get('codec_type') == 'video':
            frames = [f for f in expected.get('frames', []) if f.get('stream_index') == description.get('index')]
            if any(f.get('pix_fmt') != description.get('pix_fmt') for f in frames):
                return 'unverified', 'changing reference pixel format'
    actual = probe(mine, ffprobe, env, inputs)
    if actual is None:
        return 'failed', 'candidate probe'
    if len(actual['streams']) != len(expected['streams']):
        return 'failed', 'stream count'
    if copied_streams and any([s.get('index') for s in document['streams']] != list(range(len(document['streams'])))
                              for document in (actual, expected)):
        return 'failed', 'stream indices'
    for index in indices:
        ours, theirs = actual['streams'][index], expected['streams'][index]
        if not metadata(ours, theirs):
            return 'failed', 'stream fields or timing'
        kind = theirs['codec_type']
        if kind == 'audio':
            if index in copied_streams or exact_frame_timeline:
                try:
                    timelines = []
                    for document, stream in ((actual, ours), (expected, theirs)):
                        frames = [f for f in document.get('frames', []) if f.get('stream_index') == index]
                        if not frames:
                            return 'failed', 'no audio frames'
                        timeline = []
                        for frame in frames:
                            clock = {**frame, 'time_base': stream['time_base']}
                            pts, duration = time(clock, 'best_effort_timestamp'), time(clock, 'duration')
                            samples = int(frame['nb_samples'])
                            if pts is None or duration is None or duration <= 0 or samples <= 0:
                                return 'failed', 'invalid audio timing'
                            timeline.append((pts, duration, samples))
                        timelines.append(timeline)
                    if timelines[0] != timelines[1]:
                        return 'failed', 'audio frame timing or count'
                except (KeyError, TypeError, ValueError, ZeroDivisionError):
                    return 'failed', 'invalid audio timing'
            floating = theirs['sample_fmt'].startswith(('flt', 'dbl')) or theirs['codec_name'] == 'mp3'
            codec, form, size = ('pcm_f64le', 'f64le', 8) if floating else ('pcm_s32le', 's32le', 4)
            decoded = ['-c:a', codec, '-f', form]
            unit = size * int(theirs['channels'])
        else:
            decoded = ['-c:v', 'rawvideo', '-pix_fmt', theirs['pix_fmt'], '-fps_mode', 'passthrough',
                       '-f', 'rawvideo']
            unit = 1
            try:
                timelines = []
                for document, stream in ((actual, ours), (expected, theirs)):
                    frames = [f for f in document.get('frames', []) if f.get('stream_index') == index]
                    if not frames:
                        return 'failed', 'no video frames'
                    if any(any(f.get(k) != stream.get(k) for k in ('width', 'height', 'pix_fmt')) for f in frames):
                        return 'failed', 'video frame format'
                    timelines.append([(time({**f, 'time_base': stream['time_base']}, 'best_effort_timestamp'),
                                       time({**f, 'time_base': stream['time_base']}, 'duration')) for f in frames])
                    if exact_frame_timeline and any(pts is None or duration is None or duration <= 0
                                                    for pts, duration in timelines[-1]):
                        return 'failed', 'invalid video timing'
                if timelines[0] != timelines[1]:
                    return 'failed', 'video frame timing or count'
            except (KeyError, TypeError, ValueError, ZeroDivisionError):
                return 'failed', 'invalid video timing'
        paths = [scratch / f'decode-{index}-{side}' for side in ('candidate', 'reference')]
        for source, dest in zip((mine, reference), paths):
            argv = ([ffmpeg, '-nostdin', '-v', 'error', '-xerror', *inputs, *AUDIO_DECODE_INPUT,
                     '-i', str(source), '-map', f'0:{index}', *decoded, '-'] if kind == 'audio'
                    else video_command(ffmpeg, source, inputs, index, decoded))
            code, _, err = invoke(argv, env, output=dest)
            if code != 0 or err or not dest.stat().st_size or dest.stat().st_size % unit:
                return 'failed', 'decode or incomplete samples'
            if kind == 'audio' and floating:
                import math
                import struct
                with open(dest, 'rb') as data:
                    while block := data.read(65536):
                        if any(not math.isfinite(v[0]) for v in struct.iter_unpack('<d', block)):
                            return 'failed', 'nonfinite samples'
        if not same_files(*paths):
            return 'failed', 'decoded samples' if kind == 'audio' else 'decoded pixels'
    return 'verified', 'exact decoded output'


def hexdump(text):
    result = bytearray()
    lines = text.strip('\n').splitlines()
    for index, line in enumerate(lines):
        if not re.fullmatch('[0-9a-fA-F]{8}: .*', line) or int(line[:8], 16) != len(result):
            raise ValueError('packet data offset')
        words = line[10:49].strip().split(' ')
        if any(not re.fullmatch('[0-9a-fA-F]{4}', word) for word in words[:-1]) or not re.fullmatch(
                '[0-9a-fA-F]{2}(?:[0-9a-fA-F]{2})?', words[-1]):
            raise ValueError('packet data hex')
        data = bytes.fromhex(''.join(words))
        if not 0 < len(data) <= 16 or index < len(lines) - 1 and len(data) != 16:
            raise ValueError('packet data row')
        result.extend(data)
    return bytes(result)


def avc_configuration(record):
    if len(record) < 7 or record[0] != 1 or record[4] & 0xFC != 0xFC or record[5] & 0xE0 != 0xE0:
        raise ValueError('avc configuration header')
    length = (record[4] & 3) + 1
    if length == 3:
        raise ValueError('avc length size')
    at = 6

    def units(count, kind):
        nonlocal at
        found = set()
        for _ in range(count):
            if at + 2 > len(record):
                raise ValueError('avc configuration length')
            size = int.from_bytes(record[at:at + 2], 'big')
            at += 2
            unit = record[at:at + size]
            if not size or len(unit) != size or unit[0] & 0x80 or unit[0] & 31 != kind:
                raise ValueError('avc configuration unit')
            found.add(unit)
            at += size
        return frozenset(found)

    sps = units(record[5] & 31, 7)
    if at >= len(record):
        raise ValueError('avc missing pps count')
    count = record[at]
    at += 1
    pps = units(count, 8)
    if not sps or not pps:
        raise ValueError('avc missing parameter sets')
    if at < len(record):
        if record[1] not in (100, 110, 122, 144) or at + 4 > len(record):
            raise ValueError('avc configuration extension')
        if record[at] & 0xFC != 0xFC or any(record[at + i] & 0xF8 != 0xF8 for i in (1, 2)):
            raise ValueError('avc extension header')
        count = record[at + 3]
        at += 4
        units(count, 13)
    if at != len(record):
        raise ValueError('avc configuration trailing data')
    return length, {7: sps, 8: pps}


def nal_units(sample, length):
    at, units = 0, []
    while at < len(sample):
        if at + length > len(sample):
            raise ValueError('avc truncated prefix')
        size = int.from_bytes(sample[at:at + length], 'big')
        at += length
        unit = sample[at:at + size]
        if not size or len(unit) != size or unit[0] & 0x80 or not unit[0] & 31:
            raise ValueError('avc sample unit')
        units.append(unit)
        at += size
    if not units:
        raise ValueError('avc empty sample')
    return units


def packet_data(path, ffprobe, env, inputs=()):
    code, data, err = invoke([ffprobe, '-v', 'error', *inputs, '-i', str(path),
        '-show_streams', '-show_packets', '-show_data', '-show_data_hash', 'sha256', '-of', 'json'], env)
    if code != 0 or err:
        return None
    try:
        document = json.loads(data)
        if not isinstance(document['streams'], list) or not isinstance(document['packets'], list):
            return None
        return document
    except (KeyError, ValueError, TypeError):
        return None


def avc1_carriage(mine, reference, ffmpeg, ffprobe, env, scratch, copied_streams, inputs=(),
                  registered_codecs=()):
    """Prove only the SPS/PPS carriage difference in ISO/IEC 14496-15:2017 5.2 Table 2.

    Seventh-edition FDIS w23225 5.2 Table 1 has the same avc1 prohibition.
    """
    try:
        documents = [packet_data(path, ffprobe, env, inputs) for path in (mine, reference)]
        if any(document is None for document in documents):
            return False
        actual, expected = documents
        if not actual['streams'] or len(actual['streams']) != len(expected['streams']):
            return False
        for document in documents:
            if [s['index'] for s in document['streams']] != list(range(len(document['streams']))):
                return False
        configurations = {}
        for index in copied_streams:
            ours, theirs = actual['streams'][index], expected['streams'][index]
            if ours['codec_name'] != theirs['codec_name'] or ours['codec_type'] != theirs['codec_type']:
                return False
            if theirs['codec_name'] != 'h264':
                continue
            if any(s['codec_type'] != 'video' or s['codec_tag_string'] != 'avc1' for s in (ours, theirs)):
                return False
            configs = [hexdump(s['extradata']) for s in (ours, theirs)]
            if configs[0] != configs[1] or any(len(config) != int(s['extradata_size']) or
                    s['extradata_hash'] != 'SHA256:' + hashlib.sha256(config).hexdigest()
                    for s, config in zip((ours, theirs), configs)):
                return False
            configurations[index] = avc_configuration(configs[0])
        if not configurations:
            return False
        rows, removed = [], 0
        for side, document in enumerate(documents):
            packets, seen = [], set()
            for packet in document['packets']:
                index = packet['stream_index']
                if type(index) is not int or not 0 <= index < len(document['streams']):
                    return False
                if index not in copied_streams:
                    continue
                seen.add(index)
                payload = hexdump(packet['data'])
                if not payload or len(payload) != int(packet['size']) or packet['data_hash'] != (
                        'SHA256:' + hashlib.sha256(payload).hexdigest()):
                    return False
                if index in configurations:
                    length, sets = configurations[index]
                    kept = bytearray()
                    for unit in nal_units(payload, length):
                        kind = unit[0] & 31
                        if kind in (13, 15):
                            return False
                        if kind in (7, 8):
                            if side == 0 or unit not in sets[kind]:
                                return False
                            removed += 1
                        else:
                            kept.extend(len(unit).to_bytes(length, 'big') + unit)
                    payload = bytes(kept)
                stream = document['streams'][index]
                if fractions.Fraction(stream['time_base']) <= 0:
                    return False
                clock = {**packet, 'time_base': stream['time_base']}
                timing = tuple(time(clock, key) for key in ('pts', 'dts', 'duration'))
                if any(t is None for t in timing) or timing[2] <= 0:
                    return False
                packets.append((index, payload, *timing))
            if seen != set(copied_streams):
                return False
            rows.append(packets)
        if not removed or rows[0] != rows[1]:
            return False
    except (KeyError, IndexError, ValueError, TypeError, AttributeError, ZeroDivisionError, OverflowError):
        return False
    verdict, _ = compare(mine, reference, ffmpeg, ffprobe, env, scratch, inputs=inputs,
                         copied_streams=copied_streams, registered_codecs=registered_codecs,
                         exact_frame_timeline=True)
    return verdict == 'verified'
