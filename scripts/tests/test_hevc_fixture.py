import copy
import importlib.util
import json
import os
import pathlib
import struct
import sys
import tempfile
import unittest
from unittest import mock

PATH = pathlib.Path(__file__).resolve().parents[1] / 'hevc_fixture.py'
sys.path.insert(0, str(PATH.parent))
SPEC = importlib.util.spec_from_file_location('hevc_fixture', PATH)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


def escape(value):
    output = bytearray()
    zeros = 0
    for byte in value:
        if zeros == 2 and byte <= 3:
            output.append(3)
            zeros = 0
        output.append(byte)
        zeros = zeros + 1 if byte == 0 else 0
    return bytes(output)


def pictures():
    result = bytearray()
    for n in range(2):
        for size in (64, 32, 32):
            for i in range(size * size):
                result.extend(struct.pack('<H', (i * 7 + n * 13) % 1024))
    return bytes(result)


def stream(raw):
    result = bytearray()
    for picture in runner.raw_metadata(raw)['pictures']:
        result.extend(b'\x00\x00\x00\x01\x26\x01\x80')
        payload = b'\x00' + b''.join(bytes.fromhex(h) for h in picture['coded_component_md5'])
        result.extend(b'\x00\x00\x01\x50\x01' + escape(b'\x84\x31' + payload + b'\x80'))
    return bytes(result)


class HashContract(unittest.TestCase):
    def test_cli_executes_the_exact_owner_returned_paths(self):
        pair = runner.test_oracle.Oracle(pathlib.Path('/qualified/original/../ffmpeg'),
                                         pathlib.Path('/qualified/original/../ffprobe'))
        commands = []
        def capture(command, **_):
            commands.append(command)
            if '-version' not in command:
                raise RuntimeError('stop before fixture generation')
            name = pathlib.Path(command[0]).name
            return f'{name} version {runner.oracle_pin.load()["BUILD"]}\n'.encode(), ''
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(sys, 'argv', ['hevc_fixture.py', 'prepare', '--ffmpeg', '/original/ffmpeg',
                    '--ffprobe', '/original/ffprobe', '--output', directory]), \
                mock.patch.object(runner.test_oracle, 'require', return_value=pair), \
                mock.patch.object(runner, 'file_sha256', return_value='a' * 64), \
                mock.patch.object(runner, 'capture', side_effect=capture):
            with self.assertRaisesRegex(RuntimeError, 'stop before fixture'):
                runner.main()
        self.assertEqual([command[0] for command in commands],
                         [str(pair.ffmpeg), str(pair.ffprobe), str(pair.ffmpeg)])

    def test_positive_component_hashes_use_low_byte_first_and_do_not_hash_the_whole_frame(self):
        raw = pictures()
        metadata = runner.raw_metadata(raw)
        self.assertEqual(runner.verify_hash_sei(stream(raw), raw),
                         [p['coded_component_md5'] for p in metadata['pictures']])
        for picture in metadata['pictures']:
            self.assertTrue(all(h != picture['visible_yuv_md5'] for h in picture['coded_component_md5']))
        swapped = b''.join(raw[i:i + 2][::-1] for i in range(0, len(raw), 2))
        with self.assertRaises(ValueError):
            runner.verify_hash_sei(stream(raw), swapped)

    def test_changed_ten_bit_samples_fail_crossed_hashes(self):
        raw = pictures()
        changed = bytearray(raw)
        changed[1] ^= 1
        self.assertNotEqual(runner.raw_metadata(raw), runner.raw_metadata(changed))
        runner.reject_crossed_hashes(stream(raw), changed)
        runner.reject_crossed_hashes(stream(changed), raw)

    def test_altered_raw_byte_and_altered_embedded_hash_are_rejected(self):
        raw = pictures()
        data = stream(raw)
        altered = bytearray(raw)
        altered[0] ^= 1
        runner.reject_crossed_hashes(data, altered)
        digest = bytes.fromhex(runner.picture_hashes(data)[0][0])
        altered = data.replace(digest, bytes([digest[0] ^ 1]) + digest[1:], 1)
        runner.reject_crossed_hashes(altered, raw)

    def test_positive_pair_cannot_be_accepted_as_negative_control(self):
        with self.assertRaisesRegex(ValueError, 'unexpectedly matched'):
            runner.reject_crossed_hashes(stream(pictures()), pictures())

    def test_negative_control_must_fail_on_hashes_not_geometry_or_depth(self):
        with self.assertRaisesRegex(ValueError, 'reason other than hash disagreement'):
            runner.reject_crossed_hashes(stream(pictures()), b'bad')

    def test_raw_count_high_bits_and_reduced_depth_fail(self):
        for raw in (pictures()[:-2], pictures() + b'\x00\x00',
                    b'\xff\xff' * (runner.FRAME_BYTES),
                    b'\x00\x00' * (runner.FRAME_BYTES),
                    b'\x00\x01' * (runner.FRAME_BYTES)):
            with self.subTest(size=len(raw)), self.assertRaises(ValueError):
                runner.raw_metadata(raw)

    def test_missing_duplicate_and_non_md5_sei_fail(self):
        data = stream(pictures())
        for damaged in (b'\x00\x00\x01\x26\x01\x80' * 2,
                        data.replace(b'\x84\x31\x00', b'\x84\x31\x01', 1),
                        data + data[data.rfind(b'\x00\x00\x01\x50\x01'):],
                        data[:-1]):
            with self.subTest(data=damaged[:16]), self.assertRaises(ValueError):
                runner.picture_hashes(damaged)

    def test_not_an_idr_not_first_slice_wrong_layer_and_bad_framing_fail(self):
        data = stream(pictures())
        for damaged in (b'prefix' + data, data.replace(b'\x26\x01\x80', b'\x02\x01\x80', 1),
                        data.replace(b'\x26\x01\x80', b'\x26\x01\x00', 1),
                        data.replace(b'\x26\x01\x80', b'\x27\x01\x80', 1),
                        data.replace(b'\x26\x01\x80', b'\x26\x02\x80', 1)):
            with self.subTest(data=damaged[:16]), self.assertRaises(ValueError):
                runner.picture_hashes(damaged)

    def test_emulation_prevention_and_extended_sei_header(self):
        data = b'\x00\x00\x00\x01\x00\x00\x02\x00\x00\x03'
        self.assertEqual(runner.rbsp(escape(data)), data)
        for bad in (b'\x00\x00\x03', b'\x00\x00\x03\x04'):
            with self.assertRaises(ValueError):
                runner.rbsp(bad)
        raw = pictures()
        original = stream(raw)
        extended = original.replace(b'\x50\x01\x84\x31', b'\x50\x01\xff\x00\x01\x42\x84\x31', 1)
        self.assertEqual(runner.verify_hash_sei(original, raw), runner.verify_hash_sei(extended, raw))


class Registration(unittest.TestCase):
    def test_registration_binds_all_commands_binary_identities_and_hash_scopes(self):
        record = json.loads(runner.REGISTRATION.read_text())
        self.assertEqual(record['recipes'], runner.recipes())
        self.assertEqual(record['decode_command'], runner.decode_command())
        self.assertEqual(len(record['fixtures']), 4)
        self.assertEqual(record['purpose'], 'reference_preparation_only_not_a_CoreAV_G2_verdict')
        self.assertTrue(all(record['controls'].values()))
        for name in ('ffmpeg', 'ffprobe'):
            self.assertRegex(record['oracle'][name]['sha256'], r'^[0-9a-f]{64}$')
        for fixture, recipe in zip(record['fixtures'], runner.recipes()):
            self.assertEqual(fixture['id'], recipe['id'])
            self.assertLess(fixture['bitstream_bytes'], runner.FILE_CAP)
            self.assertEqual(fixture['decoded_picture_hash_sei_md5'],
                             [p['coded_component_md5'] for p in fixture['raw']['pictures']])
            self.assertEqual(fixture['syntax']['sample_adaptive_offset_enabled_flag'], 0)
            self.assertEqual(fixture['syntax']['pps_deblocking_filter_disabled_flag'], 1)
            self.assertEqual(fixture['syntax']['pcm_enabled_flag'], 0)

    def test_changed_registered_digest_or_recipe_fails_verification(self):
        expected = json.loads(runner.REGISTRATION.read_text())
        for key in ('bitstream_sha256', 'encoder_version'):
            altered = copy.deepcopy(expected)
            altered['fixtures'][0][key] = 'changed'
            with tempfile.TemporaryDirectory() as directory:
                registration = pathlib.Path(directory) / 'registration.json'
                registration.write_text(json.dumps(altered))
                args = ['hevc_fixture.py', 'verify', '--ffmpeg', '/ffmpeg', '--ffprobe', '/ffprobe',
                        '--output', directory, '--registration', str(registration)]
                with mock.patch.object(sys, 'argv', args), \
                     mock.patch.object(runner.test_oracle, 'require', return_value=runner.test_oracle.Oracle(
                         pathlib.Path('/ffmpeg'), pathlib.Path('/ffprobe'))), \
                     mock.patch.object(runner, 'binary_identity', return_value=expected['oracle']), \
                     mock.patch.object(runner, 'prepare', return_value=expected), \
                     self.assertRaisesRegex(ValueError, 'expected hashes differ'):
                    runner.main()

    def test_registered_binary_mismatch_stops_before_generation(self):
        with tempfile.TemporaryDirectory() as directory:
            args = ['hevc_fixture.py', 'verify', '--ffmpeg', '/ffmpeg', '--ffprobe', '/ffprobe',
                    '--output', directory]
            with mock.patch.object(sys, 'argv', args), \
                 mock.patch.object(runner.test_oracle, 'require', return_value=runner.test_oracle.Oracle(
                     pathlib.Path('/ffmpeg'), pathlib.Path('/ffprobe'))), \
                 mock.patch.object(runner, 'binary_identity', return_value={}), \
                 mock.patch.object(runner, 'prepare') as prepare, \
                 self.assertRaisesRegex(ValueError, 'oracle identity differs'):
                runner.main()
            prepare.assert_not_called()

    def test_unsupported_pcm_is_explicit_not_a_passing_generated_arm(self):
        record = json.loads(runner.REGISTRATION.read_text())
        self.assertEqual(record['pcm']['status'], 'no_PCM_fixture_from_tested_parameters')
        self.assertEqual([a['diagnostic'] for a in record['pcm']['attempts']],
                         ['Unknown option: pcm.', 'Unknown option: ipcm.'])
        self.assertFalse(any('pcm' in f['id'] for f in record['fixtures']))


class ProcessPlatform(unittest.TestCase):
    def test_non_linux_refuses_before_a_child_is_started(self):
        with mock.patch.object(sys, 'platform', 'darwin'), \
             mock.patch.object(runner.subprocess, 'Popen') as launch:
            with self.assertRaisesRegex(ValueError, 'fixture subprocesses require Linux limits'):
                runner.capture(['unused'])
        launch.assert_not_called()


@unittest.skipUnless(sys.platform == 'linux', 'requires Linux subprocess resource limits')
class ProcessBounds(unittest.TestCase):
    def test_real_child_receives_only_the_scrubbed_environment(self):
        with mock.patch.dict(os.environ, {'CAPY_API_KEY': 'must-not-reach-media-child'}):
            out, _ = runner.capture([sys.executable, '-c', 'import os,json; print(json.dumps(dict(os.environ)))'])
        self.assertEqual(json.loads(out), runner.ENVIRONMENT)

    def test_unknown_binary_option_is_a_failure_even_on_exit_zero(self):
        command = [sys.executable, '-c', 'import sys; print("Unknown option: pcm.", file=sys.stderr)']
        with self.assertRaisesRegex(ValueError, 'unsupported fixture parameter'):
            runner.capture(command)

    def test_nonzero_deadline_and_capture_overflow_fail(self):
        with self.assertRaisesRegex(ValueError, 'subprocess failed'):
            runner.capture([sys.executable, '-c', 'raise SystemExit(2)'])
        with mock.patch.object(runner, 'TIMEOUT', 0.1), \
             self.assertRaisesRegex(ValueError, 'deadline'):
            runner.capture([sys.executable, '-c', 'import time; time.sleep(5)'])
        with mock.patch.object(runner, 'CAPTURE_CAP', 100), \
             self.assertRaisesRegex(ValueError, 'capture limit'):
            runner.capture([sys.executable, '-c', 'print("x" * 200)'])

    def test_real_child_has_memory_cpu_file_and_core_limits(self):
        out, _ = runner.capture([sys.executable, '-c',
            'import resource,json; print(json.dumps([resource.getrlimit(k) for k in '
            '(resource.RLIMIT_AS,resource.RLIMIT_CPU,resource.RLIMIT_FSIZE,resource.RLIMIT_CORE)]))'])
        self.assertEqual(json.loads(out), [[2 << 30] * 2, [20] * 2, [runner.FILE_CAP] * 2, [0, 0]])


if __name__ == '__main__':
    unittest.main()
