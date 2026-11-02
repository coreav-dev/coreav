"""Capture and replay the registered generated-input VP9 filter predicate."""
import argparse
import collections
from fractions import Fraction
import hashlib
import itertools
import json
import os
import pathlib
import re
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import vp9_corpus as corpus
import vp9_filter_frames as frames
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT

ROOT = corpus.ROOT
PLAN = ROOT / 'docs/measurements/vp9-filter-coverage/plan.json'
SOURCES = ('scripts/vp9_filter_coverage.py', 'scripts/vp9_filter_frames.py',
           'scripts/vp9_corpus.py', 'scripts/oracle_audio.py', 'scripts/oracle-audio-args.txt',
           'scripts/oracle_pin.py', 'scripts/oracle-ffmpeg.env',
           'docs/measurements/vp9-filter-coverage/plan.json')
HEX = re.compile('[0-9a-f]{64}')


def registration():
    plan = corpus.read_json(PLAN)
    corpus.require(type(plan['schema']) is int and plan['schema'] == 1, 'unknown filter plan')
    corpus.require(plan['maximum_failures'] == 0 and plan['shown_frames_per_stream'] == 1,
                   'filter comparisons require all streams and exactly one frame')
    corpus.require(set(plan['variants']) == {'base', 'empty', 'sharp', 'plus', 'minus', 'absolute'},
                   'incomplete filter variants')
    corpus.require(plan['contrasts'] == dict(sharpness=['empty', 'sharp'],
                   delta_positive=['empty', 'plus'], delta_negative=['empty', 'minus'],
                   absolute=['empty', 'absolute'], absolute_vs_delta=['plus', 'absolute']),
                   'incomplete filter contrasts')
    corpus.require(type(plan['minimum_effects_per_family']) is int
                   and plan['minimum_effects_per_family'] >= 1, 'empty sensitivity requirement')
    return plan


def grid(plan, mode):
    corpus.require(mode in ('smoke', 'measure'), 'unknown coverage mode')
    settings = plan if mode == 'measure' else plan['smoke']
    return settings['width'], settings['height'], settings['patterns'], settings['levels']


def roster(plan, mode):
    _, _, patterns, levels = grid(plan, mode)
    return list(itertools.product(patterns, levels, plan['variants']))


def hashes():
    return {path: corpus.digest(ROOT / path) for path in SOURCES}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def validate(report):
    plan = registration()
    mode = report['mode']
    width, height, _, _ = grid(plan, mode)
    corpus.require(type(report.get('schema')) is int and report['schema'] == 1 and report.get('sources') == hashes(),
                   'stale filter plan or tool sources')
    corpus.require(isinstance(report.get('commit'), str) and re.fullmatch('[0-9a-f]{40}', report['commit']),
                   'missing measured commit')
    corpus.require(set(report.get('tools', {})) == {'native', 'ffmpeg', 'ffprobe'}
                   and all(isinstance(v, str) and HEX.fullmatch(v) for v in report['tools'].values()),
                   'missing tool identities')
    corpus.require(report.get('complete') is True and not report.get('error'), 'aborted filter capture')
    rows = report.get('streams')
    corpus.require(isinstance(rows, list)
                   and [(r.get('pattern'), r.get('level'), r.get('variant')) for r in rows] == roster(plan, mode),
                   'missing, duplicate or reordered filter stream')
    checked = {}
    entropy = {}
    with tempfile.TemporaryDirectory(prefix='vp9-filter-replay-') as temporary:
        capture = pathlib.Path(temporary) / 'capture.jsonl'
        for row in rows:
            pattern, level, variant = row['pattern'], row['level'], row['variant']
            corpus.require(type(level) is int, 'filter level is not an integer')
            expected = dict(level=level, deltas=False, **plan['variants'][variant])
            corpus.require(row.get('control') == expected
                           and all(type(row['control'][k]) is type(v) for k, v in expected.items()),
                           'captured filter controls differ')
            for key in ('input_sha256', 'packet_sha256', 'entropy_sha256', 'encoded_sha256', 'raw_sha256'):
                corpus.require(isinstance(row.get(key), str) and HEX.fullmatch(row[key]), 'invalid input identity')
            corpus.require(row['raw_sha256'] == sha(frames.picture(pattern, width, height)),
                           'generated picture differs from the recipe')
            identity = row['encoded_sha256'], row['entropy_sha256']
            corpus.require(entropy.setdefault(pattern, identity) == identity, 'entropy changed between controls')
            data = [row['meta'], row['native'], row['footer']]
            capture.write_text(''.join(json.dumps(value) + '\n' for value in data))
            meta, footer, mine = corpus.native_capture(capture, 'first120', row['input_sha256'])
            corpus.require((footer['fed'], footer['packets'], footer['frames'], footer['cutoff'])
                           == (1, 1, 1, None), 'generated stream is not one complete packet and frame')
            corpus.require((mine[0]['width'], mine[0]['height'], mine[0]['pts']) == (width, height, 0),
                           'generated frame dimensions or timestamp differ')
            corpus.require(footer['coverage'] == dict(coded=1, sharpness=int(expected['sharpness'] != 0),
                           segment_filter=int(expected['segmentation'] and expected['alt_l'] is not None)),
                           'native filter observations differ from the generated controls')
            clock = Fraction(*row['oracle_clock'])
            corpus.require(type(row['oracle'].get('pts')) is int and type(row['oracle'].get('size')) is int,
                           'oracle frame fields are not integers')
            corpus.require(corpus.judge(meta, footer, mine, clock, [row['oracle']]) == 1,
                           'incomplete generated comparison')
            checked[pattern, level, variant] = (meta, footer, mine, clock, row['oracle'])
    effects = collections.Counter({name: 0 for name in plan['contrasts']})
    _, _, patterns, levels = grid(plan, mode)
    for pattern, level in itertools.product(patterns, levels):
        for identity in plan['identities']:
            if level in identity['levels']:
                left = checked[pattern, level, identity['left']][2][0]
                right = checked[pattern, level, identity['right']][2][0]
                corpus.require(left['md5'] == right['md5'], 'a no-effect control changed pixels')
        for family, (left_name, right_name) in plan['contrasts'].items():
            left, right = checked[pattern, level, left_name], checked[pattern, level, right_name]
            corpus.require(left[3] == right[3], 'control and variant clocks differ')
            if left[2][0]['md5'] == right[2][0]['md5']:
                continue
            for native, oracle in ((left, right), (right, left)):
                try:
                    corpus.judge(native[0], native[1], native[2], oracle[3], [oracle[4]])
                except ValueError:
                    continue
                raise ValueError('a crossed negative control passed the comparator')
            effects[family] += 1
    if mode == 'measure':
        corpus.require(all(n >= plan['minimum_effects_per_family'] for n in effects.values()),
                       'a registered filter contrast has no pixel-affecting witness')
    return dict(mode=mode, streams=len(rows), effects=dict(effects), coverage_pass=True if mode == 'measure' else None)


def capture(args):
    corpus.require(sys.platform == 'linux', 'filter capture requires the registered Linux host')
    corpus.require(os.uname().machine == 'x86_64', 'filter capture requires x86-64')
    if args.mode == 'measure':
        corpus.require(os.environ.get('GITHUB_ACTIONS') == 'true'
                       and os.environ.get('GITHUB_REF') == 'refs/heads/main'
                       and os.environ.get('GITHUB_EVENT_NAME') == 'workflow_dispatch',
                       'judged filter inputs run only from the merged main workflow')
    plan = registration()
    width, height, patterns, levels = grid(plan, args.mode)
    report = dict(schema=1, mode=args.mode, sources=hashes(), complete=False, streams=[])
    try:
        with tempfile.TemporaryDirectory(prefix='vp9-filter-capture-') as temporary:
            work = pathlib.Path(temporary)
            sequence = 0
            def command(argv):
                nonlocal sequence
                sequence += 1
                out, errors = corpus.child(argv, work, f'command-{sequence}', plan['command_seconds'])
                corpus.require(errors.stat().st_size == 0, 'fixture tool reported an error')
                return out.read_bytes()
            command([sys.executable, ROOT / 'scripts/oracle_pin.py', '--verify-bin', pathlib.Path(args.ffmpeg).parent])
            report['commit'] = command(['git', '-C', ROOT, 'rev-parse', 'HEAD']).decode().strip()
            corpus.require(not command(['git', '-C', ROOT, 'status', '--porcelain', '--untracked-files=no']),
                           'measurement source checkout is dirty')
            if args.mode == 'measure':
                corpus.require(report['commit'] == os.environ.get('GITHUB_SHA'), 'measured revision changed')
            report['tools'] = {name: corpus.digest(getattr(args, name)) for name in ('native', 'ffmpeg', 'ffprobe')}
            encoded = {}
            for pattern in patterns:
                raw = work / f'{pattern}.yuv'
                raw.write_bytes(frames.picture(pattern, width, height))
                outputs = []
                for repeat in range(2):
                    target = work / f'{pattern}-{repeat}.ivf'
                    command([args.ffmpeg, '-nostdin', '-v', 'error', '-xerror', '-f', 'rawvideo',
                             '-pixel_format', 'yuv420p', '-video_size', f'{width}x{height}', '-framerate', '1',
                             *AUDIO_DECODE_INPUT, '-i', raw, '-an', *plan['encoder'], '-f', 'ivf', target])
                    outputs.append(target.read_bytes())
                corpus.require(outputs[0] == outputs[1], 'black-box encoding is not reproducible')
                packet = frames.ivf(outputs[0])
                template = frames.keyframe(packet)
                corpus.require(not template.control['segmentation'], 'source encoder added segmentation')
                corpus.require((template.width, template.height) == (width, height), 'source dimensions differ')
                encoded[pattern] = template, sha(outputs[0]), corpus.digest(raw)
            private = work / 'captures'
            private.mkdir(mode=0o700)
            for pattern, level, variant in roster(plan, args.mode):
                template, encoded_sha, raw_sha = encoded[pattern]
                packet = frames.rewrite(template, level, plan['variants'][variant])
                stem = f'{pattern}-{level}-{variant}'
                ivf_path, mkv_path, roundtrip = (work / f'{stem}.{suffix}' for suffix in ('ivf', 'mkv', 'roundtrip.ivf'))
                ivf_path.write_bytes(frames.wrap(packet))
                command([args.ffmpeg, '-nostdin', '-v', 'error', '-xerror', *AUDIO_DECODE_INPUT, '-i', ivf_path,
                         '-map', '0:v:0', '-c:v', 'copy', '-an', '-f', 'matroska', mkv_path])
                command([args.ffmpeg, '-nostdin', '-v', 'error', '-xerror', *AUDIO_DECODE_INPUT, '-i', mkv_path,
                         '-map', '0:v:0', '-c:v', 'copy', '-an', '-f', 'ivf', roundtrip])
                corpus.require(frames.ivf(roundtrip.read_bytes()) == packet, 'packet-copy round trip changed bytes')
                input_sha = corpus.digest(mkv_path)
                result = corpus.compare(dict(path=str(mkv_path), sha256=input_sha), args, set(), private)
                corpus.require(result['status'] == 'agree' and result['frames'] == 1, 'generated comparison failed')
                directory = private / input_sha
                meta, footer, mine = corpus.native_capture(directory / 'native.stdout', 'first120', input_sha)
                clock, oracle = corpus.oracle_capture(directory / 'oracle.stdout')
                corpus.require(len(mine) == len(oracle) == 1, 'generated comparison has extra frames')
                report['streams'].append(dict(pattern=pattern, level=level, variant=variant,
                    control=frames.keyframe(packet).control, input_sha256=input_sha, packet_sha256=sha(packet),
                    entropy_sha256=sha(template.entropy), encoded_sha256=encoded_sha, raw_sha256=raw_sha,
                    meta=meta, footer=footer, native=mine[0], oracle_clock=[clock.numerator, clock.denominator],
                    oracle=oracle[0]))
            corpus.require(report['sources'] == hashes(), 'tool sources changed during capture')
            corpus.require(report['tools'] == {name: corpus.digest(getattr(args, name))
                           for name in ('native', 'ffmpeg', 'ffprobe')}, 'tool bytes changed during capture')
            report['complete'] = True
            report['judgment'] = validate(report)
    except Exception as error:
        report['complete'] = False
        report['error'] = type(error).__name__
        raise
    finally:
        corpus.write_json(args.out, report)
    print(json.dumps(report['judgment'], sort_keys=True))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('smoke', 'measure', 'replay'))
    parser.add_argument('--out', required=True)
    for name in ('native', 'ffmpeg', 'ffprobe'):
        parser.add_argument(f'--{name}')
    args = parser.parse_args()
    try:
        if args.mode == 'replay':
            print(json.dumps(validate(corpus.read_json(args.out)), sort_keys=True))
            return 0
        corpus.require(all(getattr(args, k) for k in ('native', 'ffmpeg', 'ffprobe')), 'missing executable path')
        return capture(args)
    except Exception as error:
        print(f'vp9-filter: {args.mode} failed ({type(error).__name__})', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
