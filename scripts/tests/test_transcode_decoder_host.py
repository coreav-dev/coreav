import copy
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[1] / 'transcode_decoder_host.py'
SPEC = importlib.util.spec_from_file_location('decoder_host', SCRIPT)
tool = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(tool)


class DecoderHostTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.out = Path(self.directory.name)
        (self.out / 'checks').mkdir()
        inputs = tool.roster()
        self.report = {'status': 'complete', 'inputs': inputs, 'inputs_sha256': tool.digest(tool.INPUTS),
                       'host': {'instance': {'machine_type': 'c4-standard-8', 'provisioning': 'STANDARD',
                                            'advanced_machine_features': {'threadsPerCore': 1}}},
                       'build': {'command': tool.BUILD}, 'gates': [[1, 1, 1], [1, 1, 1]],
                       'preflight': {}, 'calls': []}
        for i, file in enumerate(inputs['files']):
            sha = file['sha256']
            checks = {}
            for encoder in ('ffmpeg', 'x264'):
                path = self.out / 'checks' / (sha + '-' + encoder + '.framemd5')
                path.write_text('# synthetic fixture\n0, 0, 0, 1, 6, sample-hash\n')
                info = {'backend': 'libvpx' if file['class'] == 'vp9' else 'dav1d', 'encoder': encoder,
                        'frames': 1, 'width': 2, 'height': 2, 'isolation': 'sandbox'}
                checks[encoder] = {'sandbox': info, 'process': info | {'isolation': 'in-process'},
                                   'framemd5_sha256': tool.digest(path), 'audio_packets': []}
            self.report['preflight'][sha] = checks
            for r in range(3):
                for arm in tool.order(i, r):
                    row = {'sha256': sha, 'round': r, 'arm': arm, 'seconds': 1,
                           'invocation': tool.invocation(arm, file)}
                    if arm not in ('base', 'base2'):
                        mode, encoder = arm.split('_')
                        row['seconds'] = 1.1 if mode == 'sandbox' else 1.05
                        row['receipt'] = checks[encoder][mode]
                    self.report['calls'].append(row)

    def summary(self, report=None):
        with patch.object(tool.tc, 'bootstrap', side_effect=lambda values, **kwargs:
                          (tool.statistics.median(values), min(values), max(values)) if values else (None,) * 3):
            return tool.summarize(self.report if report is None else report, self.out)

    def test_complete_roster_and_paired_ratios(self):
        result = self.summary()
        self.assertTrue(result['diagnostic_only'])
        self.assertEqual(len(result['files']), 16)
        for cls, count in [('vp9', 10), ('av1', 6)]:
            pairs = result['classes'][cls]['pairs']
            self.assertAlmostEqual(pairs['ffmpeg']['process_over_sandbox']['median'], 1.05 / 1.1)
            self.assertEqual(pairs['x264']['process_over_sandbox']['files'], count)
            self.assertAlmostEqual(pairs['x264']['fraction_extra_seconds_removed']['median'], 0.5)

    def test_missing_duplicate_reordered_and_wrong_arm_refuse(self):
        for change in ('missing', 'duplicate', 'reordered', 'wrong-arm'):
            report = copy.deepcopy(self.report)
            if change == 'missing':
                report['calls'].pop()
            elif change == 'duplicate':
                report['calls'].append(report['calls'][-1])
            elif change == 'reordered':
                report['calls'][:2] = reversed(report['calls'][:2])
            else:
                report['calls'][0]['arm'] = 'other'
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, 'calls'):
                self.summary(report)

    def test_changed_work_and_invocations_refuse(self):
        cases = [('backend', 'fallback'), ('isolation', 'in-process'), ('frames', 0), ('width', 0)]
        for key, value in cases:
            report = copy.deepcopy(self.report)
            report['calls'][2]['receipt'][key] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.summary(report)
        report = copy.deepcopy(self.report)
        report['calls'][0]['invocation'] = ['/bin/true']
        with self.assertRaisesRegex(ValueError, 'invocation'):
            self.summary(report)

    def test_incomplete_gate_and_roster_fail_closed(self):
        for mutate in (lambda r: r.update(status='incomplete'),
                       lambda r: r['gates'].pop(),
                       lambda r: r['gates'].__setitem__(1, [1.06] * 3),
                       lambda r: r['inputs']['files'].pop(),
                       lambda r: r['gates'][0].__setitem__(0, float('nan')),
                       lambda r: r['host']['instance'].update(provisioning='SPOT'),
                       lambda r: r['build'].update(command=['cargo', 'build'])):
            report = copy.deepcopy(self.report)
            mutate(report)
            with self.assertRaises(ValueError):
                self.summary(report)

    def test_changed_pixel_evidence_and_missing_preflight_refuse(self):
        sha = self.report['inputs']['files'][0]['sha256']
        report = copy.deepcopy(self.report)
        del report['preflight'][sha]
        with self.assertRaisesRegex(ValueError, 'preflight'):
            self.summary(report)
        (self.out / 'checks' / (sha + '-ffmpeg.framemd5')).write_text('changed')
        with self.assertRaisesRegex(ValueError, 'hashes'):
            self.summary()

    def test_nonpositive_sandbox_gap_is_undefined_not_clipped(self):
        for call in self.report['calls']:
            if call['arm'].startswith('sandbox_'):
                call['seconds'] = 0.99
        result = self.summary()['classes']['vp9']['pairs']['ffmpeg']
        self.assertEqual(result['fraction_undefined_files'], 10)
        self.assertIsNone(result['fraction_extra_seconds_removed']['median'])

    def test_source_refuses_unmerged_commit_before_media(self):
        with patch.object(tool, 'text', return_value='a' * 40), \
                patch.object(tool.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, 'git')):
            with self.assertRaises(subprocess.CalledProcessError):
                tool.source()

    def test_environments_isolate_the_switch_and_credentials(self):
        with patch.dict(os.environ, {tool.SWITCH: '1', 'COREAV_STRICT_SAFE': '1',
                                     'CAPY_API_KEY': 'synthetic', 'R2_SECRET_ACCESS_KEY': 'synthetic'}):
            self.assertNotIn(tool.SWITCH, tool.environment('base'))
            self.assertEqual(tool.environment('sandbox_ffmpeg')[tool.SWITCH], '0')
            self.assertEqual(tool.environment('process_ffmpeg')[tool.SWITCH], '1')
            self.assertNotIn('CAPY_API_KEY', tool.environment('base'))
            self.assertNotIn('R2_SECRET_ACCESS_KEY', tool.environment('process_x264'))
            self.assertNotIn('COREAV_STRICT_SAFE', tool.environment('base'))

    def test_failed_admission_retains_sanitized_incomplete_report(self):
        args = tool.argparse.Namespace(out=str(self.out), tmp=str(self.out / 'tmp'))
        with patch.object(tool, 'source', side_effect=ValueError('private diagnostic marker')):
            with self.assertRaises(ValueError):
                tool.run(args)
        capture = (self.out / 'capture.json').read_text()
        self.assertNotIn('private diagnostic marker', capture)
        self.assertEqual(json.loads(capture)['failure'], {'stage': 'admission', 'kind': 'ValueError'})

    def test_bootstrap_is_repeatable(self):
        self.assertEqual(tool.stats([1, 2, 3]), tool.stats([1, 2, 3]))

    def test_preflight_checks_pixels_and_audio_before_timing(self):
        file = self.report['inputs']['files'][0]
        info = {'stream_index': file['maps']['video'], 'backend': 'libvpx', 'frames': 1,
                'width': 2, 'height': 2}

        def execute(command, arm):
            encoder = arm.split('_')[1]
            isolation = 'in-process' if arm.startswith('process_') else 'sandbox'
            payload = {'video': info | {'encoder': encoder}, 'audio': []}
            return 1, subprocess.CompletedProcess(command, 0, json.dumps(payload).encode(),
                                                   ('coreav-diagnostic-decoder-host: ' + isolation).encode())

        executor = tool.Executor(30)
        executor.call = execute
        path = self.out / 'synthetic-input'
        path.write_bytes(b'not media')
        frames = '# synthetic fixture\n0, 0, 0, 1, 6, sample-hash\n'
        with patch.object(tool, 'output_check', return_value=(frames, [])):
            result = tool.preflight(executor, file, path, {name: self.out / name for name in ('ffmpeg', 'ffprobe', 'x264')}, self.out, self.out, self.out / 'checks')
            self.assertEqual(result['ffmpeg']['process']['isolation'], 'in-process')
        for changed in [(frames + '0, 1, 1, 1, 6, other\n', []),
                        (frames.replace('sample-hash', 'changed-hash'), []),
                        (frames, [{'data_hash': 'changed-audio'}])]:
            with patch.object(tool, 'output_check', side_effect=[(frames, []), changed]):
                with self.assertRaises(ValueError):
                    tool.preflight(executor, file, path, {name: self.out / name for name in ('ffmpeg', 'ffprobe', 'x264')}, self.out, self.out, self.out / 'checks')

    def test_timeout_prevents_a_successful_call(self):
        with patch.object(tool.subprocess, 'run', side_effect=subprocess.TimeoutExpired('synthetic', 1)):
            with self.assertRaises(subprocess.TimeoutExpired):
                tool.Executor(1).call(['/bin/true'], 'base')

    def test_run_allowance_defaults_to_five_hours_and_accepts_six(self):
        flags = [value for name in ('files', 'gate', 'tools', 'build', 'instance', 'out', 'tmp')
                 for value in ('--' + name, 'unused')]
        for extra, expected in [([], 18000), (['--max-seconds', '21600'], 21600)]:
            with patch.object(tool.sys, 'argv', [str(SCRIPT), 'run', *flags, *extra]), \
                    patch.object(tool, 'run') as capture:
                tool.main()
                self.assertEqual(capture.call_args.args[0].max_seconds, expected)
        self.assertEqual(tool.MAX_RUN_SECONDS, 21600)

    def test_run_allowance_refuses_above_six_hours_before_capture(self):
        flags = [value for name in ('files', 'gate', 'tools', 'build', 'instance', 'out', 'tmp')
                 for value in ('--' + name, 'unused')]
        for seconds in ('21601', '0', '-1'):
            with patch.object(tool.sys, 'argv', [str(SCRIPT), 'run', *flags, '--max-seconds', seconds]), \
                    patch.object(tool, 'run') as capture, contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    tool.main()
                self.assertEqual(error.exception.code, 2)
                capture.assert_not_called()
            with self.assertRaises(ValueError):
                tool.run_allowance(seconds)


if __name__ == '__main__':
    unittest.main()
