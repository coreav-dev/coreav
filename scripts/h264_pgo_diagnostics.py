#!/usr/bin/env python3
"""Prepare, execute and audit the registered diagnostic-only CLI comparisons."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import random
import re
import struct
import subprocess
import sys
import wave

sys.path.insert(0, str(Path(__file__).resolve().parent))
import h264_parity_bars as bars
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
import oracle_pin

ROOT = Path(__file__).resolve().parents[1]
PLAN = ROOT / 'scripts/h264_pgo_registration.json'
FILES = ('diagnostics/fixtures.json', 'diagnostics/signal.s16',
         'diagnostics/alac.m4a', 'diagnostics/flac.mka')
TOOLS = ('scripts/h264_pgo_diagnostics.py', 'scripts/h264_parity_bars.py',
         'scripts/oracle_pin.py', 'scripts/oracle_audio.py', 'scripts/oracle-audio-args.txt')
ARMS = ('base', 'pgo')
QUALIFY_ARMS = (*ARMS, 'independent')
MAXIMUM = 1 << 20
MAX_LOG = 16 << 20


class DiagnosticFailure(Exception):
    """A CLI operation or its complete output failed, not the shared experiment."""


def require(ok, message):
    if not ok:
        raise ValueError(message)


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def spec(plan):
    require(plan == json.loads(PLAN.read_text()), 'diagnostic registration changed')
    value = plan['diagnostics']
    require(value['gates_h264'] is False and value['expected_direction'] == 'neutral' and
            value['affinities'] == [1, 8] and [c['id'] for c in value['cases']] ==
            ['probe', 'mp4-remux', 'flac-decode'], 'invalid diagnostic registration')
    require([c['input'] for c in value['cases']] == [FILES[2], FILES[2], FILES[3]],
            'fixture paths differ from fixed inventory')
    return value


def pcm_chunks(fixture):
    """The registered LCG advances once per interleaved S16 channel sample."""
    require(fixture['generator'] == 'lcg32-high16-le-v1', 'unknown signal generator')
    state = fixture['seed']
    block = bytearray()
    for _ in range(fixture['sample_frames'] * fixture['channels']):
        state = (1664525 * state + 1013904223) & 0xffffffff
        block.extend(struct.pack('<H', state >> 16))
        if len(block) == 65536:
            yield bytes(block)
            block.clear()
    if block:
        yield bytes(block)


def signal_identity(fixture):
    value = hashlib.sha256()
    size = 0
    for block in pcm_chunks(fixture):
        value.update(block)
        size += len(block)
    return {'bytes': size, 'sha256': value.hexdigest()}


def identity(path, maximum=None):
    require(path.is_file() and not path.is_symlink(), 'expected a regular diagnostic file')
    size = path.stat().st_size
    require(size > 0 and (maximum is None or size <= maximum), 'diagnostic file exceeds budget or is empty')
    return {'bytes': size, 'sha256': digest(path)}


def manifest_header(plan, oracle):
    return {'schema': 1, 'plan_sha256': digest(PLAN), 'fixture': spec(plan)['fixture'],
            'tools': {name: digest(ROOT / name) for name in TOOLS},
            'oracle': {'sha256': digest(oracle), 'pin_sha256': digest(ROOT / oracle_pin.PIN)}}


def encode_command(oracle, root, fixture, codec):
    args = [str(oracle), '-nostdin', '-v', 'error', '-y', '-f', 's16le',
            '-ar', str(fixture['sample_rate']), '-ac', str(fixture['channels']),
            *AUDIO_DECODE_INPUT, '-i', str(root / FILES[1]), '-map', '0:a:0',
            '-c:a', codec, '-flags:a', '+bitexact', '-fflags', '+bitexact']
    return args + (['-sample_fmt', 's16p', '-movflags', '+faststart', '-f', 'ipod', str(root / FILES[2])]
                   if codec == 'alac' else ['-sample_fmt', 's16', '-compression_level', '5',
                                          '-f', 'matroska', str(root / FILES[3])])


def prepare(root, oracle, plan):
    """Resume only fixture bytes recorded by a matching preparation checkpoint."""
    import h264_pgo_round as round_driver
    round_driver.merged()
    round_driver.reservation(root, plan)
    fixture = spec(plan)['fixture']
    oracle = oracle.resolve()
    require(oracle == (root / 'ffmpeg').resolve(), 'oracle must be the bound root/ffmpeg')
    require(not list(oracle_pin.verify_bin(oracle.parent)), 'supplied oracle is not pinned')
    header = manifest_header(plan, oracle)
    path = root / FILES[0]
    require(not path.parent.is_symlink(), 'diagnostic directory must not be a symbolic link')
    if path.exists():
        identity(path, MAXIMUM)
        saved = json.loads(path.read_text())
        require(set(saved) == {*header, 'status', 'files'} and
                all(saved[k] == v for k, v in header.items()) and
                saved['status'] in ('preparing', 'complete') and set(saved['files']) <= set(FILES[1:]),
                'fixture checkpoint differs')
        for name, expected in saved['files'].items():
            require(identity(root / name) == expected, 'checkpoint fixture changed')
        if saved['status'] == 'complete':
            return verify(root, plan)
    else:
        require(not any((root / n).exists() for n in FILES[1:]), 'unrecorded diagnostic fixtures')
        saved = {**header, 'status': 'preparing', 'files': {}}
        path.parent.mkdir(parents=True, exist_ok=True)
    def checkpoint():
        temporary = path.with_suffix('.tmp')
        with temporary.open('w') as output:
            json.dump(saved, output, indent=2, allow_nan=False)
            output.flush()
            os.fsync(output.fileno())
        temporary.replace(path)
    checkpoint()
    if FILES[1] not in saved['files']:
        with (root / FILES[1]).open('wb') as output:
            for block in pcm_chunks(fixture):
                output.write(block)
        saved['files'][FILES[1]] = identity(root / FILES[1])
        checkpoint()
    require(saved['files'][FILES[1]] == signal_identity(fixture), 'signal differs from registered generator')
    for name, codec in zip(FILES[2:], ('alac', 'flac')):
        if name not in saved['files']:
            result = subprocess.run(encode_command(oracle, root, fixture, codec), check=True,
                                    capture_output=True, timeout=180)
            require(not result.stdout and not result.stderr, 'unexpected oracle encode diagnostics')
            saved['files'][name] = identity(root / name, saved['files'][FILES[1]]['bytes'] * 2 + MAXIMUM)
            checkpoint()
    saved['status'] = 'complete'
    checkpoint()
    return verify(root, plan)


def verify(root, plan):
    """Validate fixed fixture bytes without invoking codecs or parsing media."""
    header = manifest_header(plan, root / 'ffmpeg')
    require(not (root / 'diagnostics').is_symlink(), 'diagnostic directory must not be a symbolic link')
    identity(root / FILES[0], MAXIMUM)
    saved = json.loads((root / FILES[0]).read_text())
    require(set(saved) == {*header, 'status', 'files'} and saved['status'] == 'complete' and
            all(saved[k] == v for k, v in header.items()) and set(saved['files']) == set(FILES[1:]),
            'incomplete fixture manifest or changed specification/tools/oracle')
    require(saved['files'][FILES[1]] == signal_identity(header['fixture']), 'registered signal differs')
    maximum = saved['files'][FILES[1]]['bytes'] * 2 + MAXIMUM
    for name in FILES[1:]:
        require(identity(root / name, maximum) == saved['files'][name], 'diagnostic fixture changed')
    return saved


def command(root, case, arm, threads):
    output = root / 'diagnostics' / ('output.' + case['output'])
    replacements = {'input': str(root / case['input']), 'output': str(output), 'threads': str(threads)}
    return ['taskset', '-c', '2' if threads == 1 else '0-7', str(root / ('cli-' + arm)),
            *[arg.format(**replacements) for arg in case['argv']]]


def schedule(plan):
    config = spec(plan)
    rng = random.Random(plan['seed'])
    cells = []
    for threads in config['affinities']:
        for n in range(plan['rounds']):
            cases = list(config['cases'])
            rng.shuffle(cases)
            for case in cases:
                arms = list(ARMS)
                rng.shuffle(arms)
                cells.extend({'case': case['id'], 'arm': arm, 'threads': threads, 'round': n} for arm in arms)
    return cells


def roster(plan):
    cases = spec(plan)['cases']
    return ([{'kind': 'qualification', 'case': c['id'], 'arm': a, 'threads': t}
             for c in cases for t in (1, 8) for a in QUALIFY_ARMS] +
            [{'kind': 'warmup', 'case': c['id'], 'arm': a, 'threads': t}
             for c in cases for t in (1, 8) for a in ARMS] +
            [{'kind': 'timed', **cell} for cell in schedule(plan)])


@contextmanager
def strict_safe():
    previous = os.environ.get('COREAV_STRICT_SAFE')
    os.environ['COREAV_STRICT_SAFE'] = '1'
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop('COREAV_STRICT_SAFE', None)
        else:
            os.environ['COREAV_STRICT_SAFE'] = previous


def oracle_pcm(root, path):
    return [str(root / 'ffmpeg'), '-nostdin', '-v', 'error', *AUDIO_DECODE_INPUT, '-i', str(path),
            '-map', '0:a:0', '-c:a', 'pcm_s16le', '-f', 's16le', '-']


def wav_identity(path, fixture):
    try:
        with wave.open(str(path), 'rb') as audio:
            if (audio.getnchannels(), audio.getsampwidth(), audio.getframerate(), audio.getnframes(),
                    audio.getcomptype()) != (fixture['channels'], 2, fixture['sample_rate'], fixture['sample_frames'], 'NONE'):
                raise DiagnosticFailure('WAV sample geometry differs')
            value = hashlib.sha256()
            size = 0
            while block := audio.readframes(16384):
                size += len(block)
                value.update(block)
            return {'bytes': size, 'sha256': value.hexdigest()}
    except (wave.Error, EOFError) as error:
        raise DiagnosticFailure('invalid complete WAV output') from error


def run(root, plan, execute):
    """Shared-host and artifact validation remain the enclosing round's responsibility."""
    fixtures = verify(root, plan)
    require(digest(root / 'cli-pgo') == digest(root / 'cli-independent'), 'independent CLI rebuild differs')
    signal = fixtures['files'][FILES[1]]
    cases = {case['id']: case for case in spec(plan)['cases']}
    outputs = {}
    with (root / 'diagnostics.jsonl').open('x', buffering=1) as log:
        def emit(event):
            line = json.dumps(event, allow_nan=False) + '\n'
            require(log.tell() + len(line.encode()) <= MAX_LOG, 'diagnostic log exceeds budget')
            log.write(line)
        with strict_safe():
            for cell in roster(plan):
                case = cases[cell['case']]
                source = root / case['input']
                require(identity(source) == fixtures['files'][case['input']], 'diagnostic source changed')
                output = root / 'diagnostics' / ('output.' + case['output'])
                if case['output'] != 'stdout':
                    output.unlink(missing_ok=True)
                try:
                    try:
                        value = execute(command(root, case, cell['arm'], cell['threads']),
                                        maximum=MAXIMUM, timeout=180, capture=case['output'] == 'stdout')
                    except subprocess.CalledProcessError as error:
                        raise DiagnosticFailure('CLI exit or unexpected diagnostic') from error
                    require(bars.positive(value['wall']), 'invalid process-exit duration')
                    if case['output'] == 'stdout':
                        raw = value['stdout'].encode()
                        try:
                            valid = isinstance(json.loads(value['stdout']), dict)
                        except ValueError:
                            valid = False
                        if not valid or not raw:
                            raise DiagnosticFailure('probe did not produce a complete JSON object')
                        result = {'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}
                        require(result == {k: value[k] for k in ('bytes', 'sha256')}, 'capture hash contract differs')
                    else:
                        if not output.exists() or output.stat().st_size == 0:
                            raise DiagnosticFailure('CLI output is missing or empty')
                        result = identity(output, signal['bytes'] * 2 + MAXIMUM)
                    evidence = {'output': result}
                    if cell['kind'] == 'qualification':
                        if case['output'] == 'stdout':
                            evidence['probe_json'] = value['stdout']
                        else:
                            pcm = (execute(oracle_pcm(root, output), maximum=signal['bytes'], timeout=180)
                                   if case['output'] == 'mp4' else wav_identity(output, fixtures['fixture']))
                            samples = {k: pcm[k] for k in ('bytes', 'sha256')}
                            if samples != signal:
                                raise DiagnosticFailure('decoded samples or sample count differ from signal')
                            evidence['samples'] = samples
                        if case['id'] in outputs and outputs[case['id']] != result:
                            raise DiagnosticFailure('complete CLI arm outputs differ')
                        outputs[case['id']] = result
                    elif outputs[case['id']] != result:
                        raise DiagnosticFailure('CLI output changed after qualification')
                    emit({**cell, 'wall': value['wall'], **evidence})
                except DiagnosticFailure as error:
                    require(identity(source) == fixtures['files'][case['input']], 'diagnostic source changed during failed call')
                    emit({'kind': 'failed', 'cell': cell, 'reason': str(error)[:512]})
                    return 'failed'
                require(identity(source) == fixtures['files'][case['input']], 'diagnostic source changed during call')
            verify(root, plan)
            emit({'kind': 'complete'})
    return 'complete'


def valid_identity(value):
    return (isinstance(value, dict) and set(value) == {'bytes', 'sha256'} and
            isinstance(value['bytes'], int) and not isinstance(value['bytes'], bool) and value['bytes'] > 0 and
            isinstance(value['sha256'], str) and re.fullmatch('[a-f0-9]{64}', value['sha256']) is not None)


def audit(root, plan):
    fixtures = verify(root, plan)
    path = root / 'diagnostics.jsonl'
    require(path.stat().st_size <= MAX_LOG, 'diagnostic log exceeds budget')
    events = [json.loads(line) for line in path.read_text().splitlines()]
    require(events and events[-1].get('kind') in ('complete', 'failed'), 'missing diagnostic terminal event')
    terminal = events[-1]
    expected = roster(plan)
    require(len(events) - 1 <= len(expected), 'unexpected diagnostic calls')
    outputs = {}
    for got, want in zip(events[:-1], expected):
        extra = ({'probe_json'} if want['case'] == 'probe' else {'samples'}) if want['kind'] == 'qualification' else set()
        require(set(got) == {*want, 'wall', 'output', *extra} and
                all(type(got[k]) is type(v) and got[k] == v for k, v in want.items()) and bars.positive(got['wall']) and
                valid_identity(got['output']), 'invalid or out-of-order diagnostic cell')
        if want['kind'] == 'qualification':
            if want['case'] == 'probe':
                raw = got['probe_json'].encode()
                require(isinstance(json.loads(got['probe_json']), dict) and got['output'] ==
                        {'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}, 'invalid qualified probe')
            else:
                require(got['samples'] == fixtures['files'][FILES[1]], 'qualified sample identity differs')
            require(want['case'] not in outputs or outputs[want['case']] == got['output'],
                    'qualified CLI outputs differ')
            outputs[want['case']] = got['output']
        else:
            require(got['output'] == outputs[want['case']], 'post-qualification output differs')
    result = {'status': terminal['kind'], 'gates_h264': False, 'expected_direction': 'neutral',
              'ci_high_max': spec(plan)['ci_high_max'], 'commands': {}}
    if terminal['kind'] == 'failed':
        require(set(terminal) == {'kind', 'cell', 'reason'} and len(events) - 1 < len(expected) and
                isinstance(terminal['cell'], dict) and set(terminal['cell']) == set(expected[len(events) - 1]) and
                all(type(terminal['cell'][k]) is type(v) and terminal['cell'][k] == v
                    for k, v in expected[len(events) - 1].items()) and isinstance(terminal['reason'], str) and
                0 < len(terminal['reason']) <= 512, 'invalid diagnostic failure')
        result['commands'] = {case['id']: {str(t): {'guard': None, 'status': 'incomplete'} for t in (1, 8)}
                              for case in spec(plan)['cases']}
        return {**result, 'status': 'failed', 'failure': terminal, 'guards': None}
    require(terminal == {'kind': 'complete'} and len(events) - 1 == len(expected), 'incomplete diagnostic roster')
    for case in spec(plan)['cases']:
        result['commands'][case['id']] = {}
        for threads in (1, 8):
            times = {arm: {r['round']: r['wall'] for r in events[:-1] if r['kind'] == 'timed' and
                          r['case'] == case['id'] and r['threads'] == threads and r['arm'] == arm} for arm in ARMS}
            summary = bars.paired_command_summary(times, 'pgo', 'base', plan['rounds'],
                                                  plan['seed'], plan['bootstrap_replicates'])
            summary['guard'] = summary['ci95'][1] <= result['ci_high_max']
            result['commands'][case['id']][str(threads)] = summary
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('prepare', 'verify', 'audit'))
    parser.add_argument('directory', type=Path)
    parser.add_argument('--oracle', type=Path)
    args = parser.parse_args()
    plan = json.loads(PLAN.read_text())
    root = args.directory.resolve()
    if args.action == 'prepare':
        prepare(root, args.oracle or root / 'ffmpeg', plan)
        return 0
    if args.action == 'verify':
        verify(root, plan)
        return 0
    result = audit(root, plan)
    print(json.dumps(result, indent=2, allow_nan=False))
    return 0 if result['status'] == 'complete' else 1


if __name__ == '__main__':
    sys.exit(main())
