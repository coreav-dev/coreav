#!/usr/bin/env python3
"""Paired HEVC threading reference with complete-output identity checks."""

import argparse
import json
import os
import pathlib
import platform
import re
import statistics
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_reference as reference
import hevc_conformance as conformance
import oracle_pin
import round_files
import test_oracle

THREADS = (1, 4, 8, 16)


def syntax_record(item, source, directory, oracle, parser):
    directory = directory / item['sha256']
    elementary = directory / 'syntax.hevc'
    record = dict(item, status='blocked')
    try:
        code, _, _ = conformance.child([oracle.ffmpeg, '-nostdin', '-v', 'error', '-i', source,
            '-map', '0:v:0', '-c:v', 'copy', '-bsf:v', 'hevc_mp4toannexb', '-f', 'hevc', '-y', elementary],
            directory, 'extract', file_limit=4 << 30)
        conformance.require(code == 0 and 0 < elementary.stat().st_size < 4 << 30, 'syntax_extraction_failed')
        code, stdout, _ = conformance.child([parser, elementary], directory, 'headers')
        conformance.require(code == 0, 'syntax_inspection_failed')
        rows = [json.loads(line) for line in stdout.read_text().splitlines()]
        conformance.require(rows and rows[-1].get('kind') == 'summary' and rows[-1].get('complete') is True,
                            'syntax_inspection_incomplete')
        tools = rows[-1]['tools']
        conformance.require(all(type(tools.get(key)) is bool for key in ('wpp', 'tiles')), 'syntax_tools_missing')
        record.update(status='inspected', wpp=tools['wpp'], tiles=tools['tiles'], parser_sha256=conformance.digest(parser))
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        record['failure'] = type(error).__name__
    finally:
        elementary.unlink(missing_ok=True)
    return record


def census(argv):
    parser = argparse.ArgumentParser(description='Count active WPP and tile syntax over the complete existing roster')
    for name in ('parser', 'oracle-bin', 'dest', 'out'):
        parser.add_argument('--' + name, required=True, type=pathlib.Path)
    args = parser.parse_args(argv)
    args.manifests = None
    args.native = None
    def inspect(item, source, directory, oracle, **unused):
        return syntax_record(item, source, directory, oracle, args.parser)
    try:
        conformance.fetch_corpus(args, inspect=True, inspector=inspect)
    finally:
        path = args.out / 'corpus-census.json'
        if path.exists():
            report = json.loads(path.read_text())
            files = [row for row in report['files'] if row['status'] == 'inspected']
            counts = dict(files=len(report['files']), inspected=len(files),
                          wpp=sum(row['wpp'] for row in files), tiles=sum(row['tiles'] for row in files),
                          neither=sum(not row['wpp'] and not row['tiles'] for row in files),
                          both=sum(row['wpp'] and row['tiles'] for row in files), complete=report['complete'])
            (args.out / 'tool-counts.json').write_text(json.dumps(counts, indent=2) + '\n')
    return 0


def normalized(value):
    if isinstance(value, dict):
        return {key: normalized(item) for key, item in value.items() if key not in {'memory_bytes', 'parallel_pictures', 'max_workers'}}
    if isinstance(value, list):
        return [normalized(item) for item in value]
    return value


def signature(binary, source, threads):
    stdout, _ = reference.command([binary, source, 'check', threads], timeout=600)
    rows = [json.loads(line) for line in stdout.splitlines()]
    if not rows or rows[-1].get('complete') is not True:
        raise ValueError('incomplete_native')
    return normalized(rows)


def summarize(files):
    result = {}
    for depth in (8, 10):
        selected = [row for row in files if row.get('status') == 'timed' and row['depth'] == depth]
        ratios = {str(threads): [] for threads in THREADS}
        scaling = {str(threads): [] for threads in THREADS}
        oracle = {str(threads): [] for threads in THREADS}
        for row in selected:
            medians = {arm: statistics.median(call['wall'] for call in row['calls'] if call['arm'] == arm)
                       for arm in {call['arm'] for call in row['calls']}}
            for threads in THREADS:
                ratios[str(threads)].append(medians[f'main_{threads}'] / medians[f'candidate_{threads}'])
                scaling[str(threads)].append(medians['candidate_1'] / medians[f'candidate_{threads}'])
                oracle[str(threads)].append(medians[f'candidate_{threads}'] / medians['ffmpeg_one'])
        result[str(depth)] = dict(files=len(selected), main_over_candidate={key: statistics.median(values) if values else None for key, values in ratios.items()},
                                 candidate_one_over_threads={key: statistics.median(values) if values else None for key, values in scaling.items()},
                                 candidate_over_ffmpeg_one={key: statistics.median(values) if values else None for key, values in oracle.items()})
    return result


def measure(args, oracle, source, identity):
    row = dict(id=identity, input_sha256=conformance.digest(source), status='checking_main')
    try:
        row.update(reference.verify(args.main, oracle, source))
    except (ValueError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        row.update(status='baseline_not_verified', failure=type(error).__name__)
        return row
    expected = signature(args.main, source, 1)
    row['status'] = 'checking_candidate'
    for threads in (1, 2, 4, 8, 16):
        if signature(args.native, source, threads) != expected:
            raise ValueError('thread_output_mismatch')
    commands = {f'{arm}_{threads}': [binary, source, 'decode', threads]
                for arm, binary in [('main', args.main), ('candidate', args.native)] for threads in THREADS}
    commands['ffmpeg_one'] = reference.ffmpeg_command(oracle.ffmpeg, source, True, ['-progress', 'pipe:1', '-f', 'null', '-'])
    commands['ffmpeg_default'] = reference.ffmpeg_command(oracle.ffmpeg, source, False, ['-progress', 'pipe:1', '-f', 'null', '-'])
    names = tuple(commands)
    work = None
    calls = []
    for repeat in range(-1, args.repeats):
        offset = max(0, repeat) % len(names)
        order = names[offset:] + names[:offset]
        if repeat % 2:
            order = tuple(reversed(order))
        for name in order:
            stdout, receipt = reference.command(commands[name], timeout=600)
            reference.check_work('coreav' if name.startswith(('main_', 'candidate_')) else name, stdout, row['frames'])
            if name.startswith(('main_', 'candidate_')):
                data = json.loads(stdout)
                observed = {key: data[key] for key in ('pictures', 'decoded_pictures', 'bins')}
                if work is not None and work != observed:
                    raise ValueError('thread_work_mismatch')
                work = observed
                if name.startswith('candidate_'):
                    receipt.update(parallel_pictures=data['parallel_pictures'], max_workers=data['max_workers'])
            calls.append(dict(arm=name, repeat=repeat, **receipt))
    row.update(status='timed', calls=[call for call in calls if call['repeat'] >= 0], warmups=[call for call in calls if call['repeat'] < 0], work=work)
    return row


def main():
    if sys.argv[1:2] == ['census']:
        return census(sys.argv[2:])
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('native', 'main', 'oracle-bin', 'out'):
        parser.add_argument('--' + name, required=True, type=pathlib.Path)
    for name in ('candidate', 'baseline'):
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--shard-index', type=int, default=0)
    parser.add_argument('--shard-count', type=int, default=1)
    parser.add_argument('--repeats', type=int, choices=range(3, 10), default=5)
    parser.add_argument('--frames', type=int, choices=range(1, 65), default=32)
    args = parser.parse_args()
    if any(re.fullmatch('[0-9a-f]{40}', getattr(args, name)) is None for name in ('candidate', 'baseline')):
        parser.error('full_arm_shas_required')
    if not 0 <= args.shard_index < args.shard_count <= 30:
        parser.error('invalid_shard')
    ids = [f'd{index:02}' for index in range(26)] + [f'c{index:02}' for index in range(4)]
    args.only = {identity for index, identity in enumerate(ids) if index % args.shard_count == args.shard_index}
    oracle = test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
    args.out.mkdir(parents=True, exist_ok=True)
    report = dict(candidate=args.candidate, baseline=args.baseline, candidate_sha256=conformance.digest(args.native),
                  baseline_sha256=conformance.digest(args.main), oracle=oracle_pin.load()['BUILD'],
                  host=dict(cpu=next(line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),
                            cores=os.cpu_count(), affinity=sorted(os.sched_getaffinity(0)), kernel=platform.release()),
                  scope='whole-process Annex B decode-to-null; startup, framing and teardown included; main is serial at every requested count',
                  selected_ids=sorted(args.only), frames_cap=args.frames, repeats=args.repeats, complete=False, passed=True, files=[])
    pool = round_files.pool()
    pool.limits.max_requests = 4000
    pool.limits.max_bytes = 32 << 30
    try:
        with tempfile.TemporaryDirectory(prefix='hevc-threading-') as temp:
            for identity, source, original_sha in reference.inputs(args, oracle, pathlib.Path(temp), pool):
                try:
                    row = measure(args, oracle, source, identity)
                except (ValueError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
                    row = dict(id=identity, status='failed', failure=type(error).__name__)
                    if isinstance(error, ValueError):
                        row['failure'] = str(error)
                    report['passed'] = False
                row['source_sha256'] = original_sha
                report['files'].append(row)
                (args.out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
                print(json.dumps(dict(id=identity, status=row['status'], failure=row.get('failure'))), flush=True)
                if not report['passed']:
                    break
            report['complete'] = {row['id'] for row in report['files']} == args.only
    finally:
        pool.close()
        report['r2_requests'] = pool.limits.requests
        report['summary'] = summarize(report['files'])
        (args.out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    return 0 if report['complete'] and report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
