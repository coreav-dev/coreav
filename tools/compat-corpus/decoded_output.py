"""Exact decoded-output comparisons for current fixture CI, not frozen rounds."""
import fractions
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
    code, data, err = invoke([ffprobe, '-v', 'error', *inputs, '-i', str(path),
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


def compare(mine, reference, ffmpeg, ffprobe, env, scratch, inputs=()):
    expected = probe(reference, ffprobe, env, inputs)
    if expected is None:
        return 'unverified', 'reference probe'
    for stream in expected['streams']:
        codec = stream.get('codec_name', '')
        if codec not in LOSSLESS | {'mp3'} and not PCM.fullmatch(codec):
            return 'unverified', f'unregistered codec {codec}'
        if stream.get('codec_type') == 'audio' and stream.get('sample_fmt') not in (
                'u8', 'u8p', 's16', 's16p', 's32', 's32p', 'flt', 'fltp', 'dbl', 'dblp'):
            return 'unverified', 'reference sample format'
        if stream.get('codec_type') == 'video':
            frames = [f for f in expected.get('frames', []) if f.get('stream_index') == stream.get('index')]
            if any(f.get('pix_fmt') != stream.get('pix_fmt') for f in frames):
                return 'unverified', 'changing reference pixel format'
    actual = probe(mine, ffprobe, env, inputs)
    if actual is None:
        return 'failed', 'candidate probe'
    if len(actual['streams']) != len(expected['streams']):
        return 'failed', 'stream count'
    for index, (ours, theirs) in enumerate(zip(actual['streams'], expected['streams'])):
        if not metadata(ours, theirs):
            return 'failed', 'stream fields or timing'
        kind = theirs['codec_type']
        if kind == 'audio':
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
