"""Deterministic representatives for GitHub Shapes 200, without execution."""
import re
from dataclasses import dataclass

VALUES = {'-ac': '1', '-threads': '1', '-ar': '48000', '-b:a': '128k', '-b:v': '200k',
          '-vb': '200k', '-ss': '0.2', '-t': '1', '-to': '1', '-r': '10', '-framerate': '10',
          '-g': '10', '-frames:v': '3', '-s': '64x48', '-crf': '23', '-q': '2', '-q:v': '2',
          '-safe': '0', '-loop': '0', '-hls_time': '1', '-hls_list_size': '0', '-start_number': '0',
          '-strict': '-2', '-level': '3.0', '-auto-alt-ref': '0', '-lossless': '0', '-quality': 'good',
          '-bsf:a': 'aac_adtstoasc', '-bsf:v': 'h264_mp4toannexb'}
FILTERS = {'scale': 'scale=32:24', 'crop': 'crop=32:24:0:0', 'fps': 'fps=5',
           'setpts': 'setpts=PTS-STARTPTS', 'volume': 'volume=0.5',
           'silencedetect': 'silencedetect=noise=-30dB:d=0.1', 'loudnorm': 'loudnorm',
           'palettegen': 'palettegen', 'pad': 'pad=64:48:0:0', 'setsar': 'setsar=1'}
INPUTS = {'unknown': 'source.mp4', 'isobmff': 'source.mp4', 'matroska': 'source.mkv',
          'wav': 'source.wav', 'mp3': 'source.mp3', 'image-png': 'source.png',
          'image-jpeg': 'source.jpg', 'image-gif': 'source.gif',
          'image-png-sequence': 'frame-%02d.png', 'list': 'concat.txt'}
RAW_INPUTS = {'s16le': 'source.s16', 'rawvideo': 'source.bgr'}
EXTENSIONS = {'mp4': 'mp4', 'mp3': 'mp3', 'wav': 'wav', 'ogg': 'ogg', 'flv': 'flv',
              'mpegts': 'ts', 'hls': 'm3u8', 'gif': 'gif', 'rawvideo': 'raw',
              's16le': 's16', 'f32le': 'f32', 'null': 'null', 'image2pipe': 'bin'}
FLAGS = {'-an', '-vn', '-shortest', '-re', '-xerror', '-count_frames', '-sexagesimal',
         '-show_streams', '-show_format'}


class Uninstantiable(ValueError):
    pass


@dataclass(frozen=True)
class Command:
    tool: str
    args: tuple
    sources: tuple
    outputs: tuple
    output_options: tuple
    stdin: str | None = None


def options(items):
    args = []
    for key, value in items:
        if key in ('-wrapper-unresolved', '-filter_complex', '-lavfi', '-format'):
            raise Uninstantiable('lost_construction')
        if value is None:
            if key not in FLAGS:
                raise Uninstantiable('missing_value')
            args.append(key)
            continue
        if key in ('-vf', '-af'):
            try:
                value = ','.join(FILTERS[f] for f in value.split(','))
            except KeyError:
                raise Uninstantiable('lost_filter') from None
        elif key in ('-map', '-select_streams') and '<' not in value:
            value = value.replace('N', '0')
        elif key == '-of':
            parts = value.split('=', 1)
            if len(parts) == 2:
                settings = []
                for setting in parts[1].split(':'):
                    name, marker = setting.split('=', 1)
                    if marker != '<value>' or name not in ('nokey', 'nk', 'noprint_wrappers', 'nw', 'p', 's'):
                        raise Uninstantiable('lost_writer')
                    settings.append(name + '=' + (',' if name == 's' else '1'))
                value = parts[0] + '=' + ':'.join(settings)
        elif value == '<value>' and key in VALUES:
            value = VALUES[key]
        if '<' in value or '>' in value:
            raise Uninstantiable('lost_value')
        args.extend((key, value))
    return args


def instantiate(shape):
    args = options(shape['options'])
    sources, outputs, output_options, stdin = [], [], [], None
    for index, group in enumerate(shape['inputs']):
        opts = options(group['options'])
        form = dict(group['options']).get('-f')
        container = group['container']
        if form in ('lavfi', 'v4l2') or container in ('device', 'network-stream', 'hls'):
            raise Uninstantiable('external_input')
        if form in RAW_INPUTS:
            name = RAW_INPUTS[form]
        elif form == 'concat':
            name = 'concat.txt'
        else:
            name = INPUTS.get(container if container != 'pipe' else 'unknown')
        if name is None:
            raise Uninstantiable('input_container')
        source = f'in{index}/{name}'
        sources.append((source, name))
        if container == 'pipe':
            if stdin is not None or '%' in name or name == 'concat.txt':
                raise Uninstantiable('pipe_topology')
            stdin = source
            source = 'pipe:0'
        args.extend((*opts, '-i', source))
    for index, group in enumerate(shape['outputs']):
        opts = options(group['options'])
        output_options.append(tuple(opts))
        args.extend(opts)
        form = dict(group['options']).get('-f')
        container, hint = group['container'], group['container_hint']
        if container in ('device', 'network-stream'):
            raise Uninstantiable('external_output')
        if container == 'pipe':
            if '-' in outputs:
                raise Uninstantiable('pipe_topology')
            name = '-'
        else:
            extension = EXTENSIONS.get(form, hint if hint != 'unknown' else 'mp4')
            sequence = '-%02d' if container.endswith('-sequence') else ''
            name = f'output{index}{sequence}.{extension}'
        outputs.append(name)
        args.append(name)
    if shape['tool'] not in ('ffmpeg', 'ffprobe') or not sources:
        raise Uninstantiable('tool_or_input')
    return Command(shape['tool'], tuple(args), tuple(sources), tuple(outputs), tuple(output_options), stdin)


def output_files(folder, name):
    if name == '-':
        return [folder / 'stdout'] if (folder / 'stdout').is_file() else []
    if '%' not in name:
        return [folder / name] if (folder / name).is_file() else []
    pattern = re.compile(re.escape(name).replace('%02d', r'([0-9]+)'))
    found = []
    indices = set()
    for path in folder.iterdir():
        match = pattern.fullmatch(path.name)
        if match:
            index = int(match[1])
            if index in indices:
                raise ValueError('duplicate_image_index')
            indices.add(index)
            found.append((index, path))
    return [path for _, path in sorted(found)]
