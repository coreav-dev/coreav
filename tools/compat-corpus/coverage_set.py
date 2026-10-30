#!/usr/bin/env python3
"""Derives the command set of the 0.1.0 encode breadth measure from
`commands-v1` (docs/design/compat-cli.md, "The 0.1.0 codec coverage bars").

    coverage_set.py fetch ANSWERS.json     reads the linked answers (network)
    coverage_set.py outputs ANSWERS.json   writes commands-v1/outputs.json
    coverage_set.py                        writes commands-0.1.0/coverage.json
    coverage_set.py --check                exits 1 when the committed file differs

Writing or checking the set also prints the ceiling estimate of
compat-cli.md's V2 ("The ceiling").

`commands-v1` replaces every output name with `OUTPUT`, and FFmpeg picks an
output's container from its name unless `-f` names one. So the output
containers come from the answers themselves: `fetch` asks the public Stack
Exchange API for every answer a usable `ffmpeg` command was found in, with
its body, and `outputs` runs `extract.py`'s own extraction over those bodies
again and keeps, for each command, the output of each invocation that
normalizes to it, reduced to what a run needs: `OUTPUT` with the name's
number pattern and extension (`OUTPUT%03d.jpg`), the pipe or null device it
names, or a URL's scheme (`rtmp://OUTPUT`). The
bodies are not committed; those templates are, in `commands-v1/outputs.json`.

The set is the usable `ffmpeg` commands with no filter option. Each one's
output is the first invocation's of its highest-scored answer, the order
`commands.jsonl` keeps its sources in, and its container is the one the
`-f` after its last input names (for `-f segment`, its `-segment_format`),
else the one that output's extension names. An output to a network address
or a device is not a media file, whatever `-f` names. The tables below class
every name that occurs; a name they do not class is an error, not a guess.
Python 3.11 standard library only.
"""
import argparse
import importlib.util
import json
import pathlib
import re
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
COMMANDS = HERE / 'commands-v1' / 'commands.jsonl'
OUTPUTS = HERE / 'commands-v1' / 'outputs.json'
OUT = HERE / 'commands-0.1.0' / 'coverage.json'


def load(name):
    spec = importlib.util.spec_from_file_location(name, HERE / f'{name}.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


classify = load('classify')

FILTERS = {'-vf', '-af', '-filter', '-filter_complex', '-lavfi',
           '-filter_script', '-filter_complex_script'}
VALUES = classify.FFMPEG_VALUE | set(classify.FFMPEG_CODEC) | {'-segment_format'}
SEGMENT_MUXERS = {'segment', 'ssegment'}
URL = re.compile(r'\A([A-Za-z][A-Za-z0-9+.-]*)://')

# The classes of an output. `media` outputs make the encode breadth set.
MEDIA, DECODE_ONLY, SUBTITLES, OTHER, UNKNOWN = (
    'media', 'decode-only', 'subtitles', 'other', 'unknown')

# The container each output extension names.
EXTENSIONS = {
    'mp4': 'MP4', 'm4a': 'MP4', 'm4v': 'MP4', 'm4b': 'MP4', '3gp': 'MP4',
    'ismv': 'MP4', 'mov': 'MOV', 'm3u8': 'HLS', 'mp3': 'MP3',
    'mkv': 'Matroska/WebM', 'mka': 'Matroska/WebM', 'webm': 'Matroska/WebM',
    'avi': 'AVI', 'flv': 'FLV', 'ts': 'MPEG-TS', 'm2ts': 'MPEG-TS',
    'mpg': 'MPEG-PS', 'mpeg': 'MPEG-PS', 'wmv': 'ASF', 'wma': 'ASF',
    'ogg': 'Ogg', 'ogv': 'Ogg', 'oga': 'Ogg', 'opus': 'Ogg',
    'wav': 'WAV', 'flac': 'FLAC', 'aac': 'ADTS',
    'jpg': 'image', 'jpeg': 'image', 'png': 'image', 'bmp': 'image', 'gif': 'GIF',
    'h264': 'raw', '265': 'raw', 'yuv': 'raw', 'y4m': 'YUV4MPEG',
}
# The container each muxer named by `-f` writes.
MUXERS = {
    'mp4': 'MP4', 'ipod': 'MP4', 'ismv': 'MP4', 'mov': 'MOV', 'hls': 'HLS',
    'mp3': 'MP3', 'matroska': 'Matroska/WebM', 'webm': 'Matroska/WebM',
    'avi': 'AVI', 'flv': 'FLV', 'mpegts': 'MPEG-TS', 'mpeg': 'MPEG-PS',
    'ogg': 'Ogg', 'opus': 'Ogg', 'wav': 'WAV', 'flac': 'FLAC', 'adts': 'ADTS',
    'image2': 'image', 'image2pipe': 'image', 'singlejpeg': 'image', 'gif': 'GIF',
    'rawvideo': 'raw', 'h264': 'raw', 'm4v': 'raw', 'mjpeg': 'raw',
    'mpeg1video': 'raw', 's16le': 'raw', 'u16le': 'raw', 'yuv4mpegpipe': 'YUV4MPEG',
}
# Outputs that are not media files.
NOT_MEDIA = {
    'null': DECODE_ONLY,
    'srt': SUBTITLES, 'ass': SUBTITLES, 'vtt': SUBTITLES, 'webvtt': SUBTITLES,
    'ffm': OTHER, 'ffmetadata': OTHER, 'md5': OTHER, 'framemd5': OTHER,
    'rtsp': OTHER, 'rtp': OTHER, 'tee': OTHER, 'sdl': OTHER, 'sdl2': OTHER,
}
# Names that say nothing about a container: no extension, a pipe, a word an
# answer used as a name, a muxer FFmpeg does not have.
NO_CONTAINER = {'', '66', 'media', 'file', 'nul'}

# The ceiling estimate: the encoders FFmpeg takes, by muxer or extension, for
# an output of a container CoreAV writes whose command names none (each input
# taken to hold video and audio), and the encoders each stage adds.
DEFAULT_ENCODERS = {
    'mp4': ('H.264', 'AAC'), 'm4v': ('H.264', 'AAC'), 'ismv': ('H.264', 'AAC'),
    'hls': ('H.264', 'AAC'), 'm3u8': ('H.264', 'AAC'), 'ipod': (None, 'AAC'),
    'm4a': (None, 'AAC'), 'm4b': (None, 'AAC'), '3gp': ('H.263', 'AMR'),
    'jpg': ('JPEG', None), 'jpeg': ('JPEG', None), 'png': ('PNG', None),
    'bmp': ('BMP', None), 'singlejpeg': ('JPEG', None),
}
ENCODERS = {
    'copy': 'copy', 'libx264': 'H.264', 'libx264rgb': 'H.264', 'h264': 'H.264',
    'aac': 'AAC', 'libfdk_aac': 'AAC', 'libfaac': 'AAC', 'libvo_aacenc': 'AAC',
    'mjpeg': 'JPEG', 'png': 'PNG',
}
STAGES = (
    ('copies', {'MP4', 'HLS', 'copy'}),
    ('H.264 through x264', {'H.264'}),
    ('AAC', {'AAC'}),
    ('JPEG and PNG images', {'image', 'JPEG', 'PNG'}),
)


def filter_free(args):
    options, _ = classify.walk(args, VALUES)
    return not any(o.split(':', 1)[0] in FILTERS for o, _ in options)


def template(name):
    """The output name reduced to what a run needs: a pipe or a null device
    as it is, a URL as its scheme, another device as `/dev/OUTPUT`, a file as
    `OUTPUT` with its number pattern and extension."""
    if name == '-' or name.startswith('pipe:') or name == '/dev/null':
        return name
    if name.upper() == 'NUL':
        return 'NUL'
    url = URL.match(name)
    if url:
        return url.group(1).lower() + '://OUTPUT'
    if name.startswith('/dev/'):
        return '/dev/OUTPUT'
    if name.startswith('file:'):
        name = name[len('file:'):]
    tail = name.rsplit('/', 1)[-1].rsplit('\\', 1)[-1]
    numbered = classify.NUMBERED.search(tail)
    extension = classify.extension(name)
    return 'OUTPUT' + (numbered.group(0) if numbered else '') + ('.' + extension if extension else '')


def container_name(args, output):
    """How the output's container is named: (`-f`, MUXER) or (`.`, EXT)."""
    options, _ = classify.walk(args, VALUES)
    last_input = max((n for n, (o, _) in enumerate(options) if o == '-i'), default=-1)
    after = options[last_input + 1:]
    muxers = [v for o, v in after if o == '-f']
    formats = [v for o, v in after if o == '-segment_format']
    if muxers and muxers[-1] in SEGMENT_MUXERS and formats:
        return '-f', formats[-1]
    if muxers and muxers[-1] not in SEGMENT_MUXERS:
        return '-f', muxers[-1]
    return '.', classify.extension(output)


def classed(kind, name):
    table = MUXERS if kind == '-f' else EXTENSIONS
    if name in table:
        return MEDIA, table[name]
    if name in NOT_MEDIA:
        return NOT_MEDIA[name], None
    if name in NO_CONTAINER:
        return UNKNOWN, None
    raise SystemExit(f'coverage_set: {kind}{name} is in no table; class it')


def derive(records, outputs):
    by_args = {tuple(o['args']): o['outputs'] for o in outputs['commands']}
    usable = [r for r in records if r['tool'] == 'ffmpeg' and r['label'] != 'unusable']
    commands = []
    for record in usable:
        if not filter_free(record['args']):
            continue
        found = by_args[tuple(record['args'])]
        output = found[0] if found else 'OUTPUT'
        kind, name = container_name(record['args'], output)
        if URL.match(output) or output.startswith('/dev/') or output == 'NUL':
            cls, container = (DECODE_ONLY if (kind, name) == ('-f', 'null') else OTHER), None
        else:
            cls, container = classed(kind, name)
        commands.append({'args': record['args'], 'output': output,
                         'muxer': name if kind == '-f' else None,
                         'class': cls, 'container': container})
    classes = {}
    for command in commands:
        classes[command['class']] = classes.get(command['class'], 0) + 1
    return {
        'outputs_read': outputs['read'],
        'usable': len(usable),
        'filter_free': len(commands),
        'classes': dict(sorted(classes.items())),
        'commands': commands,
    }


def needs(command):
    """The container and the encoders a media command's output takes; `?`
    for a stream whose encoder the tables do not know."""
    options, _ = classify.walk(command['args'], VALUES)
    muxer = command['muxer']
    name = muxer if muxer not in (None, 'image2') else classify.extension(command['output'])
    video, audio = DEFAULT_ENCODERS.get(name, ('?', '?'))
    for option, value in options:
        kind, _, stream = option.partition(':')
        if option == '-vn':
            video = None
        if option == '-an':
            audio = None
        named = kind in ('-c', '-codec') and value
        if video and (named and stream[:1] in ('', 'v') or kind == '-vcodec'):
            video = ENCODERS.get(value, value)
        if audio and (named and stream[:1] in ('', 'a') or kind == '-acodec'):
            audio = ENCODERS.get(value, value)
    return {command['container'], video, audio} - {None}


def ceilings(document):
    """The media commands each stage of STAGES could count, cumulatively."""
    media = [needs(c) for c in document['commands'] if c['class'] == MEDIA]
    have, counts = set(), []
    for _, added in STAGES:
        have |= added
        counts.append(sum(n <= have for n in media))
    return counts


def dumps(document):
    """The document with one command a line, so that a change to one
    command is one line of the diff."""
    head = {k: v for k, v in document.items() if k != 'commands'}
    lines = [json.dumps(c, sort_keys=True) for c in document['commands']]
    return (json.dumps(head, indent=1, sort_keys=True)[:-2] + ',\n "commands": [\n  '
            + ',\n  '.join(lines) + '\n ]\n}\n')


def fetch(path):
    collect = load('collect')
    records = [json.loads(line) for line in COMMANDS.read_text(encoding='utf-8').splitlines()]
    ids = sorted({s['answer'] for r in records
                  if r['tool'] == 'ffmpeg' and r['label'] != 'unusable' for s in r['sources']})
    items = []
    for start in range(0, len(ids), collect.PAGE):
        batch = ';'.join(str(i) for i in ids[start:start + collect.PAGE])
        items.extend(collect.get(f'answers/{batch}', pagesize=collect.PAGE,
                                 filter=collect.FILTER_ANSWERS)['items'])
    path.write_text(json.dumps({'read': time.strftime('%Y-%m-%d', time.gmtime()),
                                'items': items}), encoding='utf-8')
    print(f'{len(items)} of {len(ids)} answers -> {path}')


def recover(answers):
    """Every usable ffmpeg command, with the output template of each
    invocation of its own answers that normalizes to it."""
    extract = load('extract')
    bodies = {a['answer_id']: a.get('body', '') for a in answers['items']}
    records = [json.loads(line) for line in COMMANDS.read_text(encoding='utf-8').splitlines()]
    commands = []
    for record in records:
        if record['tool'] != 'ffmpeg' or record['label'] == 'unusable':
            continue
        found = []
        for source in record['sources']:
            for block in extract.code_blocks(bodies.get(source['answer'], '')):
                for line in extract.joined_lines(block):
                    for tool, text in extract.candidates(line):
                        args = extract.arguments(text) if tool == 'ffmpeg' else None
                        if args is None or classify.normalize(tool, args) != record['args']:
                            continue
                        found.append(next((template(args[n]) for n, token in
                                           enumerate(record['args']) if token == 'OUTPUT'), ''))
        commands.append({'args': record['args'], 'outputs': found})
    return {'read': answers['read'], 'commands': commands}


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('step', nargs='?', choices=('fetch', 'outputs'))
    parser.add_argument('answers', nargs='?', type=pathlib.Path)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    if args.step == 'fetch':
        fetch(args.answers)
        return 0
    if args.step == 'outputs':
        recovered = recover(json.loads(args.answers.read_text(encoding='utf-8')))
        OUTPUTS.write_text(dumps(recovered), encoding='utf-8')
        print(f'{len(recovered["commands"])} commands -> {OUTPUTS}')
        return 0
    records = [json.loads(line) for line in COMMANDS.read_text(encoding='utf-8').splitlines()]
    derived = derive(records, json.loads(OUTPUTS.read_text(encoding='utf-8')))
    text = dumps(derived)
    media = derived['classes'][MEDIA]
    for (stage, _), count in zip(STAGES, ceilings(derived)):
        print(f'ceiling, + {stage}: {count} of {media} ({100 * count / media:.1f}%)')
    if args.check:
        if OUT.read_text(encoding='utf-8') != text:
            print(f'{OUT} differs from what coverage_set.py derives', file=sys.stderr)
            return 1
        return 0
    OUT.write_text(text, encoding='utf-8')
    print(f'{derived["filter_free"]} of {derived["usable"]} usable commands -> {OUT}')
    print('  ' + ', '.join(f'{k} {v}' for k, v in derived['classes'].items()))
    return 0


if __name__ == '__main__':
    sys.exit(main())
