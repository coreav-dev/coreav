import copy
import itertools
import json
import pathlib
import re
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import hevc_threading as threading
import hevc_decode


class ThreadingReference(unittest.TestCase):
    def test_main_baseline_keeps_wavefront_threads_and_uses_only_the_sync_api(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        source = (root / 'bench/hevc-reference/decode.rs').read_text()
        self.assertIn('PictureDecoder::new(limits).with_threads(threads)', source)
        baseline = source.split('#[cfg(hevc_synchronous_baseline)]', 1)[1].split(
            '#[cfg(not(hevc_synchronous_baseline))]', 1)[0]
        self.assertIn('decoder.read_nal(nal, None, &mut callback)', baseline)
        self.assertNotIn('read_nal_scheduled', baseline)
        self.assertIn('decoder.read_nal_scheduled(nal, None, None,', source)
        self.assertNotIn('black_box(threads)', source)
        workflow = (root / '.github/workflows/hevc-threading.yml').read_text()
        self.assertIn('ref: ${{ inputs.baseline }}\n          fetch-depth: 0', workflow)
        self.assertIn('--cfg hevc_synchronous_baseline', workflow)
        self.assertNotIn('hevc_serial_baseline', workflow)

    def test_explicit_sanitizer_mode_overrides_reference_selection(self):
        workflow = pathlib.Path(__file__).resolve().parents[2] / '.github/workflows/hevc-threading.yml'
        job = workflow.read_text().split('  thread-sanitizer:', 1)[1]
        self.assertIn("if: ${{ !inputs.thread_diagnostics_only && !inputs.thread_dependencies_only && !inputs.thread_rows_only && (inputs.sanitizer_only || inputs.only == '') }}", job)

    def test_sanitizer_executes_both_scheduling_owners_and_rejects_zero_tests(self):
        workflow = pathlib.Path(__file__).resolve().parents[2] / '.github/workflows/hevc-threading.yml'
        job = workflow.read_text().split('  thread-sanitizer:', 1)[1]
        self.assertIn('for group in wavefront decoder::scheduler::tests; do', job)
        self.assertIn('--lib "$group" | tee', job)
        self.assertIn('set -euo pipefail', job)
        check = next(line.strip() for line in job.splitlines() if line.strip().startswith('grep -Eq'))
        with tempfile.TemporaryDirectory() as temp:
            log = pathlib.Path(temp) / 'threading-tests.log'
            for count in (0, 1, 20):
                log.write_text(f'test result: ok. {count} passed; 0 failed; 255 filtered out\n')
                result = subprocess.run(['bash', '-c', check.replace('"$RUNNER_TEMP/threading-tests.log"', '"$1"'),
                                         'check', str(log)], capture_output=True, check=False)
                self.assertEqual(result.returncode == 0, count > 0)

    def test_threaded_conformance_rejects_silent_serial_fallback(self):
        serial = dict(complete=True, summary=dict(decoded_pictures=3, parallel_pictures=0, max_workers=1,
                                                stats=dict(wpp_restarts=2)))
        self.assertEqual(hevc_decode.threading_coverage([serial], 1),
                         dict(parallel_pictures=0, frame_parallel_pictures=0, max_workers=1))
        for threads in (2, 4, 8):
            with self.assertRaisesRegex(ValueError, 'native_parallel_coverage_missing'):
                hevc_decode.threading_coverage([serial], threads)
        parallel = copy.deepcopy(serial)
        parallel['summary'].update(parallel_pictures=2, max_workers=2)
        self.assertEqual(hevc_decode.threading_coverage([serial, parallel], 4),
                         dict(parallel_pictures=2, frame_parallel_pictures=0, max_workers=2))
        parallel['summary']['stats']['wpp_restarts'] = 0
        with self.assertRaisesRegex(ValueError, 'native_parallel_wpp_work'):
            hevc_decode.threading_coverage([parallel], 4)

    def test_frame_coverage_requires_a_complete_overlapping_pair(self):
        frame = dict(complete=True, summary=dict(decoded_pictures=3, parallel_pictures=2,
                     frame_parallel_pictures=2, max_workers=2, stats=dict(wpp_restarts=0)))
        self.assertEqual(hevc_decode.threading_coverage([frame], 4),
                         dict(parallel_pictures=2, frame_parallel_pictures=2, max_workers=2))
        for count in (True, -1, 1, 3, '2', None):
            changed = copy.deepcopy(frame)
            changed['summary']['frame_parallel_pictures'] = count
            with self.subTest(count=count), self.assertRaisesRegex(ValueError, 'native_frame_thread_counters'):
                hevc_decode.threading_coverage([changed], 4)
        for workers in (0, 1, 5):
            changed = copy.deepcopy(frame)
            changed['summary']['max_workers'] = workers
            with self.subTest(workers=workers), self.assertRaisesRegex(ValueError, 'native_thread_counters'):
                hevc_decode.threading_coverage([changed], 4)

    def test_frame_coverage_does_not_hide_unproved_wavefront_credit(self):
        mixed = dict(complete=True, summary=dict(decoded_pictures=4, parallel_pictures=3,
                     frame_parallel_pictures=2, max_workers=2, stats=dict(wpp_restarts=0)))
        with self.assertRaisesRegex(ValueError, 'native_parallel_wpp_work'):
            hevc_decode.threading_coverage([mixed], 4)
        mixed['summary']['stats']['wpp_restarts'] = 2
        self.assertEqual(hevc_decode.threading_coverage([mixed], 4),
                         dict(parallel_pictures=3, frame_parallel_pictures=2, max_workers=2))

    def test_unverified_baseline_cannot_pass_or_continue_the_shard(self):
        with tempfile.TemporaryDirectory() as temp:
            out = pathlib.Path(temp) / 'out'
            args = ['driver', '--native', '/native', '--main', '/main', '--oracle-bin', '/oracle', '--out', str(out),
                    '--candidate', 'a' * 40, '--baseline', 'b' * 40, '--only', 'd00,d01']
            pool = types.SimpleNamespace(limits=types.SimpleNamespace(requests=0), close=lambda: None)
            with patch.object(sys, 'argv', args), \
                    patch.object(threading.test_oracle, 'require', return_value=object()), \
                    patch.object(threading.conformance, 'digest', return_value='c' * 64), \
                    patch.object(threading.oracle_pin, 'load', return_value=dict(BUILD='qualified')), \
                    patch.object(pathlib.Path, 'read_text', return_value='model name : fixture host\n'), \
                    patch.object(threading.os, 'sched_getaffinity', return_value={0, 1}, create=True), \
                    patch.object(threading.round_files, 'pool', return_value=pool), \
                    patch.object(threading.reference, 'inputs', return_value=[('d00', pathlib.Path('/input'), 'd' * 64),
                                                                             ('d01', pathlib.Path('/next'), 'e' * 64)]), \
                    patch.object(threading, 'measure', return_value=dict(id='d00', status='baseline_not_verified')) as measure:
                self.assertEqual(threading.main(), 1)
                measure.assert_called_once()
            report = json.loads((out / 'results.json').read_text())
            self.assertFalse(report['passed'])
            self.assertEqual(report['selected_ids'], ['d00', 'd01'])

    def test_baseline_failure_retains_only_closed_verification_reasons(self):
        args = types.SimpleNamespace(native='/candidate', main=pathlib.Path('/main'))
        for reason in ('incomplete_native', 'depth_change', 'oracle_geometry', 'oracle_pixel_format',
                       'pixel_mismatch', 'untrusted diagnostic'):
            with self.subTest(reason=reason), \
                    patch.object(threading.conformance, 'digest', return_value='a' * 64), \
                    patch.object(threading.reference, 'verify', side_effect=ValueError(reason)):
                row = threading.measure(args, object(), pathlib.Path('/input'), 'd00')
            self.assertEqual(row['status'], 'baseline_not_verified')
            self.assertEqual(row['failure'], 'ValueError')
            self.assertEqual(row.get('verification_failure'), None if reason == 'untrusted diagnostic' else reason)

    def test_baseline_timeout_records_the_failing_verification_step(self):
        args = types.SimpleNamespace(native='/candidate', main=pathlib.Path('/main'))
        oracle = types.SimpleNamespace(ffmpeg='/qualified-ffmpeg', ffprobe='/qualified-ffprobe')
        for program, step in [('/main', 'baseline_decode'), (oracle.ffprobe, 'oracle_probe'),
                              (oracle.ffmpeg, 'oracle_decode')]:
            with patch.object(threading.conformance, 'digest', return_value='a' * 64), \
                    patch.object(threading.reference, 'verify', side_effect=subprocess.TimeoutExpired([program], 600)) as verify:
                row = threading.measure(args, oracle, pathlib.Path('/input'), 'd00')
            self.assertEqual((row['status'], row['verification_step'], row['timeout_seconds']),
                             ('baseline_not_verified', step, 600))
            verify.assert_called_once_with(args.main, oracle, pathlib.Path('/input'), timeout=600)

    def test_census_counts_only_complete_native_syntax_and_uses_qualified_extraction(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            source = root / 'source'
            source.write_bytes(b'fixture')
            oracle = types.SimpleNamespace(ffmpeg='/qualified-ffmpeg')
            commands = []
            def child(command, directory, label, **unused):
                commands.append(command)
                directory.mkdir(parents=True, exist_ok=True)
                if label == 'extract':
                    pathlib.Path(command[-1]).write_bytes(b'fixture')
                stdout = directory / (label + '.stdout')
                stdout.write_text('{"kind":"summary","complete":true,"tools":{"wpp":true,"tiles":false}}')
                return 0, stdout, directory / 'stderr'
            with patch.object(threading.conformance, 'child', side_effect=child), \
                    patch.object(threading.conformance, 'digest', return_value='b' * 64):
                result = threading.syntax_record(dict(sha256='a' * 64), source, root, oracle, '/native-parser')
            self.assertEqual((result['status'], result['wpp'], result['tiles']), ('inspected', True, False))
            self.assertEqual(commands[0][0], oracle.ffmpeg)
            self.assertEqual(commands[1][0], '/native-parser')
            self.assertFalse((root / ('a' * 64) / 'syntax.hevc').exists())

    def test_identity_ignores_only_resource_and_scheduling_counters(self):
        one = dict(output_md5='a' * 32, stats=dict(ctus=3, memory_bytes=100), parallel_pictures=0, max_workers=1)
        many = copy.deepcopy(one)
        many.update(parallel_pictures=1, max_workers=8)
        many['frame_parallel_pictures'] = 2
        many['stats']['memory_bytes'] = 500
        self.assertEqual(threading.normalized(one), threading.normalized(many))
        for owner, key in [(many, 'output_md5'), (many['stats'], 'ctus')]:
            old = owner[key]
            owner[key] = 'changed'
            self.assertNotEqual(threading.normalized(one), threading.normalized(many))
            owner[key] = old

    def test_thread_count_reaches_every_conformance_control(self):
        args = types.SimpleNamespace(native='/native', max_memory_bytes=100, max_stream_bins=200, max_picture_bins=300, threads=8)
        self.assertEqual(hevc_decode.command(args, '/input'), ['/native', '/input', '100', '200', '300', '8'])
        self.assertEqual(hevc_decode.command(args, '/input', stream_bins=0), ['/native', '/input', '100', '0', '300', '8'])
        del args.threads
        self.assertEqual(hevc_decode.command(args, '/input'), ['/native', '/input', '100', '200', '300'])

    def test_complete_conformance_compares_every_semantic_record(self):
        one = dict(complete=True, passed=True, scope='complete_c1', identities=dict(native='a' * 64),
                   results=[dict(complete=True, sha256='b' * 64,
                                 summary=dict(decoded_pictures=2, parallel_pictures=0, max_workers=1,
                                              stats=dict(bins=25, memory_bytes=100)),
                                 pictures=[dict(output_md5='c' * 32, poc=1)])])
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            for count in (1, 2, 4, 8):
                directory = root / f'threads-{count}'
                directory.mkdir()
                many = copy.deepcopy(one)
                many['results'][0]['summary'].update(frame_parallel_pictures=2, parallel_pictures=2,
                                                    max_workers=count)
                many['results'][0]['summary']['stats']['memory_bytes'] = count * 100
                (directory / 'result.json').write_text(json.dumps(one if count == 1 else many))
            receipt = threading.compare_conformance(root)
            self.assertEqual((receipt['passed'], receipt['streams'], receipt['pictures']), (True, 1, 2))
            self.assertEqual(receipt['threading']['4']['frame_parallel_pictures'], 2)
            self.assertEqual(receipt['threading']['4']['picture_job_streams'], 1)
            for field in ('bins', 'output_md5', 'poc', 'source'):
                changed = copy.deepcopy(many)
                if field == 'bins':
                    changed['results'][0]['summary']['stats']['bins'] += 1
                elif field == 'source':
                    changed['results'][0]['sha256'] = 'd' * 64
                else:
                    changed['results'][0]['pictures'][0][field] = 'changed'
                (root / 'threads-8' / 'result.json').write_text(json.dumps(changed))
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'thread_conformance_mismatch'):
                    threading.compare_conformance(root)
            changed = copy.deepcopy(many)
            changed['complete'] = False
            (root / 'threads-8' / 'result.json').write_text(json.dumps(changed))
            with self.assertRaisesRegex(ValueError, 'thread_conformance_incomplete'):
                threading.compare_conformance(root)
            for count in (2, 4, 8):
                changed = copy.deepcopy(many)
                changed['results'][0]['summary']['frame_parallel_pictures'] = 0
                changed['results'][0]['summary']['max_workers'] = count
                changed['results'][0]['summary']['stats']['wpp_restarts'] = 1
                (root / f'threads-{count}' / 'result.json').write_text(json.dumps(changed))
            changed = copy.deepcopy(one)
            changed['results'][0]['summary']['stats']['wpp_restarts'] = 1
            (root / 'threads-1' / 'result.json').write_text(json.dumps(changed))
            with self.assertRaisesRegex(ValueError, 'native_picture_job_coverage_missing'):
                threading.compare_conformance(root)
        workflow = (pathlib.Path(__file__).resolve().parents[2] / '.github/workflows/hevc-threading.yml').read_text()
        self.assertIn('hevc_threading.py compare-conformance --root', workflow)

    def test_signature_requires_completion_and_supplies_threads(self):
        with patch.object(threading.reference, 'command', return_value=(b'{"type":"summary","complete":true}', {})) as child:
            for threads in (0, 4):
                threading.signature('/native', '/input', threads)
                self.assertEqual(child.call_args.args[0], ['/native', '/input', 'check', threads])
        with patch.object(threading.reference, 'command', return_value=(b'{"complete":false}', {})):
            with self.assertRaisesRegex(ValueError, 'incomplete_native'):
                threading.signature('/native', '/input', 4)

    def test_output_mismatch_prevents_any_timing(self):
        args = types.SimpleNamespace(native='/candidate', main='/main')
        with patch.object(threading.conformance, 'digest', return_value='a' * 64), \
             patch.object(threading.reference, 'verify', return_value=dict(frames=2)), \
             patch.object(threading, 'signature', side_effect=[[dict(output_md5='a')], [dict(output_md5='b')]]), \
             patch.object(threading.reference, 'command') as child:
            with self.assertRaisesRegex(ValueError, 'thread_output_mismatch'):
                threading.measure(args, object(), pathlib.Path('/input'), 'd00')
            child.assert_not_called()

    def test_ratio_direction_and_every_requested_count(self):
        calls = []
        for threads in threading.THREADS:
            calls += [dict(arm=f'main_{threads}', wall=12), dict(arm=f'candidate_{threads}', wall=6 / (threads or 4))]
        calls += [dict(arm='ffmpeg_one', wall=2)]
        summary = threading.summarize([dict(status='timed', depth=8, calls=calls)])['8']
        self.assertEqual(summary['main_over_candidate']['4'], 8)
        self.assertEqual(summary['main_over_candidate']['0'], 8)
        self.assertEqual(summary['candidate_one_over_threads']['4'], 4)
        self.assertEqual(summary['candidate_over_ffmpeg_one']['1'], 3)

    def test_timed_receipts_retain_requested_host_and_frame_work(self):
        args = types.SimpleNamespace(native='/candidate', main='/main', repeats=3)
        oracle = types.SimpleNamespace(ffmpeg='/ffmpeg', ffprobe='/ffprobe')
        def child(command, **unused):
            if command[0] == oracle.ffmpeg:
                return b'frame=2\n', dict(wall=1, user=0.5, system=0.1)
            requested = command[-1]
            limit = min(requested or 4, 4)
            data = dict(pictures=2, decoded_pictures=2, bins=5, requested_threads=requested,
                        host_threads=4, thread_limit=limit, max_memory_bytes=8888)
            if command[0] == args.native:
                data.update(parallel_pictures=2 if limit > 1 else 0,
                            frame_parallel_pictures=2 if limit > 1 else 0,
                            max_workers=min(limit, 2))
            return json.dumps(data).encode(), dict(wall=1, user=0.5, system=0.1)
        with patch.object(threading.conformance, 'digest', return_value='a' * 64), \
                patch.object(threading.reference, 'verify', return_value=dict(frames=2, depth=8)), \
                patch.object(threading, 'signature', return_value=[dict(output_md5='a')]), \
                patch.object(threading.reference, 'command', side_effect=child):
            result = threading.measure(args, oracle, pathlib.Path('/input'), 'd00')
        self.assertEqual(result['status'], 'timed')
        for call in result['calls']:
            if call['arm'].startswith('candidate_'):
                requested = int(call['arm'].split('_')[1])
                self.assertEqual(call['requested_threads'], requested)
                self.assertEqual(call['host_threads'], 4)
                self.assertEqual(call['thread_limit'], min(requested or 4, 4))
                self.assertEqual(call['frame_parallel_pictures'], 0 if requested == 1 else 2)


class ThreadDiagnostics(unittest.TestCase):
    def record(self):
        return dict(pictures=3, decoded_pictures=4, bins=55, requested_threads=4, host_threads=2,
                    thread_limit=2, max_memory_bytes=8888, parallel_pictures=2,
                    frame_parallel_pictures=2, max_workers=2,
                    thread_diagnostics=[[key, 0] for key in sorted(threading.DIAGNOSTIC_KEYS)])

    def test_requires_complete_unique_counters_and_picture_denominators(self):
        valid = self.record()
        self.assertEqual(len(threading.diagnostic_record(valid)['barrier_histogram']), 16)
        events = threading.diagnostic_record(valid)['events']
        self.assertEqual(len(events), 19)
        self.assertTrue({f'event_sei_type_{kind}' for kind in (0, 1, 5, 129, 132, 'other')} <= events.keys())
        serial = dict(valid, parallel_pictures=0, frame_parallel_pictures=0, max_workers=1)
        self.assertEqual(threading.diagnostic_record(serial)['raw']['frame_parallel_pictures'], 0)
        mutations = []
        for key in valid:
            missing = copy.deepcopy(valid)
            del missing[key]
            mutations.append(missing)
        for key, values in {
            'pictures': (0, 5, True), 'decoded_pictures': (0, 2, 33), 'bins': (0, -1, True),
            'requested_threads': (0, 1, 8), 'host_threads': (0, -1), 'thread_limit': (0, 1, 4),
            'max_workers': (0, 3), 'max_memory_bytes': (0,),
            'parallel_pictures': (-1, 5), 'frame_parallel_pictures': (-1, 1, 3, '2')
        }.items():
            for value in values:
                changed = copy.deepcopy(valid)
                changed[key] = value
                mutations.append(changed)
        for counters in ([], None, valid['thread_diagnostics'][:-1],
                         valid['thread_diagnostics'] + valid['thread_diagnostics'][:1],
                         valid['thread_diagnostics'] + [['barrier_unknown', 0]]):
            mutations.append(dict(valid, thread_diagnostics=counters))
        for value in (-1, True, '1', 1 << 64, 5):
            changed = copy.deepcopy(valid)
            changed['thread_diagnostics'][0][1] = value
            mutations.append(changed)
        for changed in mutations:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                threading.diagnostic_record(changed)
        for key in threading.DIAGNOSTIC_KEYS:
            missing = dict(valid, thread_diagnostics=[pair for pair in valid['thread_diagnostics'] if pair[0] != key])
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'diagnostic_counters_missing'):
                threading.diagnostic_record(missing)

    def test_histograms_allow_multiple_reasons_and_distinguish_events(self):
        data = self.record()
        for pair in data['thread_diagnostics']:
            pair[1] = 4 if pair[0].startswith('barrier_') else 999
        row = dict(id='d00', status='decoded', **threading.diagnostic_record(data))
        second = dict(row, id='c00')
        summary = threading.diagnostic_summary([row, second], {'d00', 'c00'})
        self.assertTrue(summary['complete'])
        self.assertEqual(summary['pictures'], 6)
        self.assertEqual(summary['decoded_pictures'], 8)
        self.assertEqual(summary['frame_parallel_pictures'], 4)
        self.assertEqual(summary['inputs_with_overlap'], 2)
        self.assertEqual(summary['barrier_histogram'], {key: 8 for key in row['barrier_histogram']})
        self.assertEqual(row['events']['event_reconcile'], 999)
        self.assertFalse(threading.diagnostic_summary([], set())['complete'])
        self.assertFalse(threading.diagnostic_summary([row, row], {'d00'})['complete'])
        self.assertEqual(threading.diagnostic_summary([row], {'d00', 'c00'})['missing_inputs'], 1)

    def run_diagnostics(self, inputs=None, output=None, dependencies=False, rows=False):
        with tempfile.TemporaryDirectory() as temp:
            out = pathlib.Path(temp) / 'out'
            oracle = types.SimpleNamespace(ffmpeg='/qualified/ffmpeg', ffprobe='/qualified/ffprobe')
            pool = types.SimpleNamespace(limits=types.SimpleNamespace(requests=0), close=lambda: None)
            selected = threading.ROW_INPUT_IDS if rows else threading.INPUT_IDS
            inputs = [(key, pathlib.Path('/' + key), 'd' * 64) for key in selected] if inputs is None else inputs
            child = types.SimpleNamespace(stdout=json.dumps(self.record()).encode())
            def result(*args, **kwargs):
                return output(*args, **kwargs) if output else child
            with patch.object(threading.sys, 'platform', 'linux'), \
                    patch.dict(threading.os.environ, GITHUB_ACTIONS='true'), \
                    patch.object(threading.test_oracle, 'require', return_value=oracle) as require, \
                    patch.object(threading.conformance, 'digest', return_value='c' * 64), \
                    patch.object(threading.oracle_pin, 'load', return_value=dict(BUILD='qualified')), \
                    patch.object(pathlib.Path, 'read_text', return_value='model name : fixture host\n'), \
                    patch.object(threading.os, 'sched_getaffinity', return_value={0, 1}, create=True), \
                    patch.object(threading.round_files, 'pool', return_value=pool), \
                    patch.object(threading.reference, 'inputs', return_value=inputs) as prepare, \
                    patch.object(threading.subprocess, 'run', side_effect=result) as run, \
                    patch.object(threading.reference, 'command', side_effect=AssertionError('timing forbidden')), \
                    patch.object(threading.reference, 'verify', side_effect=AssertionError('comparison forbidden')), \
                    patch.object(threading.reference, 'profile', side_effect=AssertionError('profiling forbidden')):
                extra = ['--dependencies-only'] if dependencies else []
                if rows:
                    extra.append('--rows-only')
                code = threading.diagnostics(['--native', '/native', '--oracle-bin', '/caller-oracle',
                                              '--out', str(out), '--candidate', 'a' * 40, *extra])
                require.assert_called_once_with(pathlib.Path('/caller-oracle/ffmpeg'), pathlib.Path('/caller-oracle/ffprobe'))
                self.assertIs(prepare.call_args.args[1], oracle)
                self.assertEqual(prepare.call_args.args[0].frames, 32)
                self.assertEqual(prepare.call_args.args[0].only, set(selected))
                self.assertEqual(pool.limits.max_requests, 4000)
                self.assertEqual(pool.limits.max_bytes, 32 << 30)
                calls = run.call_args_list
            return code, json.loads((out / 'results.json').read_text()), calls

    def test_each_existing_input_runs_once_in_decode_mode_without_timing(self):
        code, report, calls = self.run_diagnostics()
        self.assertEqual(code, 0)
        self.assertEqual(len(calls), 30)
        self.assertEqual(report['candidate'], 'a' * 40)
        self.assertEqual(report['candidate_sha256'], 'c' * 64)
        self.assertEqual(report['frames_cap'], 32)
        for key, call, row in zip(threading.INPUT_IDS, calls, report['files']):
            self.assertEqual(call.args[0], ['/native', '/' + key, 'decode', '4'])
            self.assertEqual(call.kwargs, dict(capture_output=True, check=True, timeout=600,
                                               env=threading.reference.CHILD_ENV))
            self.assertEqual(row['raw'], self.record())
            self.assertEqual(row['input_sha256'], 'c' * 64)
        self.assertEqual(report['summary']['decoded_pictures'], 120)
        self.assertEqual(report['summary']['frame_parallel_pictures'], 60)
        self.assertNotIn('wall', json.dumps(report))
        self.assertNotIn('baseline', report)

    def test_header_replay_never_reconstructs_and_keeps_the_prepared_input_binding(self):
        rows = [dict(index=i, poc=i, reconstruction_required=True, wpp=False,
                     slice_types=[2] if i == 0 else [1],
                     rps=[] if i == 0 else [[0, 0, False]],
                     active=[] if i == 0 else [[0, 0, False]]) for i in range(3)]
        data = dict(mode='header_dependencies', header_only=True, complete=True,
                    coded_pictures=3, reconstruction_required=3, output_pictures=3,
                    nals=6, pictures=rows, output_events=[
                        dict(picture_index=i, poc=i, before_picture=i + 1) for i in range(3)])
        child = lambda *a, **kw: types.SimpleNamespace(stdout=json.dumps(data).encode())
        code, report, calls = self.run_diagnostics(output=child, dependencies=True)
        self.assertEqual((code, report['mode'], report['requested_threads']), (0, 'header_dependencies', 0))
        self.assertEqual((report['summary']['parsed_inputs'], report['summary']['coded_pictures']), (30, 90))
        for key, call, row in zip(threading.INPUT_IDS, calls, report['files']):
            self.assertEqual(call.args[0], ['/native', '/' + key, 'dependencies', '1'])
            self.assertEqual(row['status'], 'parsed')
            self.assertEqual(row['input_sha256'], 'c' * 64)
            self.assertEqual(row['dependency_ceiling']['active']['coded_admission_width'], 2)
            self.assertEqual(row['dependency_ceiling']['active']['unit_duration_receive_barrier']['4'],
                             dict(steps=2, max_workers=2, overlap_pictures=2,
                                  non_wpp_overlap_pictures=2))
        del data['output_events']
        missing, invalid, _ = self.run_diagnostics(output=child, dependencies=True)
        self.assertEqual((missing, invalid['summary']['errors']), (1, 30))
        failed, bad, _ = self.run_diagnostics(dependencies=True)
        self.assertEqual((failed, bad['summary']['errors']), (1, 30))
        self.assertTrue(all(row['failure_reason'] == 'dependency_record_invalid' for row in bad['files']))

    def test_errors_missing_counters_and_empty_preparation_fail(self):
        for output in (lambda *a, **kw: types.SimpleNamespace(stdout=b'{}'),
                       lambda *a, **kw: types.SimpleNamespace(stdout=b'not json')):
            code, report, calls = self.run_diagnostics(output=output)
            self.assertEqual(code, 1)
            self.assertEqual(len(calls), 30)
            self.assertEqual(report['summary']['errors'], 30)
        def failed(*args, **kwargs):
            raise subprocess.CalledProcessError(9, args[0], stderr=b'private decode log')
        code, report, calls = self.run_diagnostics(output=failed)
        self.assertEqual(code, 1)
        self.assertEqual(report['summary']['errors'], 30)
        self.assertEqual(report['files'][0]['exit_code'], 9)
        self.assertNotIn('private decode log', json.dumps(report))
        code, report, calls = self.run_diagnostics(inputs=[])
        self.assertEqual((code, len(calls), report['summary']['missing_inputs']), (1, 0, 30))
        self.assertFalse(report['complete'])
        code, report, calls = self.run_diagnostics(inputs=[('d00', '/input', 'd' * 64)])
        self.assertEqual((code, len(calls), report['summary']['missing_inputs']), (1, 1, 29))
        code, report, calls = self.run_diagnostics(inputs=[('d00', '/input', 'd' * 64)] * 2)
        self.assertEqual((code, len(calls), report['summary']['preparation_errors']), (1, 1, 1))
        self.assertEqual(report['summary']['errors'], 1)

    def test_diagnostics_refuse_non_actions_or_non_linux_before_acquisition(self):
        for host, actions in [('darwin', 'true'), ('linux', 'false')]:
            with patch.object(threading.sys, 'platform', host), \
                    patch.dict(threading.os.environ, GITHUB_ACTIONS=actions), \
                    patch.object(threading.test_oracle, 'require') as require, \
                    self.assertRaises(SystemExit):
                threading.diagnostics(['--native', '/native', '--oracle-bin', '/oracle',
                                       '--out', '/out', '--candidate', 'a' * 40])
            require.assert_not_called()

    def test_row_and_header_selections_are_exclusive_before_acquisition(self):
        with patch.object(threading.test_oracle, 'require') as require, self.assertRaises(SystemExit):
            threading.diagnostics(['--native', '/native', '--oracle-bin', '/oracle', '--out', '/out',
                                   '--candidate', 'a' * 40, '--rows-only', '--dependencies-only'])
        require.assert_not_called()

    def test_row_selector_is_not_available_in_ordinary_mode(self):
        command = [sys.executable, '-I', str(pathlib.Path(threading.__file__)),
                   '--native', '/native', '--main', '/main', '--oracle-bin', '/oracle', '--out', '/out',
                   '--candidate', 'a' * 40, '--baseline', 'b' * 40, '--rows-only']
        result = subprocess.run(command, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 2)
        self.assertIn(b'unrecognized arguments: --rows-only', result.stderr)

    def row_record(self):
        headers = [dict(index=index, poc=index, reconstruction_required=True, wpp=False,
                        slice_types=[1] if index else [2],
                        rps=[[index - 1, index - 1, False]] if index else [],
                        active=[[index - 1, index - 1, False]] if index else []) for index in range(3)]
        header = dict(mode='header_dependencies', complete=True, header_only=True,
                      coded_pictures=3, reconstruction_required=3, output_pictures=3, nals=6,
                      pictures=headers, output_events=[dict(picture_index=i, poc=i, before_picture=i + 1)
                                                      for i in range(3)])
        pictures = [dict(index=index, poc=index, width=32, height=48, ctb_log2=4, tiles=False, wpp=False,
                         rows=[dict(row=row, sample_calls=int(index > 0), motion_calls=int(index > 0),
                                    references=[[index - 1, index - 1, row, row]] if index else [])
                               for row in range(3)]) for index in range(3)]
        return dict(mode='row_dependencies', complete=True, header_only=False, requested_threads=1,
                    thread_limit=1, max_workers=1, decoded_pictures=3, output_pictures=3,
                    header_dependencies=header, pictures=pictures)

    def test_rows_decode_exactly_the_existing_24_non_wpp_inputs_with_hashes_and_no_timing(self):
        expected = tuple([f'd{i:02}' for i in range(26) if i not in (3, 6, 7, 10, 14, 22)]
                         + [f'c{i:02}' for i in range(4)])
        self.assertEqual(threading.ROW_INPUT_IDS, expected)
        data = self.row_record()
        child = lambda *a, **kw: types.SimpleNamespace(stdout=json.dumps(data).encode())
        code, report, calls = self.run_diagnostics(output=child, rows=True)
        self.assertEqual((code, report['passed'], report['complete']), (0, True, True))
        self.assertEqual((report['mode'], report['header_only'], report['requested_threads']),
                         ('row_dependencies', False, 1))
        self.assertEqual(report['selected_ids'], list(expected))
        self.assertEqual(report['summary'], dict(selected_inputs=24, attempted_inputs=24, decoded_inputs=24,
                                               decoded_pictures=72, errors=0, missing_inputs=0,
                                               preparation_errors=0, complete=True))
        self.assertEqual(len(calls), 24)
        for key, call, row in zip(expected, calls, report['files']):
            self.assertEqual(call.args[0], ['/native', '/' + key, 'rows', '1'])
            self.assertEqual(call.kwargs, dict(capture_output=True, check=True, timeout=600,
                                               env=threading.reference.CHILD_ENV))
            self.assertEqual((row['status'], row['input_sha256'], row['source_sha256']),
                             ('decoded', 'c' * 64, 'd' * 64))
            self.assertEqual(row['raw'], data)
            self.assertEqual(row['row_dependency_ceiling'], threading.hevc_row_dependency.analyze(data))
        for field in ('baseline', 'wall', 'calls', 'warmups', 'barrier_histogram'):
            self.assertNotIn(field, report)
        self.assertIn('actual decoded inter-reference reads', report['scope'])

    def test_rows_missing_invalid_and_failed_decodes_never_receive_success_credit(self):
        valid = self.row_record()
        invalid = [{}, [], dict(valid, header_only=True), dict(valid, complete=False),
                   dict(valid, decoded_pictures=2), dict(valid, requested_threads=4)]
        missing = copy.deepcopy(valid)
        del missing['pictures'][0]['rows']
        invalid.append(missing)
        changed = copy.deepcopy(valid)
        changed['pictures'][1]['rows'][0]['references'][0][2] = 999
        invalid.append(changed)
        for data in invalid:
            child = lambda *a, **kw: types.SimpleNamespace(stdout=json.dumps(data).encode())
            with self.subTest(data=data):
                code, report, calls = self.run_diagnostics(output=child, rows=True)
                self.assertEqual((code, len(calls), report['summary']['errors'],
                                  report['summary']['decoded_inputs'], report['summary']['decoded_pictures']),
                                 (1, 24, 24, 0, 0))
                self.assertTrue(all(row['failure_reason'] == 'row_dependency_record_invalid'
                                    for row in report['files']))
        child = lambda *a, **kw: types.SimpleNamespace(stdout=json.dumps(valid).encode())
        for inputs, attempted, missing, preparation_errors in [([], 0, 24, 0),
                ([('d00', '/input', 'd' * 64)], 1, 23, 0),
                ([('d00', '/input', 'd' * 64)] * 2, 1, 23, 1),
                ([('d03', '/excluded-wpp', 'd' * 64)], 0, 24, 1)]:
            code, report, calls = self.run_diagnostics(inputs=inputs, output=child, rows=True)
            self.assertEqual((code, len(calls), report['summary']['missing_inputs'],
                              report['summary']['preparation_errors']), (1, attempted, missing, preparation_errors))
            self.assertFalse(report['complete'])
        def failed(*args, **kwargs):
            raise subprocess.TimeoutExpired(args[0], 600, stderr=b'private row decode log')
        code, report, calls = self.run_diagnostics(output=failed, rows=True)
        self.assertEqual((code, len(calls), report['summary']['errors'], report['summary']['decoded_pictures']),
                         (1, 24, 24, 0))
        self.assertNotIn('private row decode log', json.dumps(report))

    def test_row_selection_filters_wpp_before_any_source_fetch_or_preparation(self):
        roster = [dict(partition='development', sha256=f'{i + 1:064x}', size=100, ext='mp4')
                  for i in range(26)]
        manifest = json.dumps(dict(files=[dict(sha256=item['sha256'], size=100, ext='mp4', key=f'd{i:02}')
                                          for i, item in enumerate(roster)])).encode()
        pool = types.SimpleNamespace(head=lambda *a: (len(manifest), 'etag'), get=lambda *a: manifest,
                                     limits=object())
        args = types.SimpleNamespace(only=set(threading.ROW_INPUT_IDS), frames=32)
        oracle = types.SimpleNamespace(ffmpeg='/qualified/ffmpeg', ffprobe='/qualified/ffprobe')
        with tempfile.TemporaryDirectory() as temp, \
                patch.object(threading.conformance, 'roster', return_value=roster), \
                patch.object(threading.conformance, 'registration', return_value={}), \
                patch.object(threading.reference, 'CONFORMANCE_BITSTREAMS', ()), \
                patch.object(threading.reference, 'fetch', return_value=pathlib.Path(temp) / 'original') as fetch, \
                patch.object(threading.reference, 'command') as command:
            prepared = list(threading.reference.inputs(args, oracle, pathlib.Path(temp), pool))
        expected = [identity for identity in threading.ROW_INPUT_IDS if identity.startswith('d')]
        self.assertEqual([row[0] for row in prepared], expected)
        self.assertEqual(fetch.call_count, 20)
        self.assertEqual(command.call_count, 20)
        self.assertEqual([call.args[0]['sha256'] for call in fetch.call_args_list],
                         [roster[int(identity[1:])]['sha256'] for identity in expected])
        for call in command.call_args_list:
            self.assertEqual(call.args[0][0], oracle.ffmpeg)
            self.assertIn('-frames:v', call.args[0])
            self.assertEqual(call.args[0][call.args[0].index('-frames:v') + 1], '32')

    def test_workflow_diagnostic_selector_excludes_every_other_job(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        dispatch = (root / '.github/workflows/hevc-reference.yml').read_text()
        reusable = (root / '.github/workflows/hevc-threading.yml').read_text()
        def gate(workflow, job, values):
            text = workflow.split(f'\n  {job}:\n', 1)[1].splitlines()[0].strip().removeprefix('if: ')
            text = text.strip("'").removeprefix('${{ ').removesuffix(' }}')
            text = re.sub(r'inputs\.([a-z_]+)', lambda match: repr(values[match[1]]), text)
            text = re.sub(r'!(?!=)', ' not ', text).replace('&&', ' and ').replace('||', ' or ')
            return bool(eval(text))
        for inverse, diagnostics, dependencies, rows, sanitizer, baseline, only in itertools.product(
                (False, True), (False, True), (False, True), (False, True), (False, True), ('', 'b' * 40), ('', 'd00')):
            values = dict(thread_diagnostics_only=diagnostics, thread_dependencies_only=dependencies,
                          thread_rows_only=rows, sanitizer_only=sanitizer, baseline=baseline, only=only, inverse=inverse)
            ordinary = gate(dispatch, 'reference', values)
            threading_job = gate(dispatch, 'threading', values)
            self.assertEqual(ordinary, not inverse and not diagnostics and not dependencies and not rows and not sanitizer and not baseline)
            self.assertEqual(bool(threading_job), not inverse and bool(diagnostics or dependencies or rows or sanitizer or baseline))
            self.assertEqual(gate(dispatch, 'inverse-comparison', values), inverse)
            self.assertEqual(gate(reusable, 'diagnostics', values), diagnostics or dependencies or rows)
            for job in ('reference', 'conformance', 'thread-sanitizer'):
                if diagnostics or dependencies or rows:
                    self.assertFalse(gate(reusable, job, values))
        self.assertIn('thread_diagnostics_only: ${{ inputs.thread_diagnostics_only }}', dispatch)
        self.assertIn('thread_dependencies_only: ${{ inputs.thread_dependencies_only }}', dispatch)
        self.assertIn('thread_rows_only: ${{ inputs.thread_rows_only }}', dispatch)
        for workflow in (dispatch, reusable):
            self.assertRegex(workflow, r'thread_rows_only:\n(?:[^\n]+\n)*?        default: false\n        type: boolean')
        job = reusable.split('  diagnostics:', 1)[1].split('\n  reference:', 1)[0]
        self.assertEqual(job.count('uses: actions/checkout@'), 1)
        self.assertIn('ref: ${{ inputs.candidate }}\n          fetch-depth: 0', job)
        self.assertLess(job.index('git rev-parse HEAD'), job.index('cargo build'))
        self.assertLess(job.index('git branch -r --contains'), job.index('secrets.'))
        self.assertIn("--cfg hevc_thread_diagnostics --check-cfg=cfg(hevc_thread_diagnostics) --check-cfg=cfg(hevc_synchronous_baseline)", job)
        self.assertNotIn('--cfg hevc_synchronous_baseline', job)
        for forbidden in ('valgrind', 'hevc_conformance.py', 'path: baseline'):
            self.assertNotIn(forbidden, job)
        self.assertIn('test -z "$ONLY"', job)
        self.assertIn('test "$FRAMES" = 32', job)
        self.assertIn('--candidate "$CANDIDATE" "${EXTRA[@]}" > "$RUNNER_TEMP/private/diagnostics.log" 2>&1', job)
        self.assertIn('EXTRA+=(--dependencies-only)', job)
        self.assertIn('EXTRA+=(--rows-only)', job)
        self.assertIn('-p test_hevc_row_dependency.py', job)
        self.assertIn('RUSTFLAGS=\'--cfg hevc_thread_diagnostics\' cargo test --locked -p coreav-hevc --features native-hevc --lib', job)
        self.assertIn("grep -Eq '^test result: ok\\. [1-9][0-9]* passed; 0 failed;'", job)
        self.assertIn('tar -czf records.tgz private', job)
        self.assertIn('rsa_oaep_md:sha256', job)
        self.assertIn('name: hevc-threading-diagnostics-${{ inputs.candidate }}', job)
        self.assertIn('path: ${{ runner.temp }}/sealed', job)
        upload = job.split('- uses: actions/upload-artifact@')[1]
        self.assertNotIn('/private', upload)

    def test_ordinary_pr_ci_executes_cfg_tests_and_excludes_the_private_bridge_from_releases(self):
        workflow = (pathlib.Path(__file__).resolve().parents[2] / '.github/workflows/hevc-conformance.yml').read_text()
        step = workflow.split('- name: Check diagnostic-only extraction and row reads\n', 1)[1].split('\n      - ', 1)[0]
        self.assertIn("RUSTFLAGS='--cfg hevc_thread_diagnostics' cargo test", step)
        self.assertIn('-p coreav-hevc -p coreav-kernels-hevc --features native-hevc --lib', step)
        for name in ('decoder::scheduler::tests::header_only_dependency_replay_distinguishes_chains_and_siblings',
                     'decoder::scheduler::tests::row_dependency_trace_observes_generated_predictions',
                     'inter::tests::diagnostic_read_bounds_match_interpolation_support'):
            self.assertIn(f"grep -Fx 'test {name} ... ok'", step)
        self.assertIn('set -euo pipefail', step)
        self.assertIn('for feature in release-default release-full; do', workflow)
        self.assertIn('native-hevc-private|coreav-(kernels-|backend-)?hevc', workflow)

    def test_workflow_cfg_tests_require_both_row_owners_and_header_extraction(self):
        workflow = (pathlib.Path(__file__).resolve().parents[2] / '.github/workflows/hevc-threading.yml').read_text()
        step = workflow.split('- name: Check cfg-only dependency extraction and scheduling tests\n', 1)[1]
        command = step.split('        run: |\n', 1)[1].split('\n      - ', 1)[0]
        decoder = ('decoder::scheduler::tests::header_only_dependency_replay_distinguishes_chains_and_siblings',
                   'decoder::scheduler::tests::row_dependency_trace_observes_generated_predictions')
        kernel = 'inter::tests::diagnostic_read_bounds_match_interpolation_support'
        self.assertIn("RUSTFLAGS='--cfg hevc_thread_diagnostics' cargo test --locked -p coreav-hevc --features native-hevc --lib", command)
        self.assertIn("RUSTFLAGS='--cfg hevc_thread_diagnostics' cargo test --locked -p coreav-kernels-hevc --features native-hevc --lib", command)
        def log(names, count=2):
            return '\n'.join([*(f'test {name} ... ok' for name in names),
                              f'test result: ok. {count} passed; 0 failed; 200 filtered out']) + '\n'
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            cargo = root / 'cargo'
            cargo.write_text('#!/bin/bash\ncase "$*" in\n*coreav-kernels-hevc*) cat "$KERNEL_LOG"; exit "${KERNEL_EXIT:-0}";;\n*) cat "$DECODER_LOG"; exit "${DECODER_EXIT:-0}";;\nesac\n')
            cargo.chmod(0o700)
            command = command.replace('cargo test', f'"{cargo}" test')
            env = dict(PATH='/usr/bin:/bin', RUNNER_TEMP=temp, DECODER_LOG=str(root / 'decoder.log'),
                       KERNEL_LOG=str(root / 'kernel.log'))
            cases = [(log(decoder), log([kernel]), {}, True)]
            cases.extend((log([name for name in decoder if name != missing]), log([kernel]), {}, False)
                         for missing in decoder)
            cases += [(log(decoder), log(['inter::tests::unrelated']), {}, False),
                      (log(decoder, 0), log([kernel]), {}, False),
                      (log(decoder), log([kernel], 0), {}, False),
                      (log(decoder), log([kernel]), dict(DECODER_EXIT='1'), False),
                      (log(decoder), log([kernel]), dict(KERNEL_EXIT='1'), False)]
            for decoder_log, kernel_log, extra, passes in cases:
                (root / 'decoder.log').write_text(decoder_log)
                (root / 'kernel.log').write_text(kernel_log)
                result = subprocess.run(['bash', '-c', command], env=dict(env, **extra), capture_output=True, timeout=10)
                self.assertEqual(result.returncode == 0, passes, result.stdout + result.stderr)

    def test_workflow_rejects_conflicting_modes_before_credentials_and_forwards_row_flag(self):
        workflow = (pathlib.Path(__file__).resolve().parents[2] / '.github/workflows/hevc-threading.yml').read_text()
        job = workflow.split('  diagnostics:\n', 1)[1].split('\n  reference:', 1)[0]
        guard = next(line.strip() for line in job.splitlines() if line.strip().startswith('test "$DEPENDENCIES_ONLY"'))
        self.assertLess(job.index(guard), job.index('cargo test'))
        self.assertLess(job.index(guard), job.index('secrets.'))
        step = job.split('- name: Inspect each prepared input once\n', 1)[1]
        command = step.split('        run: |\n', 1)[1].split('\n      - ', 1)[0]
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            python = root / 'python'
            (root / 'private').mkdir()
            python.write_text('#!/bin/bash\nprintf "%s\\n" "$@"\n')
            python.chmod(0o700)
            command = command.replace('python3 -I candidate/scripts/hevc_threading.py', f'"{python}"')
            for dependencies, rows in itertools.product(('false', 'true'), repeat=2):
                env = dict(PATH='/usr/bin:/bin', RUNNER_TEMP=temp, CANDIDATE='a' * 40,
                           DEPENDENCIES_ONLY=dependencies, ROWS_ONLY=rows)
                result = subprocess.run(['bash', '-c', guard], env=env, capture_output=True, timeout=10)
                self.assertEqual(result.returncode == 0, (dependencies, rows) != ('true', 'true'))
                if result.returncode:
                    continue
                bash = '/opt/homebrew/bin/bash' if pathlib.Path('/opt/homebrew/bin/bash').exists() else 'bash'
                subprocess.run([bash, '-c', command], env=env, capture_output=True, check=True, timeout=10)
                args = (root / 'private/diagnostics.log').read_text().splitlines()
                expected = ['diagnostics', '--native', str(pathlib.Path.cwd() / 'candidate/target/release/examples/reference'),
                            '--oracle-bin', str(root / 'oracle'), '--out', str(root / 'private'), '--candidate', 'a' * 40]
                expected += ['--rows-only'] if rows == 'true' else ['--dependencies-only'] if dependencies == 'true' else []
                self.assertEqual(args, expected)


if __name__ == '__main__':
    unittest.main()
