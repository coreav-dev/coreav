import copy
import json
import pathlib
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
    def test_threaded_conformance_rejects_silent_serial_fallback(self):
        serial = dict(complete=True, summary=dict(decoded_pictures=3, parallel_pictures=0, max_workers=1,
                                                stats=dict(wpp_restarts=2)))
        self.assertEqual(hevc_decode.threading_coverage([serial], 1), dict(parallel_pictures=0, max_workers=1))
        for threads in (2, 4, 8):
            with self.assertRaisesRegex(ValueError, 'native_parallel_coverage_missing'):
                hevc_decode.threading_coverage([serial], threads)
        parallel = copy.deepcopy(serial)
        parallel['summary'].update(parallel_pictures=2, max_workers=2)
        self.assertEqual(hevc_decode.threading_coverage([serial, parallel], 4),
                         dict(parallel_pictures=2, max_workers=2))
        parallel['summary']['stats']['wpp_restarts'] = 0
        with self.assertRaisesRegex(ValueError, 'native_parallel_wpp_work'):
            hevc_decode.threading_coverage([parallel], 4)

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


if __name__ == '__main__':
    unittest.main()
