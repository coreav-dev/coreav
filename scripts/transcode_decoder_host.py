#!/usr/bin/env python3
"""Capture and replay the diagnostic in docs/design/transcode-decoder-host.md.

Run from a clean merged checkout on the registered host. Build there first:
  python3 scripts/transcode_decoder_host.py build --receipt build.json
  python3 scripts/transcode_decoder_host.py run --files DIR --gate FILE \
    --tools DIR --build build.json --instance instance.json --out DIR --tmp /dev/shm/decoder-host
  python3 scripts/transcode_decoder_host.py summarize --out DIR

The VM launcher must enforce a hard lifetime as well as this runner's deadline.
No media or timing command is allowed before the plan and tool merge.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_pin
import test_oracle
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
import transcode_check as tc
import transcode_speed as speed

ROOT = Path(__file__).resolve().parent.parent
INPUTS = ROOT / 'bench/transcode-decoder-host/inputs.json'
SWITCH = 'COREAV_DIAG_TRANSCODE_IN_PROCESS'
ARMS = ('base', 'base2', 'sandbox_ffmpeg', 'sandbox_x264', 'process_ffmpeg', 'process_x264')
BUILD = ['cargo', 'build', '--locked', '-p', 'coreav-cli', '--release', '--features',
         'native-h264,dav1d,libvpx,ffmpeg-fallback']
ROUNDS = 3
DEFAULT_RUN_SECONDS = 5 * 3600
MAX_RUN_SECONDS = 6 * 3600


class HostFault(ValueError):
    pass


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def run_allowance(value):
    seconds = int(value)
    require(0 < seconds <= MAX_RUN_SECONDS, 'run deadline must be at most six hours')
    return seconds


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def save(path, value):
    path = Path(path)
    temporary = path.with_suffix('.partial')
    temporary.write_text(json.dumps(value, indent=2, allow_nan=False) + '\n')
    temporary.replace(path)


def text(command, timeout=60):
    return subprocess.run([str(x) for x in command], check=True, capture_output=True,
                          text=True, timeout=timeout).stdout.strip()


def source():
    commit = text(['git', '-C', ROOT, 'rev-parse', 'HEAD'])
    subprocess.run(['git', '-C', ROOT, 'merge-base', '--is-ancestor', commit, 'origin/main'],
                   check=True, capture_output=True, timeout=60)
    require(not text(['git', '-C', ROOT, 'status', '--porcelain', '--untracked-files=no']),
            'the source checkout has tracked changes')
    return commit


def build(args):
    commit = source()
    overrides = [key for key in os.environ if key.startswith(('CARGO_PROFILE_', 'CARGO_BUILD_', 'CARGO_TARGET_'))
                 or key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER',
                            'RUSTC_WORKSPACE_WRAPPER', 'RUSTC', 'RUSTC_BOOTSTRAP')]
    require(not overrides, 'remove compiler and profile environment overrides')
    external = [directory / '.cargo' for directory in ROOT.parents]
    external.append(Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo')))
    for cargo in external:
        require(not any((cargo / name).exists() for name in ('config', 'config.toml')),
                'remove external Cargo configuration before building')
    for name in ('config', 'config.toml'):
        config = ROOT / '.cargo' / name
        if config.exists():
            subprocess.run(['git', '-C', ROOT, 'ls-files', '--error-unmatch', str(config)],
                           check=True, capture_output=True, timeout=60)
    subprocess.run(BUILD, cwd=ROOT, check=True, timeout=3600)
    require(source() == commit, 'source changed during the build')
    save(args.receipt, {'commit': commit, 'command': BUILD,
                        'coreav_sha256': digest(ROOT / 'target/release/coreav'),
                        'rustc': text(['rustc', '-Vv'])})


def roster():
    registered = json.loads(INPUTS.read_text())
    historical = [json.loads(line) for line in
                  (ROOT / 'docs/measurements/transcode/round-2/speed/files.jsonl').read_text().splitlines()]
    expected = [row for cls in ('vp9', 'av1') for row in historical
                if row['class'] == cls and row['status'] == 'timed']
    require([(f['class'], f['sha256'], f['ext'], f['maps']) for f in registered['files']] ==
            [(f['class'], f['sha256'], f['ext'], f['maps']) for f in expected],
            'the registered roster differs from round two')
    return registered


def validate_instance(instance):
    require(instance['machine_type'] == 'c4-standard-8' and instance['provisioning'] == 'STANDARD' and
            instance['advanced_machine_features']['threadsPerCore'] == 1,
            'requires a standard-provisioned c4-standard-8 with SMT off')


def host(tmp, instance):
    validate_instance(instance)
    request = urllib.request.Request(
        'http://metadata.google.internal/computeMetadata/v1/instance/machine-type',
        headers={'Metadata-Flavor': 'Google'})
    with urllib.request.urlopen(request, timeout=5) as response:
        machine = response.read(4096).decode().strip().split('/')[-1]
    cpus = text(['lscpu'])
    smt = Path('/sys/devices/system/cpu/smt/active').read_text().strip()
    release = Path('/etc/os-release').read_text()
    require(machine == 'c4-standard-8' and smt == '0' and len(os.sched_getaffinity(0)) == 4,
            'requires c4-standard-8 with SMT off and four available CPUs')
    require('8581C' in cpus and 'VERSION_ID="13"' in release and 'ID=debian' in release,
            'requires the registered CPU and Debian release')
    require(text(['findmnt', '-n', '-o', 'FSTYPE', '-T', tmp]) == 'tmpfs', 'output must be on tmpfs')
    libraries = text(['dpkg-query', '-W', '-f=${Package} ${Version}\n', 'libvpx9', 'libdav1d7'])
    require('libvpx9 1.15.0' in libraries and 'libdav1d7 1.5.1' in libraries,
            'requires the registered decoder library versions')
    return {'instance': instance, 'machine_type': machine, 'smt_active': smt, 'lscpu': cpus, 'os_release': release,
            'affinity': sorted(os.sched_getaffinity(0)), 'libraries': libraries,
            'kernel': platform.release(), 'memory': Path('/proc/meminfo').read_text().splitlines()[:3]}


def command(arm, file, path, output, tools, coreav):
    if arm in ('base', 'base2'):
        return speed.baseline_cmd(tools['ffmpeg'], path, output,
                                  file['maps']['video'], file['maps']['audio'])
    encoder = arm.split('_')[1]
    return [coreav, 'transcode', path, output, '--encoder', encoder,
            '--ffmpeg', tools['ffmpeg'], '--x264', tools['x264'],
            '--preset', 'medium', '--crf', '23', '--timeout', '900', '--force']


def environment(arm):
    env = {key: value for key, value in os.environ.items()
           if key in ('PATH', 'HOME', 'LANG', 'LC_ALL', 'TMPDIR')}
    if arm not in ('base', 'base2', 'gate'):
        env[SWITCH] = '1' if arm.startswith('process_') else '0'
    return env


class Executor:
    def __init__(self, seconds):
        self.deadline = time.monotonic() + seconds

    def call(self, command, arm):
        left = self.deadline - time.monotonic()
        require(left > 0, 'the run deadline expired')
        start = time.perf_counter()
        result = subprocess.run([str(x) for x in command], env=environment(arm),
                                check=True, capture_output=True, timeout=min(960, left))
        return time.perf_counter() - start, result


def receipt(result, file, arm):
    document = json.loads(result.stdout)
    video = document['video']
    require(video['stream_index'] == file['maps']['video'] and
            [audio['stream_index'] for audio in document['audio']] == file['maps']['audio'],
            'stream maps changed')
    isolation = 'in-process' if arm.startswith('process_') else 'sandbox'
    require(f'coreav-diagnostic-decoder-host: {isolation}' in result.stderr.decode(),
            'missing actual decoder-isolation receipt')
    info = {key: video[key] for key in ('backend', 'encoder', 'frames', 'width', 'height')}
    info['isolation'] = isolation
    validate_work(info, file, arm)
    return info


def validate_work(info, file, arm):
    require(info['backend'] == ('libvpx' if file['class'] == 'vp9' else 'dav1d') and
            info['encoder'] == arm.split('_')[1] and
            info['isolation'] == ('in-process' if arm.startswith('process_') else 'sandbox'),
            'wrong decoder backend, isolation or encoder')
    require(all(type(info[k]) is int and info[k] > 0 for k in ('frames', 'width', 'height')),
            'no encoded frames or invalid geometry')


def invocation(arm, file):
    return [str(value) for value in command(arm, file, Path('INPUT'), Path('OUTPUT'),
                                            {name: Path('TOOLS') / name for name in ('ffmpeg', 'ffprobe', 'x264')},
                                            Path('COREAV'))]


def output_check(executor, tools, output):
    _, frames = executor.call([tools['ffmpeg'], '-v', 'error', '-nostdin', *AUDIO_DECODE_INPUT, '-i', output,
                                '-map', '0:v:0', '-fps_mode', 'passthrough',
                                '-f', 'framemd5', '-'], 'base')
    _, packets = executor.call([tools['ffprobe'], '-v', 'error', '-select_streams', 'a',
                                 '-show_packets', '-show_entries',
                                 'packet=stream_index,pts,dts,duration,size,data_hash',
                                 '-show_data_hash', 'sha256', '-of', 'json', output], 'base')
    return frames.stdout.decode(), json.loads(packets.stdout)['packets']


def preflight(executor, file, path, tools, coreav, tmp, checks):
    speed.prime(path)
    result = {}
    for encoder in ('ffmpeg', 'x264'):
        paired = []
        for mode in ('sandbox', 'process'):
            arm = mode + '_' + encoder
            output = tmp / 'preflight.mp4'
            try:
                _, process = executor.call(command(arm, file, path, output, tools, coreav), arm)
                info = receipt(process, file, arm)
                frames, audio = output_check(executor, tools, output)
                require(sum(bool(line and not line.startswith('#')) for line in frames.splitlines()) ==
                        info['frames'], 'independent decoded frame count differs')
                paired.append((info, frames, audio))
            finally:
                output.unlink(missing_ok=True)
        require(paired[0][0] | {'isolation': 'in-process'} == paired[1][0] and
                paired[0][1:] == paired[1][1:], 'hosting pair differs in frames or audio packets')
        name = file['sha256'] + '-' + encoder + '.framemd5'
        (checks / name).write_text(paired[0][1])
        result[encoder] = {'sandbox': paired[0][0], 'process': paired[1][0],
                           'framemd5_sha256': digest(checks / name), 'audio_packets': paired[0][2]}
    return result


def gate(executor, tools, path, tmp):
    _, probe = executor.call([tools['ffprobe'], '-v', 'error', '-show_streams',
                              '-of', 'json', path], 'gate')
    video = tc.default_video(json.loads(probe.stdout).get('streams', []))
    require(video is not None, 'the gate has no video')
    cmd = speed.baseline_cmd(tools['ffmpeg'], path, tmp / 'gate.mp4', video['index'], [])
    cmd[-2:-2] = ['-an']
    speed.prime(path)
    try:
        executor.call(cmd, 'gate')
        return [executor.call(cmd, 'gate')[0] for _ in range(3)]
    finally:
        (tmp / 'gate.mp4').unlink(missing_ok=True)


def order(index, round_number):
    return speed.rotation(list(ARMS), (index + round_number) % len(ARMS))


def stats(values):
    median, low, high = tc.bootstrap(values, resamples=10000, seed=518)
    return {'median': median, 'ci95': [low, high], 'files': len(values)}


def ratio_stats(values):
    return stats(values) | {'below_one': sum(value < 1 for value in values),
                            'share_below_one': sum(value < 1 for value in values) / len(values)}


def summarize(report, out):
    registered = roster()
    require(report['inputs'] == registered and report['inputs_sha256'] == digest(INPUTS),
            'changed input registration')
    require(report['status'] == 'complete', 'incomplete capture')
    validate_instance(report['host']['instance'])
    require(report['build']['command'] == BUILD, 'changed build configuration')
    require(len(report['gates']) == 2 and all(len(g) == 3 for g in report['gates']), 'missing gate calls')
    for group in report['gates']:
        require(all(type(t) in (int, float) and math.isfinite(t) and t > 0 for t in group), 'invalid gate time')
    drift = statistics.median(report['gates'][1]) / statistics.median(report['gates'][0]) - 1
    if abs(drift) > 0.05:
        raise HostFault('void host gate')
    expected = [(f['sha256'], r, arm) for i, f in enumerate(registered['files'])
                for r in range(ROUNDS) for arm in order(i, r)]
    require([(c['sha256'], c['round'], c['arm']) for c in report['calls']] == expected,
            'missing, duplicate, reordered or unregistered calls')
    files = []
    for file in registered['files']:
        sha = file['sha256']
        require(sha in report['preflight'], 'missing complete preflight')
        check = report['preflight'][sha]
        for encoder in ('ffmpeg', 'x264'):
            frame_path = out / 'checks' / (sha + '-' + encoder + '.framemd5')
            require(digest(frame_path) ==
                    check[encoder]['framemd5_sha256'], 'changed preflight frame hashes')
            for mode in ('sandbox', 'process'):
                validate_work(check[encoder][mode], file, mode + '_' + encoder)
            require(sum(bool(line and not line.startswith('#')) for line in frame_path.read_text().splitlines()) ==
                    check[encoder]['sandbox']['frames'], 'changed independent frame count')
            require(check[encoder]['sandbox'] | {'isolation': 'in-process'} == check[encoder]['process'],
                    'changed preflight pair')
        calls = [c for c in report['calls'] if c['sha256'] == sha]
        for call in calls:
            require(call['invocation'] == invocation(call['arm'], file), 'changed timed invocation')
            require(type(call['seconds']) in (int, float) and math.isfinite(call['seconds']) and
                    call['seconds'] > 0, 'invalid wall time')
            if call['arm'] not in ('base', 'base2'):
                mode, encoder = call['arm'].split('_')
                require(call['receipt'] == check[encoder][mode], 'timed command changed work')
        medians = {arm: statistics.median(c['seconds'] for c in calls if c['arm'] == arm) for arm in ARMS}
        files.append({'class': file['class'], 'sha256': sha, 'seconds': medians})
    summary = {'diagnostic_only': True, 'gate_drift': drift, 'classes': {}, 'files': files}
    for cls in ('vp9', 'av1'):
        rows = [f['seconds'] for f in files if f['class'] == cls]
        table = {arm: {'seconds': stats([r[arm] for r in rows]),
                       'over_baseline': ratio_stats([r[arm] / r['base'] for r in rows])} for arm in ARMS}
        pairs = {}
        for encoder in ('ffmpeg', 'x264'):
            sandbox, process = 'sandbox_' + encoder, 'process_' + encoder
            fractions = [(r[sandbox] - r[process]) / (r[sandbox] - r['base'])
                         for r in rows if r[sandbox] > r['base']]
            pairs[encoder] = {'process_over_sandbox': ratio_stats([r[process] / r[sandbox] for r in rows]),
                              'seconds_saved': stats([r[sandbox] - r[process] for r in rows]),
                              'fraction_extra_seconds_removed': stats(fractions),
                              'fraction_undefined_files': len(rows) - len(fractions)}
        summary['classes'][cls] = {'arms': table, 'pairs': pairs}
    return summary


def run(args):
    out, tmp = Path(args.out).resolve(), Path(args.tmp).resolve()
    out.mkdir(parents=True, exist_ok=True)
    require(not (out / 'capture.json').exists(), 'capture already exists; do not repeat completed work')
    require(not tmp.exists(), 'use a new dedicated tmpfs directory')
    tmp.mkdir(parents=True)
    checks = out / 'checks'
    checks.mkdir(exist_ok=True)
    report = {'status': 'incomplete', 'calls': [], 'gates': [], 'preflight': {}}
    stage = 'admission'
    try:
        commit = source()
        tools, coreav = Path(args.tools).resolve(), ROOT / 'target/release/coreav'
        build_receipt = json.loads(Path(args.build).read_text())
        require(build_receipt['commit'] == commit and build_receipt['command'] == BUILD and
                build_receipt['coreav_sha256'] == digest(coreav), 'build receipt does not match source and executable')
        try:
            oracle = test_oracle.require(tools / 'ffmpeg', tools / 'ffprobe')
        except test_oracle.QualificationError:
            raise ValueError('oracle pin verification failed') from None
        tools = {'ffmpeg': oracle.ffmpeg, 'ffprobe': oracle.ffprobe, 'x264': tools / 'x264'}
        registered = roster()
        report.update(inputs=registered, inputs_sha256=digest(INPUTS), build=build_receipt,
                      host=host(tmp, json.loads(Path(args.instance).read_text())),
                      tools={name: digest(tools[name]) for name in ('ffmpeg', 'ffprobe', 'x264')},
                      versions={name: text([tools[name], '-version' if name != 'x264' else '--version'])
                                for name in ('ffmpeg', 'ffprobe', 'x264')})
        paths = {f['sha256']: Path(args.files).resolve() / (f['sha256'] + '.' + f['ext'])
                 for f in registered['files']}
        for file in registered['files']:
            path = paths[file['sha256']]
            require(path.stat().st_size == file['size'] and digest(path) == file['sha256'], 'input hash differs')
        require(digest(args.gate) == registered['gate_sha256'], 'wrong gate input')
        executor = Executor(run_allowance(args.max_seconds))
        stage = 'preflight'
        for file in registered['files']:
            report['preflight'][file['sha256']] = preflight(
                executor, file, paths[file['sha256']], tools, coreav, tmp, checks)
            save(out / 'capture.json', report)
        stage = 'gate-before'
        report['gates'].append(gate(executor, tools, Path(args.gate).resolve(), tmp))
        save(out / 'capture.json', report)
        stage = 'timing'
        for index, file in enumerate(registered['files']):
            path = paths[file['sha256']]
            speed.prime(path)
            for round_number in range(ROUNDS):
                for arm in order(index, round_number):
                    output = tmp / 'timed.mp4'
                    try:
                        elapsed, result = executor.call(command(arm, file, path, output, tools, coreav), arm)
                        row = {'sha256': file['sha256'], 'round': round_number, 'arm': arm, 'seconds': elapsed,
                               'invocation': invocation(arm, file)}
                        if arm not in ('base', 'base2'):
                            row['receipt'] = receipt(result, file, arm)
                        report['calls'].append(row)
                        save(out / 'capture.json', report)
                    finally:
                        output.unlink(missing_ok=True)
        stage = 'gate-after'
        report['gates'].append(gate(executor, tools, Path(args.gate).resolve(), tmp))
        report['status'] = 'complete'
        stage = 'replay'
        summary = summarize(report, out)
        save(out / 'summary.json', summary)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        report['status'] = 'void' if isinstance(error, HostFault) else 'incomplete'
        report['failure'] = {'stage': stage, 'kind': type(error).__name__}
        raise
    finally:
        save(out / 'capture.json', report)


def smoke(args):
    tools = Path(args.tools).resolve()
    try:
        oracle = test_oracle.require(tools / 'ffmpeg', tools / 'ffprobe')
    except test_oracle.QualificationError:
        raise ValueError('oracle pin verification failed') from None
    tools = {'ffmpeg': oracle.ffmpeg, 'ffprobe': oracle.ffprobe, 'x264': tools / 'x264'}
    tmp = Path(args.tmp).resolve()
    tmp.mkdir(parents=True, exist_ok=False)
    executor = Executor(300)
    for cls, encoder in (('vp9', 'libvpx-vp9'), ('av1', 'libaom-av1')):
        path = tmp / (cls + '.mkv')
        executor.call([tools['ffmpeg'], '-v', 'error', '-nostdin', '-f', 'lavfi', '-i',
                       'testsrc2=size=64x64:rate=5:duration=1', '-c:v', encoder,
                       '-threads', '1', '-y', path], 'base')
        file = {'class': cls, 'sha256': digest(path), 'maps': {'video': 0, 'audio': []}}
        preflight(executor, file, path, tools, ROOT / 'target/release/coreav', tmp, tmp)
    print('synthetic decoder-host preflight passed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='action', required=True)
    builder = commands.add_parser('build')
    builder.add_argument('--receipt', required=True)
    capture = commands.add_parser('run')
    for flag in ('files', 'gate', 'tools', 'build', 'instance', 'out', 'tmp'):
        capture.add_argument('--' + flag, required=True)
    capture.add_argument('--max-seconds', type=run_allowance, default=DEFAULT_RUN_SECONDS)
    pilot = commands.add_parser('smoke')
    pilot.add_argument('--tools', required=True)
    pilot.add_argument('--tmp', required=True)
    replay = commands.add_parser('summarize')
    replay.add_argument('--out', required=True)
    args = parser.parse_args()
    if args.action == 'build':
        build(args)
    elif args.action == 'run':
        run(args)
    elif args.action == 'smoke':
        smoke(args)
    else:
        out = Path(args.out)
        save(out / 'summary.json', summarize(json.loads((out / 'capture.json').read_text()), out))


if __name__ == '__main__':
    main()
