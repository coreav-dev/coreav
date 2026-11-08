#!/usr/bin/env python3
"""Paired HEVC threading reference with complete-output identity checks."""

import argparse
import hashlib
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
import hevc_decode
import hevc_dependency
import hevc_row_dependency
import oracle_pin
import round_files
import test_oracle

THREADS = (0, 1, 4, 8, 16)
INPUT_IDS = tuple([f'd{index:02}' for index in range(26)] + [f'c{index:02}' for index in range(4)])
ROW_INPUT_IDS = tuple(identity for identity in INPUT_IDS
                      if identity not in {'d03', 'd06', 'd07', 'd10', 'd14', 'd22'})
DIAGNOSTIC_KEYS = frozenset(('barrier_' + name) for name in (
    'workers_disabled', 'wpp', 'output_random_access', 'output_capacity', 'output_reorder',
    'output_latency', 'reference_unavailable', 'memory_input', 'memory_scratch', 'memory_frame',
    'queue_budget', 'queue_memory', 'snapshot_allocation', 'window_full', 'duplicate_hash', 'worker_spawn'
)) | frozenset(('event_' + name) for name in (
    'picture_i', 'picture_p', 'picture_b', 'prefix_sei', 'suffix_sei_informational', 'suffix_sei_hash',
    'parameter_set', 'queue_admitted', 'spawned_job', 'caller_job', 'forced_flush', 'reconcile', 'sequence_end',
    'sei_type_0', 'sei_type_1', 'sei_type_5', 'sei_type_129', 'sei_type_132', 'sei_type_other'
))


def diagnostic_record(data):
    if not isinstance(data, dict):
        raise ValueError('diagnostic_record_missing')
    fields = ('pictures', 'decoded_pictures', 'bins', 'requested_threads', 'host_threads',
              'thread_limit', 'max_memory_bytes', 'parallel_pictures',
              'frame_parallel_pictures', 'max_workers')
    if any(type(data.get(key)) is not int or not 0 <= data[key] <= (1 << 64) - 1 for key in fields):
        raise ValueError('diagnostic_counts_missing')
    if not (0 < data['pictures'] <= data['decoded_pictures'] <= 32 and data['bins'] > 0
            and data['max_memory_bytes'] > 0 and data['requested_threads'] == 4
            and data['host_threads'] > 0 and data['thread_limit'] == min(4, data['host_threads'])
            and 1 <= data['max_workers'] <= data['thread_limit']
            and 0 <= data['frame_parallel_pictures'] <= data['parallel_pictures'] <= data['decoded_pictures']
            and data['frame_parallel_pictures'] != 1
            and (data['parallel_pictures'] == 0 or data['max_workers'] > 1)):
        raise ValueError('diagnostic_counts_invalid')
    counters = data.get('thread_diagnostics')
    if not isinstance(counters, list) or not counters:
        raise ValueError('diagnostic_counters_missing')
    histogram = {}
    events = {}
    for pair in counters:
        if not isinstance(pair, list) or len(pair) != 2:
            raise ValueError('diagnostic_counter_invalid')
        key, value = pair
        if not isinstance(key, str) or re.fullmatch(r'(barrier|event)_[a-z0-9_]+', key) is None \
                or type(value) is not int or not 0 <= value <= (1 << 64) - 1 \
                or key in histogram or key in events:
            raise ValueError('diagnostic_counter_invalid')
        if key.startswith('barrier_'):
            if value > data['decoded_pictures']:
                raise ValueError('diagnostic_barrier_denominator')
            histogram[key] = value
        else:
            events[key] = value
    if set(histogram) | set(events) != DIAGNOSTIC_KEYS:
        raise ValueError('diagnostic_counters_missing')
    return dict(raw=data, barrier_histogram=histogram, events=events)


def diagnostic_summary(files, selected):
    ids = [row['id'] for row in files]
    successful = [row for row in files if row['status'] == 'decoded']
    histogram = {}
    for row in successful:
        for key, value in row['barrier_histogram'].items():
            histogram[key] = histogram.get(key, 0) + value
    return dict(selected_inputs=len(selected), attempted_inputs=len(files), decoded_inputs=len(successful),
                errors=len(files) - len(successful),
                missing_inputs=len(set(selected) - set(ids)),
                complete=bool(selected) and len(ids) == len(set(ids)) and set(ids) == set(selected),
                pictures=sum(row['raw']['pictures'] for row in successful),
                decoded_pictures=sum(row['raw']['decoded_pictures'] for row in successful),
                frame_parallel_pictures=sum(row['raw']['frame_parallel_pictures'] for row in successful),
                inputs_with_overlap=sum(row['raw']['frame_parallel_pictures'] > 0 for row in successful),
                barrier_histogram=histogram)


def diagnostics(argv):
    parser = argparse.ArgumentParser(description='Internal Actions-only picture-job diagnostics')
    for name in ('native', 'oracle-bin', 'out'):
        parser.add_argument('--' + name, required=True, type=pathlib.Path)
    parser.add_argument('--candidate', required=True)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument('--dependencies-only', action='store_true')
    selection.add_argument('--rows-only', action='store_true')
    args = parser.parse_args(argv)
    if sys.platform != 'linux' or os.environ.get('GITHUB_ACTIONS') != 'true':
        parser.error('diagnostics_require_actions_linux')
    if re.fullmatch('[0-9a-f]{40}', args.candidate) is None:
        parser.error('full_candidate_sha_required')
    selected_ids = ROW_INPUT_IDS if args.rows_only else INPUT_IDS
    args.only = set(selected_ids)
    args.frames = 32
    oracle = test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
    args.out.mkdir(parents=True, exist_ok=True)
    report = dict(candidate=args.candidate, candidate_sha256=conformance.digest(args.native),
                  oracle=oracle_pin.load()['BUILD'], selected_ids=list(selected_ids), frames_cap=32,
                  requested_threads=1 if args.rows_only else 0 if args.dependencies_only else 4,
                  mode='row_dependencies' if args.rows_only else 'header_dependencies' if args.dependencies_only else 'decode',
                  header_only=args.dependencies_only,
                  scope='actual decoded inter-reference reads by CTU row; synchronous reconstruction; '
                        'diagnostic models only, no baseline, timing or profiling'
                        if args.rows_only else
                        'header-only RPS and active-list dependency replay; no reconstruction, timing or profiling'
                        if args.dependencies_only else
                        'picture-job diagnostics only; no pixel comparison, baseline, timing or profiling; '
                        'barriers count affected pictures per reason and may overlap across reasons; '
                        'events are not picture counts',
                  host=dict(cpu=next(line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),
                            cores=os.cpu_count(), affinity=sorted(os.sched_getaffinity(0)), kernel=platform.release()),
                  complete=False, passed=False, files=[])
    pool = round_files.pool()
    pool.limits.max_requests = 4000
    pool.limits.max_bytes = 32 << 30
    try:
        with tempfile.TemporaryDirectory(prefix='hevc-thread-diagnostics-') as temp:
            seen = set()
            for identity, source, original_sha in reference.inputs(args, oracle, pathlib.Path(temp), pool):
                if identity not in args.only or identity in seen:
                    raise ValueError('diagnostic_input_identity')
                seen.add(identity)
                row = dict(id=identity, source_sha256=original_sha, status='failed')
                report['files'].append(row)
                try:
                    row['input_sha256'] = conformance.digest(source)
                    mode = ('rows', '1') if args.rows_only else ('dependencies', '1') if args.dependencies_only else ('decode', '4')
                    child = subprocess.run([str(args.native), str(source), *mode],
                                           capture_output=True, check=True, timeout=600, env=reference.CHILD_ENV)
                    data = json.loads(child.stdout)
                    row['raw'] = data
                    if args.rows_only:
                        row.update(row_dependency_ceiling=hevc_row_dependency.analyze(data), status='decoded')
                    elif args.dependencies_only:
                        if 'output_events' not in data:
                            raise ValueError('dependency_output_events_missing')
                        row.update(dependency_ceiling=hevc_dependency.analyze(data), status='parsed')
                    else:
                        row.update(diagnostic_record(data), status='decoded')
                except (ValueError, OSError, subprocess.SubprocessError) as error:
                    row['failure'] = type(error).__name__
                    if args.rows_only and isinstance(error, ValueError):
                        row['failure_reason'] = 'row_dependency_record_invalid'
                    if args.dependencies_only and isinstance(error, ValueError):
                        row['failure_reason'] = 'dependency_record_invalid'
                    if isinstance(error, ValueError) and str(error) in {
                            'diagnostic_record_missing', 'diagnostic_counts_missing', 'diagnostic_counts_invalid',
                            'diagnostic_counters_missing', 'diagnostic_counter_invalid', 'diagnostic_barrier_denominator'}:
                        row['failure_reason'] = str(error)
                    if isinstance(error, subprocess.CalledProcessError):
                        row['exit_code'] = error.returncode
                (args.out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        report['failure'] = type(error).__name__
    finally:
        pool.close()
        report['r2_requests'] = pool.limits.requests
        if args.rows_only:
            rows = report['files']
            successful = [row for row in rows if row['status'] == 'decoded']
            ids = [row['id'] for row in rows]
            report['summary'] = dict(selected_inputs=len(args.only), attempted_inputs=len(rows),
                                    decoded_inputs=len(successful), errors=len(rows) - len(successful),
                                    missing_inputs=len(args.only - set(ids)),
                                    complete=len(ids) == len(set(ids)) and set(ids) == args.only,
                                    decoded_pictures=sum(row['raw']['decoded_pictures'] for row in successful))
        elif args.dependencies_only:
            rows = report['files']
            successful = [row for row in rows if row['status'] == 'parsed']
            ids = [row['id'] for row in rows]
            report['summary'] = dict(selected_inputs=len(args.only), attempted_inputs=len(rows),
                                    parsed_inputs=len(successful), errors=len(rows) - len(successful),
                                    missing_inputs=len(args.only - set(ids)),
                                    complete=len(ids) == len(set(ids)) and set(ids) == args.only,
                                    coded_pictures=sum(row['raw']['coded_pictures'] for row in successful),
                                    reconstruction_required=sum(row['raw']['reconstruction_required'] for row in successful))
        else:
            report['summary'] = diagnostic_summary(report['files'], args.only)
        report['summary']['preparation_errors'] = int('failure' in report)
        report['summary']['errors'] += report['summary']['preparation_errors']
        report['complete'] = report['summary']['complete']
        report['passed'] = report['complete'] and report['summary']['errors'] == 0 and 'failure' not in report
        (args.out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(dict(selected=len(args.only), attempted=report['summary']['attempted_inputs'],
                          errors=report['summary']['errors'], complete=report['complete'], passed=report['passed'])))
    return 0 if report['passed'] else 1


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
        return {key: normalized(item) for key, item in value.items() if key not in {'memory_bytes', 'parallel_pictures', 'frame_parallel_pictures', 'max_workers'}}
    if isinstance(value, list):
        return [normalized(item) for item in value]
    return value


def compare_conformance(root):
    reports = [conformance.read_json(root / f'threads-{threads}' / 'result.json')
               for threads in (1, 2, 4, 8)]
    for report in reports:
        conformance.require(report.get('complete') is True and report.get('passed') is True
                            and report.get('scope') == 'complete_c1' and report.get('results'),
                            'thread_conformance_incomplete')
    expected = normalized(reports[0])
    conformance.require(all(normalized(report) == expected for report in reports[1:]),
                        'thread_conformance_mismatch')
    coverage = {str(threads): hevc_decode.threading_coverage(report['results'], threads)
                for threads, report in zip((1, 2, 4, 8), reports)}
    for threads, report in zip((1, 2, 4, 8), reports):
        coverage[str(threads)]['picture_job_streams'] = sum(
            row['summary'].get('frame_parallel_pictures', 0) > 0 for row in report['results'])
    conformance.require(all(coverage[str(threads)]['frame_parallel_pictures'] > 0
                            for threads in (2, 4, 8)), 'native_picture_job_coverage_missing')
    canonical = json.dumps(expected, sort_keys=True, separators=(',', ':')).encode()
    return dict(passed=True, streams=len(expected['results']), threading=coverage,
                pictures=sum(row['summary']['decoded_pictures'] for row in expected['results']),
                normalized_sha256=hashlib.sha256(canonical).hexdigest())


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
        row.update(reference.verify(args.main, oracle, source, timeout=600))
    except (ValueError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        row.update(status='baseline_not_verified', failure=type(error).__name__)
        if isinstance(error, ValueError) and str(error) in {
                'incomplete_native', 'depth_change', 'oracle_geometry', 'oracle_pixel_format', 'pixel_mismatch'}:
            row['verification_failure'] = str(error)
        if isinstance(error, subprocess.TimeoutExpired):
            program = error.cmd[0]
            row.update(timeout_seconds=error.timeout,
                       verification_step='baseline_decode' if program == str(args.main)
                       else 'oracle_probe' if program == str(oracle.ffprobe) else 'oracle_decode')
        return row
    expected = signature(args.main, source, 1)
    row['status'] = 'checking_candidate'
    for threads in (0, 1, 2, 4, 8, 16):
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
                receipt.update(thread_limit=data['thread_limit'], requested_threads=data['requested_threads'],
                               host_threads=data['host_threads'], max_memory_bytes=data['max_memory_bytes'])
                if name.startswith('candidate_'):
                    receipt.update(parallel_pictures=data['parallel_pictures'],
                                   frame_parallel_pictures=data['frame_parallel_pictures'],
                                   max_workers=data['max_workers'])
            calls.append(dict(arm=name, repeat=repeat, **receipt))
    row.update(status='timed', calls=[call for call in calls if call['repeat'] >= 0], warmups=[call for call in calls if call['repeat'] < 0], work=work)
    return row


def main():
    if sys.argv[1:2] == ['diagnostics']:
        return diagnostics(sys.argv[2:])
    if sys.argv[1:2] == ['census']:
        return census(sys.argv[2:])
    if sys.argv[1:2] == ['compare-conformance']:
        parser = argparse.ArgumentParser(description='Compare complete threaded conformance records')
        parser.add_argument('--root', required=True, type=pathlib.Path)
        parser.add_argument('--out', required=True, type=pathlib.Path)
        args = parser.parse_args(sys.argv[2:])
        args.out.write_text(json.dumps(compare_conformance(args.root), indent=2) + '\n')
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('native', 'main', 'oracle-bin', 'out'):
        parser.add_argument('--' + name, required=True, type=pathlib.Path)
    for name in ('candidate', 'baseline'):
        parser.add_argument('--' + name, required=True)
    parser.add_argument('--shard-index', type=int, default=0)
    parser.add_argument('--shard-count', type=int, default=1)
    parser.add_argument('--only', default='')
    parser.add_argument('--repeats', type=int, choices=range(3, 10), default=5)
    parser.add_argument('--frames', type=int, choices=range(1, 65), default=32)
    args = parser.parse_args()
    if any(re.fullmatch('[0-9a-f]{40}', getattr(args, name)) is None for name in ('candidate', 'baseline')):
        parser.error('full_arm_shas_required')
    if not 0 <= args.shard_index < args.shard_count <= 30:
        parser.error('invalid_shard')
    ids = [f'd{index:02}' for index in range(26)] + [f'c{index:02}' for index in range(4)]
    selected = set(args.only.split(',')) if args.only else set(ids)
    if not selected <= set(ids):
        parser.error('unknown_input_identity')
    args.only = {identity for index, identity in enumerate(ids)
                 if index % args.shard_count == args.shard_index and identity in selected}
    if not args.only:
        parser.error('empty_shard')
    oracle = test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
    args.out.mkdir(parents=True, exist_ok=True)
    report = dict(candidate=args.candidate, baseline=args.baseline, candidate_sha256=conformance.digest(args.native),
                  baseline_sha256=conformance.digest(args.main), oracle=oracle_pin.load()['BUILD'],
                  host=dict(cpu=next(line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines() if line.startswith('model name')),
                            cores=os.cpu_count(), affinity=sorted(os.sched_getaffinity(0)), kernel=platform.release()),
                  scope='whole-process Annex B decode-to-null; startup, framing and teardown included; count 0 selects automatic workers with the backend memory budget; workers are bounded by available CPUs; baseline uses the synchronous decode entry point',
                  selected_ids=sorted(args.only), frames_cap=args.frames, repeats=args.repeats, complete=False, passed=True, files=[])
    pool = round_files.pool()
    pool.limits.max_requests = 4000
    pool.limits.max_bytes = 32 << 30
    try:
        with tempfile.TemporaryDirectory(prefix='hevc-threading-') as temp:
            for identity, source, original_sha in reference.inputs(args, oracle, pathlib.Path(temp), pool):
                try:
                    row = measure(args, oracle, source, identity)
                    if row['status'] != 'timed':
                        report['passed'] = False
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
