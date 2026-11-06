import importlib.util
from pathlib import Path
import random
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / 'audio_source_diagnostic.py'
SPEC = importlib.util.spec_from_file_location('audio_source_diagnostic', SCRIPT)
diagnostic = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(diagnostic)


def pcm(values):
    return struct.pack('<' + 'f' * len(values), *values)


class AudioSourceDiagnostic(unittest.TestCase):
    def test_main_passes_the_owner_returned_pair_to_diagnosis(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            qualified = diagnostic.test_oracle.Oracle(root / 'encoder', root / 'separate-probe')
            for path in (root / 'coreav', qualified.ffmpeg, qualified.ffprobe):
                path.write_bytes(b'controlled identity')
            (root / 'MANIFEST').write_text('generated browser.webm\n')
            output = root / 'out'
            argv = ['diagnostic', '--coreav', str(root / 'coreav'), '--ffmpeg', 'original',
                    '--fixtures', str(root), '--out', str(output)]
            commands = []

            def run(command, **kwargs):
                commands.append(command)
                if command[0] == qualified.ffprobe:
                    raise RuntimeError('probe boundary')
                return b'controlled version\n'

            with mock.patch.object(sys, 'argv', argv), \
                    mock.patch.object(diagnostic.test_oracle, 'require', return_value=qualified) as require, \
                    mock.patch.object(diagnostic, 'run', side_effect=run):
                with self.assertRaisesRegex(RuntimeError, 'probe boundary'):
                    diagnostic.main()
            require.assert_called_once_with(Path('original'))
            self.assertEqual([c[0] for c in commands], [qualified.ffmpeg] * 3 + [qualified.ffprobe])
            report = diagnostic.json.loads((output / 'report.json').read_text())
            self.assertEqual(report['ffprobe_sha256'], diagnostic.hashlib.sha256(b'controlled identity').hexdigest())

    def test_command_failures_retain_a_bounded_stderr_tail(self):
        self.assertEqual(diagnostic.run([sys.executable, '-I', '-c', 'print("ok")']), b'ok\n')
        command = [sys.executable, '-I', '-c',
                   'import sys; sys.stderr.write("discarded-prefix" + "x" * 5000 + "failure-marker"); sys.exit(7)']
        with self.assertRaisesRegex(RuntimeError, 'failure-marker') as raised:
            diagnostic.run(command)
        self.assertIsInstance(raised.exception.__cause__, subprocess.CalledProcessError)
        self.assertNotIn('discarded-prefix', str(raised.exception))
        self.assertLess(len(str(raised.exception)), 4300)

    def test_fixture_and_metadata_failures_leave_an_incomplete_report(self):
        for failure in ['fixture', 'version', 'hash']:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                for program in ['coreav', 'ffmpeg', 'ffprobe']:
                    (root / program).write_bytes(b'controlled tool identity')
                (root / 'MANIFEST').write_text('generated browser.webm\n')
                output = root / 'report'
                if failure == 'hash':
                    (root / 'coreav').unlink()

                def failing_run(command, **kwargs):
                    if (failure == 'fixture' and 'lavfi' in command
                            or failure == 'version' and '-version' in command):
                        raise RuntimeError('controlled preparation failure')
                    return b'controlled tool version\n'

                argv = ['audio_source_diagnostic.py', '--coreav', str(root / 'coreav'),
                        '--ffmpeg', str(root / 'ffmpeg'), '--fixtures', str(root), '--out', str(output)]
                with mock.patch.object(sys, 'argv', argv), mock.patch.object(diagnostic, 'run', failing_run), \
                        mock.patch.object(diagnostic.test_oracle, 'require', side_effect=lambda path:
                                          diagnostic.test_oracle.Oracle(path, path.with_name('ffprobe'))):
                    with self.assertRaises((RuntimeError, FileNotFoundError)):
                        diagnostic.main()
                report = diagnostic.json.loads((output / 'report.json').read_text())
                self.assertFalse(report['completed'])
                self.assertTrue(report['diagnostic_only'])
                self.assertEqual(report['results'], [])

    def test_workflow_diagnostic_survives_test_failure_but_not_a_missing_build(self):
        workflow = SCRIPT.parents[1] / '.github/workflows/ci.yml'
        job = workflow.read_text().split('\n  isobmff-decode:\n', 1)[1].split('\n  transcode-compat-shard:\n', 1)[0]
        self.assertTrue('id: decode-build\n' in job, 'Give the diagnostic prerequisite build id: decode-build')
        step = job.split('- name: Observe decoder and AAC encoder boundaries on generated fixtures\n', 1)[1]
        step = step.split('\n      - ', 1)[0]
        self.assertTrue("if: ${{ !cancelled() && steps.decode-build.outcome == 'success' }}" in step,
                        'Run the diagnostic unless cancelled, only after a successful decode-build')

    def test_equal_pcm_and_counts(self):
        raw = pcm(range(64))
        result = diagnostic.compare_pcm(raw, raw, max_lag=4, window=16)
        self.assertTrue(result['equal'])
        self.assertEqual(result['coreav_samples'], 64)
        self.assertEqual(result['ffmpeg_samples'], 64)
        self.assertEqual(result['max_absolute_difference'], 0)
        self.assertIsNone(result['zero_lag_snr_db'])
        self.assertEqual(result['lag_peaks'][0]['lag'], 0)

    def test_lag_direction_and_unequal_lengths_are_reported(self):
        rng = random.Random(7)
        reference = [rng.uniform(-1, 1) for _ in range(80)]
        result = diagnostic.compare_pcm(pcm([0] * 3 + reference), pcm(reference), max_lag=4, window=32)
        self.assertFalse(result['equal'])
        self.assertEqual(result['coreav_samples'], 83)
        self.assertEqual(result['ffmpeg_samples'], 80)
        self.assertEqual(result['lag_peaks'][0]['lag'], 3)

    def test_silence_and_nonfinite_pcm_are_not_false_agreement(self):
        result = diagnostic.compare_pcm(pcm([0] * 64), pcm([0] * 64), max_lag=4, window=16)
        self.assertTrue(result['reference_silent'])
        self.assertEqual(result['lag_peaks'], [])
        with self.assertRaises(ValueError):
            diagnostic.compare_pcm(pcm([float('nan')] * 64), pcm([0] * 64), max_lag=4, window=16)

    def test_owned_f32_wav_data_is_not_redecoded(self):
        raw = pcm([0.25, -0.5])
        fmt = struct.pack('<HHIIHH', 3, 1, 48_000, 192_000, 4, 32)
        chunks = b'fmt ' + struct.pack('<I', len(fmt)) + fmt + b'data' + struct.pack('<I', len(raw)) + raw
        wav = b'RIFF' + struct.pack('<I', len(chunks) + 4) + b'WAVE' + chunks
        self.assertEqual(diagnostic.wav_pcm(wav, 48_000), raw)
        fmt = (struct.pack('<HHIIHHHHI', 0xfffe, 1, 48_000, 192_000, 4, 32, 22, 32, 4)
               + bytes.fromhex('0300000000001000800000aa00389b71'))
        extended = b'fmt ' + struct.pack('<I', len(fmt)) + fmt + b'data' + struct.pack('<I', len(raw)) + raw
        extended = b'RIFF' + struct.pack('<I', len(extended) + 4) + b'WAVE' + extended
        self.assertEqual(diagnostic.wav_pcm(extended, 48_000), raw)
        with self.assertRaises(ValueError):
            diagnostic.wav_pcm(wav, 44_100)
        with self.assertRaises(ValueError):
            diagnostic.wav_pcm(wav[:-1], 48_000)


if __name__ == '__main__':
    unittest.main()
