import collections
import hashlib
import json
import os
import pathlib
import shutil
import struct
import sys
import tempfile
import types
import unittest
from unittest import mock
import wave
import zlib

ROOT = pathlib.Path(__file__).resolve().parents[3]
HERE = ROOT / 'tools' / 'compat-corpus'
sys.path.insert(0, str(HERE))
import current_ci
import decoded_output


class CurrentPolicy(unittest.TestCase):
    def run_policy(self, code, stdout=b'', stderr=b'', result=('verified', 'exact'), event=None, modify=False):
        row = {'tool': 'ffmpeg', 'label': 'out-of-scope',
               'args': ['-i', 'INPUT', '-f', 's16le', '-acodec', 'pcm_s16le', 'OUTPUT']}
        legacy = types.SimpleNamespace(load_commands=lambda: [row], ENV={}, HELP_QUERIES=[])
        records, failures = [], []
        def invoke(argv, env, cwd):
            if modify:
                pathlib.Path(argv[argv.index('-i') + 1]).write_bytes(b'changed')
            if event is not None:
                pathlib.Path(env['COREAV_FALLBACK_LOG']).write_text(json.dumps(event) + '\n')
            if code == 0:
                (cwd / 'output.mp4').write_bytes(b'not enough to prove an output')
            return code, stdout, stderr
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(decoded_output, 'invoke', side_effect=invoke), \
                mock.patch.object(current_ci, 'verify', return_value=result) as verify:
            fixture = pathlib.Path(tmp) / 'fixture.wav'
            fixture.write_bytes(b'unchanged input')
            current_ci.refusals(legacy, types.SimpleNamespace(coreav='stand-in'), [fixture],
                                collections.Counter(), failures, records, 'ffmpeg', 'ffprobe')
            self.assertEqual(fixture.read_bytes(), b'unchanged input')
        return records[0], failures, verify.call_count

    def test_clean_refusal_and_verified_success_pass_on_either_side_of_a_feature_merge(self):
        for code, err, status in ((1, b'rejected', 'refused'), (2, b'usage', 'refused'),
                                  (4, b'unsupported', 'refused'), (0, b'', 'verified')):
            record, failures, calls = self.run_policy(code, stderr=err)
            self.assertEqual(record['status'], status)
            self.assertEqual(failures, [])
            self.assertEqual(calls, int(code == 0))

    def test_a_wrong_output_success_fails(self):
        record, failures, _ = self.run_policy(0, result=('failed', 'decoded samples'))
        self.assertEqual(record['status'], 'failed')
        self.assertEqual(len(failures), 1)

    def test_an_unverified_success_never_counts(self):
        record, failures, _ = self.run_policy(0, result=('unverified', 'no_oracle: PNG/BMP colour conversion'))
        self.assertEqual(record['status'], 'unverified')
        self.assertEqual(len(failures), 1)

    def test_crashes_and_timeouts_do_not_become_refusals(self):
        for code in (-11, -6, 3, 101, 134, 139, None):
            record, failures, calls = self.run_policy(code, stderr=b'a diagnostic')
            self.assertEqual(record['status'], 'failed')
            self.assertEqual(len(failures), 1)
            self.assertEqual(calls, 0)

    def test_empty_error_is_not_a_clean_refusal(self):
        self.assertEqual(self.run_policy(4)[0]['status'], 'failed')

    def test_input_mutation_fails_and_cannot_modify_the_fixture(self):
        record, failures, calls = self.run_policy(4, stderr=b'unsupported', modify=True)
        self.assertEqual(record['reason'], 'modified input')
        self.assertEqual(len(failures), 1)
        self.assertEqual(calls, 0)

    def test_refusal_logs_are_allowed_but_fallback_and_success_logs_are_not(self):
        refusal = {'ran': None, 'reason': 'unsupported'}
        self.assertEqual(self.run_policy(4, stderr=b'unsupported', event=refusal)[0]['status'], 'refused')
        self.assertEqual(self.run_policy(4, stderr=b'unsupported', event={'ran': 'ffmpeg'})[0]['status'], 'failed')
        self.assertEqual(self.run_policy(0, event=refusal)[0]['status'], 'failed')

    def test_a_baseline_cannot_hide_a_current_policy_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            report = pathlib.Path(tmp) / 'report.json'
            def commands(opts):
                counts, failures = legacy.ffmpeg_refusals(opts, [], collections.Counter(), [])
                report.write_text(json.dumps({'B1 ffmpeg': dict(counts), 'failures': failures}))
                return 0
            legacy = types.SimpleNamespace(cmd_commands=commands)
            def refusals(*args):
                args[5].append({'status': 'unverified'})
                return collections.Counter(fail=1), []
            with mock.patch.object(sys, 'argv', ['current_ci.py', 'commands', '--coreav', 'candidate',
                                                '--report', str(report), '--baseline', 'baseline.json']), \
                    mock.patch.object(current_ci, 'reporter', return_value=legacy), \
                    mock.patch.object(current_ci, 'refusals', side_effect=refusals), \
                    mock.patch.object(current_ci.shutil, 'which', side_effect=lambda n: '/oracle/' + n), \
                    mock.patch.object(current_ci.oracle_pin, 'verify_bin', return_value=[]):
                self.assertEqual(current_ci.main(), 1)

    def test_the_frozen_judging_files_keep_their_registered_hashes(self):
        registration = json.loads((ROOT / 'docs/measurements/compat-0.1.0/judging.json').read_text())
        self.assertIn('tools/compat-corpus/run.py', registration['files'])
        for name in ('tools/compat-corpus/run.py', '.github/workflows/compat-0.1.0.yml'):
            with self.subTest(path=name):
                self.assertEqual(hashlib.sha256((ROOT / name).read_bytes()).hexdigest(), registration['files'][name])

    def test_current_ci_is_not_used_by_the_frozen_workflow(self):
        workflow = (ROOT / '.github/workflows/compat-0.1.0.yml').read_text()
        self.assertNotIn('current_ci.py', workflow)
        self.assertNotIn('decoded_output.py', workflow)

    def test_every_current_fixture_refusal_call_uses_the_current_adapter(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertEqual(workflow.count('tools/compat-corpus/current_ci.py commands'), 3)
        self.assertNotIn('tools/compat-corpus/run.py commands --coreav', workflow)
        self.assertEqual(workflow.count('tools/compat-corpus/run.py commands --set 0.1.0'), 3)


class DecodedSamples(unittest.TestCase):
    def test_frame_probes_use_the_shared_audio_input_options(self):
        with mock.patch.object(decoded_output, 'invoke', return_value=(0, b'{"streams":[{}]}', b'')) as invoke:
            self.assertIsNotNone(decoded_output.probe('media', 'ffprobe', {}))
        args = invoke.call_args.args[0]
        self.assertEqual(args[args.index('-i') - len(decoded_output.AUDIO_DECODE_INPUT):args.index('-i')],
                         list(decoded_output.AUDIO_DECODE_INPUT))

    def test_a_selected_encode_does_not_replace_the_copy_stream_oracle(self):
        stream = {'index': 1, 'codec_type': 'audio', 'codec_name': 'flac', 'sample_fmt': 's16',
                  'sample_rate': '48000', 'channels': 1}
        listing = {'streams': [{'index': 0, 'codec_type': 'video', 'codec_name': 'h264'}, stream], 'frames': []}
        def decode(args, env, output):
            self.assertIn('0:1', args)
            output.write_bytes(struct.pack('<i', 256))
            return 0, b'', b''
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(decoded_output, 'probe', return_value=listing), \
                mock.patch.object(decoded_output, 'invoke', side_effect=decode):
            self.assertEqual(decoded_output.compare('a', 'b', 'ffmpeg', 'ffprobe', {}, pathlib.Path(tmp), stream=1)[0],
                             'verified')
            self.assertEqual(decoded_output.compare('a', 'b', 'ffmpeg', 'ffprobe', {}, pathlib.Path(tmp))[0],
                             'unverified')

    def test_timed_out_programs_are_killed_and_reaped(self):
        popen, children = decoded_output.subprocess.Popen, []
        def launch(*args, **kwargs):
            child = popen(*args, **kwargs)
            children.append(child)
            return child
        with mock.patch.object(decoded_output, 'SECONDS', 0.1), \
                mock.patch.object(decoded_output.subprocess, 'Popen', side_effect=launch):
            result = decoded_output.invoke([sys.executable, '-c', 'import time; time.sleep(60)'], os.environ)
        self.assertEqual(result[0], None)
        self.assertIsNotNone(children[0].returncode)

    def compare_audio(self, left, right):
        stream = {'codec_type': 'audio', 'codec_name': 'flac', 'sample_fmt': 's32',
                  'sample_rate': '48000', 'channels': 1, 'bits_per_raw_sample': '24'}
        listing = {'streams': [stream], 'frames': []}
        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp)
            def decode(argv, env, output):
                output.write_bytes(left if argv[argv.index('-i') + 1] == 'candidate' else right)
                return 0, b'', b''
            with mock.patch.object(decoded_output, 'probe', return_value=listing), \
                    mock.patch.object(decoded_output, 'invoke', side_effect=decode):
                return decoded_output.compare('candidate', 'reference', 'ffmpeg', 'ffprobe', {}, directory)

    def test_the_last_significant_sample_bit_is_not_lost(self):
        self.assertEqual(self.compare_audio(struct.pack('<i', 256), struct.pack('<i', 512))[0], 'failed')
        self.assertEqual(self.compare_audio(struct.pack('<i', 256), struct.pack('<i', 256))[0], 'verified')

    def test_candidate_format_mismatches_fail_instead_of_becoming_unverified(self):
        expected = {'streams': [{'codec_type': 'audio', 'codec_name': 'flac', 'sample_fmt': 's16',
                                  'sample_rate': '48000', 'channels': 1}], 'frames': []}
        actual = {'streams': [{**expected['streams'][0], 'sample_rate': '16000'}], 'frames': []}
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.object(decoded_output, 'probe', side_effect=[expected, actual]):
            result = decoded_output.compare('candidate', 'reference', 'ffmpeg', 'ffprobe', {}, pathlib.Path(tmp))
        self.assertEqual(result, ('failed', 'stream fields or timing'))

    def test_raw_description_follows_the_selected_stream_not_output_length(self):
        source = {'streams': [{'index': 0, 'codec_type': 'audio', 'sample_rate': '44100', 'channels': 1},
                              {'index': 1, 'codec_type': 'audio', 'sample_rate': '48000', 'channels': 2}]}
        args = ['-i', 'INPUT', '-f', 's16le', '-map', '0:0', 'OUTPUT']
        self.assertEqual(decoded_output.raw_inputs(args, source),
                         ['-f', 's16le', '-sample_rate', '44100', '-ch_layout', 'mono'])
        self.assertEqual(decoded_output.raw_inputs(['-i', 'INPUT', '-f', 's16le', 'OUTPUT'], source),
                         ['-f', 's16le', '-sample_rate', '48000', '-ch_layout', 'stereo'])


@unittest.skipUnless(os.environ.get('COREAV_CURRENT_CI_ORACLE') == '1', 'pinned Linux oracle runs in fixture CI')
class PinnedOracle(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.ffmpeg, cls.ffprobe = shutil.which('ffmpeg'), shutil.which('ffprobe')
        if not cls.ffmpeg or not cls.ffprobe:
            raise AssertionError('fixture CI must install the pinned oracle')
        problems = list(current_ci.oracle_pin.verify_bin(pathlib.Path(cls.ffmpeg).parent))
        if problems:
            raise AssertionError(problems)
        cls.env = {'PATH': os.environ.get('PATH', ''), 'GCONV_PATH': '/nonexistent/gconv'}

    def wav(self, path, samples, junk=False):
        with wave.open(str(path), 'wb') as writer:
            writer.setparams((1, 2, 48000, 0, 'NONE', 'not compressed'))
            writer.writeframes(struct.pack('<' + 'h' * len(samples), *samples))
        if junk:
            data = path.read_bytes()
            extra = b'JUNK' + struct.pack('<I', 4) + b'test'
            path.write_bytes(data[:4] + struct.pack('<I', len(data) - 8 + len(extra)) + data[8:12] + extra + data[12:])

    def png(self, path, pixel, level):
        def chunk(kind, data):
            return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
        header = struct.pack('>IIBBBBB', 1, 1, 8, 2, 0, 0, 0)
        path.write_bytes(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', header)
                         + chunk(b'IDAT', zlib.compress(b'\0' + pixel, level)) + chunk(b'IEND', b''))

    def test_byte_different_audio_passes_but_one_wrong_sample_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = pathlib.Path(tmp)
            left, right = work / 'left.wav', work / 'right.wav'
            self.wav(left, [1, 2, 3], junk=True)
            self.wav(right, [1, 2, 3])
            self.assertNotEqual(left.read_bytes(), right.read_bytes())
            self.assertEqual(decoded_output.compare(left, right, self.ffmpeg, self.ffprobe, self.env, work)[0], 'verified')
            self.wav(left, [1, 2, 4], junk=True)
            self.assertEqual(decoded_output.compare(left, right, self.ffmpeg, self.ffprobe, self.env, work),
                             ('failed', 'decoded samples'))

    def test_raw_audio_success_is_verified_but_one_wrong_sample_fails(self):
        for channels in (1, 2):
            with self.subTest(channels=channels), tempfile.TemporaryDirectory() as tmp:
                work = pathlib.Path(tmp)
                candidate, reference, scratch = (work / name for name in ('candidate', 'reference', 'scratch'))
                for directory in (candidate, reference, scratch):
                    directory.mkdir()
                source = work / 'input.wav'
                samples = [1, 2, 3, 4, 5, 6, 7, 8]
                pcm = struct.pack('<8h', *samples)
                with wave.open(str(source), 'wb') as writer:
                    writer.setparams((channels, 2, 48000, 0, 'NONE', 'not compressed'))
                    writer.writeframes(pcm)
                output = candidate / 'output.mp4'
                output.write_bytes(pcm)
                args = ['-i', 'INPUT', '-f', 's16le', '-acodec', 'pcm_s16le', 'OUTPUT']
                self.assertEqual(current_ci.verify(args, source, candidate, reference, self.env,
                                                    self.ffmpeg, self.ffprobe, scratch)[0], 'verified')
                for path in reference.iterdir():
                    path.unlink()
                output.write_bytes(struct.pack('<8h', *samples[:-1], 9))
                self.assertEqual(current_ci.verify(args, source, candidate, reference, self.env,
                                                    self.ffmpeg, self.ffprobe, scratch),
                                 ('failed', 'decoded samples'))

    def test_byte_different_images_pass_but_one_wrong_pixel_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = pathlib.Path(tmp)
            left, right = work / 'left.png', work / 'right.png'
            self.png(left, b'\x01\x02\x03', 0)
            self.png(right, b'\x01\x02\x03', 9)
            self.assertNotEqual(left.read_bytes(), right.read_bytes())
            self.assertEqual(decoded_output.compare(left, right, self.ffmpeg, self.ffprobe, self.env, work)[0], 'verified')
            self.png(left, b'\x01\x02\x04', 0)
            self.assertEqual(decoded_output.compare(left, right, self.ffmpeg, self.ffprobe, self.env, work),
                             ('failed', 'decoded pixels'))


if __name__ == '__main__':
    unittest.main()
