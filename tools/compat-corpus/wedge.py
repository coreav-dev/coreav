#!/usr/bin/env python3
"""Derives `wedge-v1`, the command set CoreAV 0.1.0 runs natively, from the
native commands of `commands-v1` (docs/design/compat-cli.md, "The 0.1.0
wedge").

    wedge.py            writes tools/compat-corpus/wedge-v1/wedge.json
    wedge.py --check    exits 1 when the committed file differs

Every native command is reduced to its **shape**: what it asks the tool to
do, with what does not change the output left out (the log level, `-y`,
option order, `-i` against a bare input, the time values, the stream
indexes, the `-show_entries` keys). Shapes are ranked by how many
command-and-answer pairs of the set use them, and the wedge is the
smallest prefix of that ranking that reaches COVERAGE of the eligible
native pairs, taking whole ties: a shape used as often as the last one taken
is taken too, so that no tie is broken by the order of the shapes' names.
A native command the command refuses by design (one that asks for no
section, an interval that needs a seek) is not eligible. Python 3.11
standard library only.
"""
import argparse
import collections
import importlib.util
import json
import pathlib
import re
import sys

HERE = pathlib.Path(__file__).resolve().parent
COMMANDS = HERE / 'commands-v1' / 'commands.jsonl'
OUT = HERE / 'wedge-v1' / 'wedge.json'
SPEC = importlib.util.spec_from_file_location('classify', HERE / 'classify.py')
classify = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(classify)

COVERAGE = 0.80

# The input containers each entry is measured on. ffprobe reads all three;
# the copy jobs read what the CoreAV job behind them reads today (job_of).
CONTAINERS = ('mp4-mov', 'matroska-webm', 'mpegts')
# A demuxer forced with `-f` before the input: the command means what it
# means only on files of that family, so it runs on those alone.
FORCED = {'mov,mp4,m4a,3gp,3g2,mj2': 'mp4-mov', 'mp4': 'mp4-mov', 'mov': 'mp4-mov',
          'm4a': 'mp4-mov', '3gp': 'mp4-mov', 'matroska,webm': 'matroska-webm',
          'matroska': 'matroska-webm', 'webm': 'matroska-webm', 'mpegts': 'mpegts'}

# Options that do not change what the command writes, or whose value the
# runner picks (a file, a segment length), so they make no shape.
FFPROBE_IGNORED = {'-v', '-loglevel', '-hide_banner', '-nostdin', '-i'}
FFMPEG_IGNORED = {'-y', '-n', '-hide_banner', '-nostdin', '-loglevel', '-v',
                  '-threads', '-max_muxing_queue_size', '-safe',
                  '-ignore_unknown', '-segment_time', '-segment_format',
                  '-hls_time', '-i'}

SECTION_FLAGS = {'-show_format': 'format', '-show_streams': 'streams',
                 '-show_chapters': 'chapters', '-show_programs': 'programs',
                 '-show_packets': 'packets', '-show_error': 'error'}
SECTION_KEYS = {'format': 'format', 'streams': 'stream', 'chapters': 'chapter',
                'programs': 'program', 'packets': 'packet', 'error': 'error'}
# The intervals the command reads (compat-cli.md, "How the values are
# derived"): one from the start of the file, with an end.
READ_INTERVAL = re.compile(r'\A%(\+#\d+|\+?[0-9:.]+)\Z')


def ffprobe_shape(args):
    """(shape, keys, refusal, constraints): the sections asked for, the writer and the
    stream selection; the keys the command prints; why the command refuses
    it by design, or None; and what limits the inputs the command runs on (a
    forced demuxer, `input_container`). A writer's settings and the selected stream's
    type and index make no shape."""
    options, _ = classify.walk(args, classify.FFPROBE_VALUE)
    parts, keys, refusal, constraints = set(), set(), None, {}
    for option, value in options:
        if option in SECTION_FLAGS:
            parts.add(SECTION_FLAGS[option])
        elif option == '-show_entries':
            for section in (value or '').split(':'):
                name, _, fields = section.partition('=')
                parts.add(f'entries:{name}')
                keys.update(f'{name}.{f}' for f in fields.split(',') if f)
                if not fields:
                    keys.add(f'{name}.*')
        elif option in ('-of', '-print_format', '-output_format'):
            parts.add(f'writer:{(value or "").partition("=")[0]}')
        elif option == '-select_streams':
            parts.add('select_streams')
        elif option == '-f':
            constraints['input_container'] = FORCED[value]
        elif option == '-read_intervals':
            parts.add('read_intervals')
            if not READ_INTERVAL.match(value or ''):
                refusal = f'-read_intervals {value}'
        elif option in FFPROBE_IGNORED:
            continue
        else:
            parts.add(option.lstrip('-'))
    sections = {p for p in parts if p in SECTION_FLAGS.values() or p.startswith('entries:')}
    if not sections:
        refusal = refusal or 'no section: only the summary on standard error'
    keys.update(f'{SECTION_KEYS[s]}.*' for s in sections if s in SECTION_KEYS)
    if not any(p.startswith('writer:') for p in parts):
        parts.add('writer:default')
    return tuple(sorted(parts)), keys, refusal, constraints


TYPES = {'v', 'a', 's'}


def selected(maps, drops):
    """The stream types an output gets: FFmpeg's automatic selection (a
    video, an audio and a subtitle stream) without `-map`, else the types
    the selectors add less the ones they remove, then less `-vn`, `-an` and
    `-sn`. A selector by index or by metadata may take any type."""
    if not maps:
        return TYPES - drops
    types = set()
    for value in maps:
        kind = (value.lstrip('-').split(':') + [''])[1]
        named = {kind} if kind in TYPES else None
        if value.startswith('-'):
            types -= named or set()
        else:
            types |= named or TYPES
    return types - drops


def ffmpeg_shape(args):
    """(shape, keys, refusal, constraints) as for ffprobe: the job, the seek and
    its position, the streams the output keeps and copies, and the stream
    selection, muxer, timestamp and metadata options as one feature each;
    the times and the selectors make no shape. `input_without` is the stream
    types the command selects but does not copy: FFmpeg would encode them,
    so the command is a copy only on an input that has none."""
    options, _ = classify.walk(args, classify.FFMPEG_VALUE | set(classify.FFMPEG_CODEC))
    parts, copied, drops, maps, seen_input, constraints = set(), set(), set(), [], False, {}
    job = 'copy'
    for option, value in options:
        codec = classify.codec_of(option)
        if codec:
            spec = option.split(':', 1)[1] if ':' in option else None
            kind = {'-vcodec': 'v', '-acodec': 'a', '-scodec': 's'}.get(codec, spec)
            copied.add(kind[:1] if kind else 'all')
            continue
        if option == '-i':
            seen_input = True
            continue
        if option == '-f':
            if value in ('concat', 'segment', 'hls'):
                job = value
            elif not seen_input:
                constraints['input_container'] = FORCED[value]
            continue
        if option == '-ss':
            parts.add('seek:output' if seen_input else 'seek:input')
        elif option in ('-t', '-to'):
            parts.add('end')
        elif option == '-sseof':
            parts.add('seek:end')
        elif option == '-map':
            parts.add('map')
            maps.append(value or '')
        elif option in ('-vn', '-an', '-sn', '-dn'):
            drops.add(option[1])
        elif option == '-movflags':
            parts.add('movflags:' + ('faststart' if value == '+faststart' else 'fragmented'))
        elif option in ('-fflags', '-avoid_negative_ts', '-reset_timestamps', '-copyts', '-start_at_zero'):
            parts.add('timestamps')
        elif option in ('-map_metadata', '-map_chapters'):
            parts.add('metadata')
        elif option in FFMPEG_IGNORED:
            continue
        else:
            parts.add(option.lstrip('-'))
    parts.add(f'job:{job}')
    chosen = selected(maps, drops)
    kept = chosen if 'all' in copied else chosen & copied
    parts.add('streams:' + ('audio' if kept == {'a'} else 'video' if kept == {'v'} else 'all'))
    if 'all' not in copied and chosen - copied:
        constraints['input_without'] = sorted(chosen - copied)
    return tuple(sorted(parts)), set(), None, constraints


def job_of(tool, shape):
    """The CoreAV job behind a shape, and the containers it reads today:
    `remux` reads MP4/MOV and Matroska/WebM, the other copy jobs MP4/MOV
    only, and no copy job reads a transport stream."""
    if tool == 'ffprobe':
        if any(p in ('packets', 'entries:packet', 'count_packets') for p in shape):
            return 'probe-packets', CONTAINERS
        return 'probe', CONTAINERS
    parts = dict(p.split(':', 1) for p in shape if ':' in p)
    if parts['job'] == 'concat':
        return 'join', ('mp4-mov',)
    if parts['job'] in ('segment', 'hls'):
        return {'segment': 'split', 'hls': 'hls'}[parts['job']], ('mp4-mov',)
    if 'end' in shape or any(p.startswith('seek:') for p in shape):
        return 'clip', ('mp4-mov',)
    if parts['streams'] == 'audio':
        return 'extract-audio', ('mp4-mov',)
    return 'remux', ('mp4-mov', 'matroska-webm')


def derive(records):
    native = [r for r in records if r['label'] == 'native']
    groups = collections.OrderedDict()
    refused = []
    for record in native:
        shape_of = ffprobe_shape if record['tool'] == 'ffprobe' else ffmpeg_shape
        shape, keys, refusal, constraints = shape_of(record['args'])
        if refusal:
            refused.append({'tool': record['tool'], 'args': record['args'],
                            'pairs': len(record['sources']), 'refused': refusal})
            continue
        group = groups.setdefault((record['tool'], shape), {
            'tool': record['tool'], 'shape': list(shape), 'pairs': 0,
            'commands': [], 'keys': set()})
        group['pairs'] += len(record['sources'])
        group['commands'].append({'args': record['args'], 'pairs': len(record['sources']),
                                  **constraints})
        group['keys'] |= keys
    ranked = sorted(groups.values(), key=lambda g: (-g['pairs'], -len(g['commands']),
                                                    g['tool'], g['shape']))
    native_pairs = sum(len(r['sources']) for r in native)
    eligible_pairs = sum(g['pairs'] for g in ranked)
    wedge, covered = [], 0
    for group in ranked:
        if covered >= COVERAGE * eligible_pairs and group['pairs'] < wedge[-1]['pairs']:
            break
        covered += group['pairs']
        wedge.append(group)
    entries = []
    for n, group in enumerate(wedge, 1):
        job, containers = job_of(group['tool'], group['shape'])
        entries.append({
            'id': f'W{n:02d}', 'tool': group['tool'], 'job': job,
            'containers': list(containers), 'shape': group['shape'],
            'pairs': group['pairs'], 'keys': sorted(group['keys']),
            'commands': sorted(group['commands'], key=lambda c: (-c['pairs'], len(c['args']), c['args'])),
        })
    tail = ranked[len(wedge):]
    return {
        'set': 'commands-v1',
        'native': {'commands': len(native), 'pairs': native_pairs},
        'eligible': {'commands': len(native) - len(refused), 'pairs': eligible_pairs,
                     'shapes': len(ranked)},
        'wedge': {'shapes': len(wedge),
                  'commands': sum(len(e['commands']) for e in entries),
                  'pairs': covered,
                  'share_of_eligible_pairs': round(covered / eligible_pairs, 4),
                  'share_of_native_pairs': round(covered / native_pairs, 4)},
        'entries': entries,
        'refused_by_design': refused,
        'not_in_wedge': [{'tool': g['tool'], 'shape': g['shape'], 'pairs': g['pairs'],
                          'commands': len(g['commands'])} for g in tail],
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--check', action='store_true', help='compare with the committed file')
    a = ap.parse_args()
    records = [json.loads(line) for line in COMMANDS.read_text(encoding='utf-8').splitlines()]
    text = json.dumps(derive(records), indent=1, sort_keys=True) + '\n'
    if a.check:
        if not OUT.exists() or OUT.read_text(encoding='utf-8') != text:
            sys.exit(f'wedge: {OUT.relative_to(HERE.parents[1])} is not what wedge.py derives; run it')
        return
    OUT.parent.mkdir(exist_ok=True)
    OUT.write_text(text, encoding='utf-8')


if __name__ == '__main__':
    main()
