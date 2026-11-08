import argparse
import json
import os
import pathlib
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import hevc_reference as ref
from filter_ceiling import count

parser = argparse.ArgumentParser()
for arm in ('main', 'candidate', 'control'):
    parser.add_argument('--' + arm, type=pathlib.Path, required=True)
parser.add_argument('--main-sha', required=True)
parser.add_argument('--candidate-sha', required=True)
parser.add_argument('--oracle-bin', type=pathlib.Path, required=True)
parser.add_argument('--out', type=pathlib.Path, required=True)
parser.add_argument('--only', required=True)
args = parser.parse_args()
args.only = set(args.only.split(','))
args.frames = 16
os.sched_setaffinity(0, {min(os.sched_getaffinity(0))})
oracle = ref.test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
args.out.mkdir(parents=True, exist_ok=True)
pool = ref.round_files.pool()
pool.limits.max_requests = 4000
pool.limits.max_bytes = 32 << 30
binaries = {arm: getattr(args, arm) for arm in ('main', 'candidate', 'control')}
report = dict(main=args.main_sha, candidate=args.candidate_sha, frames_cap=16, repeats=7,
    control='Only the two new deblocking dispatch conditions gain an opaque false; interpolation and SAO SIMD retained',
    scope='Whole-process single-thread Annex B decode-to-null including startup and teardown; reference only',
    cpu=next(line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),
    affinity=sorted(os.sched_getaffinity(0)), load=os.getloadavg(), complete=False, files=[],
    binaries={arm: ref.conformance.digest(binary) for arm, binary in binaries.items()},
    oracle=ref.oracle_pin.load()['BUILD'])


def save():
    report['r2_requests'] = pool.limits.requests
    (args.out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')


try:
    with tempfile.TemporaryDirectory(prefix='hevc-deblock-') as temporary:
        directory = pathlib.Path(temporary)
        for identity, source, original_sha in ref.inputs(args, oracle, directory, pool):
            row = dict(id=identity, source_sha256=original_sha,
                       input_sha256=ref.conformance.digest(source), calls=[], complete=False)
            report['files'].append(row)
            save()
            expected = ref.verify(args.main, oracle, source)
            for arm in ('candidate', 'control'):
                if ref.verify(binaries[arm], oracle, source) != expected:
                    raise ValueError('arm_output_mismatch')
            row.update(expected)
            commands = {arm: [binary, source, 'decode'] for arm, binary in binaries.items()}
            commands['ffmpeg'] = ref.ffmpeg_command(oracle.ffmpeg, source, True,
                ['-progress', 'pipe:1', '-f', 'null', '-'])
            work = None
            for arm, command in commands.items():
                stdout, _ = ref.command(command)
                ref.check_work('coreav' if arm != 'ffmpeg' else 'ffmpeg_one', stdout, row['frames'])
                if arm != 'ffmpeg':
                    actual = json.loads(stdout)
                    if work is not None and actual != work:
                        raise ValueError('arm_work_mismatch')
                    work = actual
            row['work'] = work
            for repeat in range(7):
                arms = ('main', 'candidate', 'control', 'ffmpeg')
                if repeat % 2:
                    arms = tuple(reversed(arms))
                for arm in arms:
                    stdout, receipt = ref.command(commands[arm])
                    ref.check_work('coreav' if arm != 'ffmpeg' else 'ffmpeg_one', stdout, row['frames'])
                    if arm != 'ffmpeg' and json.loads(stdout) != work:
                        raise ValueError('timed_work_mismatch')
                    row['calls'].append(dict(arm=arm, repeat=repeat, **receipt))
            if identity in ('c00', 'c02'):
                small = directory / 'profile.hevc'
                ref.command([oracle.ffmpeg, '-hide_banner', '-loglevel', 'error', '-nostdin',
                    '-i', source, '-frames:v', '2', '-c:v', 'copy', '-f', 'hevc', '-y', small])
                stdout, _ = ref.command([args.main, small, 'decode'])
                profile_work = json.loads(stdout)
                row['profiles'] = {arm: count(binary, small, args.out, identity, arm, profile_work)
                                   for arm, binary in binaries.items()}
            row['complete'] = True
            save()
            print(json.dumps(dict(id=identity, depth=row['depth'], frames=row['frames'], complete=True)), flush=True)
        if {row['id'] for row in report['files']} != args.only:
            raise ValueError('incomplete_input_set')
        report['complete'] = True
finally:
    save()
    pool.close()
