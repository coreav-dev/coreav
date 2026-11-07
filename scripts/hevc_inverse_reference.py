#!/usr/bin/env python3
"""Compare explicit HEVC reconstruction commits on one reference host."""

import argparse
import json
import pathlib
import platform
import re
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_reference as reference
import test_oracle


def compare(baseline, candidate, oracle, source, repeats):
    expected = reference.verify(baseline, oracle, source)
    if reference.verify(candidate, oracle, source) != expected:
        raise ValueError('candidate_output_identity')
    commands = {
        'baseline': [baseline, source, 'decode'],
        'candidate': [candidate, source, 'decode'],
        'ffmpeg_one': reference.ffmpeg_command(oracle.ffmpeg, source, True,
            ['-progress', 'pipe:1', '-f', 'null', '-']),
    }
    arms = tuple(commands)
    for arm in arms:
        stdout, _ = reference.command(commands[arm])
        reference.check_work('coreav' if arm != 'ffmpeg_one' else arm, stdout, expected['frames'])
    calls = []
    for repeat in range(repeats):
        offset = repeat % len(arms)
        for arm in arms[offset:] + arms[:offset]:
            stdout, receipt = reference.command(commands[arm])
            reference.check_work('coreav' if arm != 'ffmpeg_one' else arm, stdout, expected['frames'])
            calls.append(dict(arm=arm, repeat=repeat, **receipt))
    return dict(**expected, calls=calls)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for arm in ('baseline', 'candidate'):
        parser.add_argument('--' + arm, type=pathlib.Path, required=True)
        parser.add_argument('--' + arm + '-sha', required=True)
    parser.add_argument('--oracle-bin', type=pathlib.Path, required=True)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('--only', required=True)
    parser.add_argument('--frames', type=int, default=32, choices=range(1, 65))
    parser.add_argument('--repeats', type=int, default=5, choices=range(5, 10))
    args = parser.parse_args()
    for sha in (args.baseline_sha, args.candidate_sha):
        if re.fullmatch('[0-9a-f]{40}', sha) is None:
            parser.error('full_commit_sha_required')
    args.only = set(args.only.split(','))
    if not args.only or args.only - {f'd{i:02}' for i in range(26)} - {f'c{i:02}' for i in range(4)}:
        parser.error('unknown_reference_input')
    oracle = test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
    args.out.mkdir(parents=True, exist_ok=True)
    report = dict(complete=False, baseline=args.baseline_sha, candidate=args.candidate_sha,
                  baseline_binary_sha256=reference.conformance.digest(args.baseline),
                  candidate_binary_sha256=reference.conformance.digest(args.candidate),
                  oracle=reference.oracle_pin.load()['BUILD'], selected_ids=sorted(args.only),
                  frames_cap=args.frames, repeats=args.repeats, files=[],
                  host=dict(cpu=next(line.split(':', 1)[1].strip()
                       for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines()
                       if line.startswith('model name')), kernel=platform.release()))
    pool = reference.round_files.pool()
    pool.limits.max_requests = 4000
    pool.limits.max_bytes = 32 << 30
    try:
        with tempfile.TemporaryDirectory(prefix='hevc-inverse-') as temp:
            directory = pathlib.Path(temp)
            for identity, source, original_sha in reference.inputs(args, oracle, directory, pool):
                row = dict(id=identity, source_sha256=original_sha,
                           input_sha256=reference.conformance.digest(source),
                           **compare(args.baseline, args.candidate, oracle, source, args.repeats))
                report['files'].append(row)
                if identity == 'c00':
                    small = directory / 'profile.hevc'
                    reference.command([oracle.ffmpeg, '-hide_banner', '-loglevel', 'error', '-nostdin',
                        '-i', source, '-frames:v', '2', '-c:v', 'copy', '-f', 'hevc', '-y', small])
                    for arm in ('baseline', 'candidate'):
                        binary = getattr(args, arm)
                        profile = dict(id=arm, **reference.verify(binary, oracle, small))
                        reference.profile(argparse.Namespace(native=binary), small, profile, directory)
                        row[arm + '_profile'] = profile
                report['r2_requests'] = pool.limits.requests
                (args.out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
            if {row['id'] for row in report['files']} != args.only:
                raise ValueError('incomplete_input_selection')
            report['complete'] = True
    finally:
        report['r2_requests'] = pool.limits.requests
        pool.close()
        (args.out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')


if __name__ == '__main__':
    main()
