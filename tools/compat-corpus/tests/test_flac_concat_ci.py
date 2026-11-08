import copy
from fractions import Fraction
import hashlib
import json
import os
from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import flac_concat_ci as checker
import flac_concat_fixture


def packet(number, variable=False, block=32, body=bytes(6)):
    if number < 128:
        coded = bytes([number])
    else:
        count = next(n for n in range(2, 8) if number < 1 << (5 * n + 1))
        coded = bytes([(255 << (8 - count) & 255) | (number >> (6 * (count - 1)))])
        coded += bytes(128 | (number >> (6 * n) & 63) for n in reversed(range(count - 1)))
    header = bytes([255, 248 | variable, 0x6a, 0x18]) + coded + bytes([block - 1])
    header += bytes([checker.crc(header, 8, 7)])
    data = header + body
    return data + checker.crc(data, 16, 0x8005).to_bytes(2, 'big')


def config(samples=128, minimum=16, maximum=32, frame_size=15):
    packed = 48000 << 44 | 1 << 41 | 15 << 36 | samples
    return (minimum.to_bytes(2, 'big') + maximum.to_bytes(2, 'big')
            + frame_size.to_bytes(3, 'big') * 2 + packed.to_bytes(8, 'big') + bytes(16))


def dump(data):
    lines = []
    for at in range(0, len(data), 16):
        part = data[at:at + 16]
        words = ' '.join(part[n:n + 2].hex() for n in range(0, len(part), 2))
        lines.append(f'{at:08x}: {words:<39}  .')
    return '\n' + '\n'.join(lines)


def rows(path, packets):
    path.write_bytes(b''.join(packets))
    result, offset = [], 0
    for index, data in enumerate(packets):
        result.append({'pos': str(offset), 'size': str(len(data)), 'data_hash': 'SHA256:' + hashlib.sha256(data).hexdigest(),
                       'pts': index * 32, 'dts': index * 32, 'duration': 32, 'flags': 'K_'})
        offset += len(data)
    return result


class HeaderProof(unittest.TestCase):
    def test_rfc_header_crc_and_canonical_subframes(self):
        self.assertEqual(checker.crc(bytes.fromhex('fff869180000'), 8, 7), 0xbf)
        original = checker.frame(packet(0))['canonical']
        for number in [0, 127, 128, 2048, 65536, 1 << 21, 1 << 26, 1 << 31, (1 << 36) - 1]:
            data = packet(number, True)
            parsed = checker.frame(data)
            self.assertEqual(parsed['number'], number)
            self.assertEqual(parsed['canonical'], original)
            self.assertEqual(checker.crc(data, 16, 0x8005), 0)
        changed = checker.frame(packet(0, True, body=b'\0\0\1\0\0\0'))
        self.assertNotEqual(changed['canonical'], original)

    def test_bad_crcs_noncanonical_numbers_and_extents_fail(self):
        good = packet(0)
        bad_header = bytearray(good)
        bad_header[6] ^= 1
        bad_header[-2:] = checker.crc(bad_header[:-2], 16, 0x8005).to_bytes(2, 'big')
        noncanonical = good[:4] + b'\xc0\x80' + good[5:6]
        noncanonical += bytes([checker.crc(noncanonical, 8, 7)]) + bytes(6)
        noncanonical += checker.crc(noncanonical, 16, 0x8005).to_bytes(2, 'big')
        for bad in [*([good[:end] for end in range(8)]), bytes(bad_header), noncanonical,
                    good[:-1] + bytes([good[-1] ^ 1])]:
            with self.subTest(data=bad), self.assertRaises(ValueError):
                checker.frame(bad)

    def test_output_sequence_and_every_streaminfo_statistic_are_required(self):
        frames = [checker.frame(packet(number, True)) for number in [0, 32, 64, 96]]
        info = checker.stream_info(config())
        checker.validate_stream(frames, info, info['format'])
        for name, value in [('minimum', 32), ('maximum', 31), ('samples', 64),
                            ('min_frame', 14), ('max_frame', 16), ('md5', b'x' * 16), ('format', 0)]:
            with self.subTest(field=name), self.assertRaises(ValueError):
                checker.validate_stream(frames, {**info, name: value}, info['format'])
        for name, value in [('number', 0), ('variable', False), ('block', 1)]:
            bad = copy.deepcopy(frames)
            bad[2][name] = value
            with self.subTest(frame_field=name), self.assertRaises(ValueError):
                checker.validate_stream(bad, info, info['format'])
        fixed = [checker.frame(packet(number)) for number in [0, 1]]
        checker.validate_stream(fixed, checker.stream_info(config(64, 32, 32)), info['format'])


class ScopedProjection(unittest.TestCase):
    def test_exact_six_cases_reject_missing_duplicate_unlisted_or_refused_variants(self):
        cases = [{'id': 'C02', 'job': 'join', 'status': 'written', 'source': checker.FIXTURE, 'args': list(args)}
                 for args in checker.VARIANTS]
        self.assertEqual(checker.exact_cases(cases), cases)
        for bad in [[], cases[:-1], cases + cases[:1], cases[:-1] + cases[:1],
                    [{**row, 'id': 'C03'} for row in cases],
                    [{**row, 'status': 'refused'} for row in cases],
                    [{**row, 'args': ['-hide_banner'] + row['args']} for row in cases]]:
            with self.subTest(cases=bad), self.assertRaisesRegex(ValueError, 'exact_six'):
                checker.exact_cases(bad)

    def test_fixture_identity_is_not_a_filename_or_command_pattern(self):
        with tempfile.TemporaryDirectory() as temporary:
            source = Path(temporary) / checker.FIXTURE
            source.write_bytes(b'wrong fixture')
            record = {'id': 'C02', 'job': 'join', 'status': 'written',
                      'source': str(source), 'args': list(checker.VARIANTS[0])}
            with self.assertRaisesRegex(ValueError, 'fixture_digest'):
                checker.qualified_format(record, {})
            for field, value in [('id', 'C03'), ('status', 'refused'),
                                 ('args', ['-hide_banner'] + record['args'])]:
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'unlisted'):
                    checker.qualified_format({**record, field: value}, {})

    def test_every_binding_is_checked_before_frozen_output_grouping(self):
        cases = [{'id': 'C02', 'job': 'join', 'status': 'written', 'source': checker.FIXTURE, 'args': list(args)}
                 for args in checker.VARIANTS]
        with tempfile.TemporaryDirectory() as temporary:
            records = Path(temporary) / 'records.json'
            records.write_text(json.dumps(cases))
            with mock.patch.object(sys, 'argv', ['checker', '--records', str(records), '--ffprobe', 'qualified']), \
                    mock.patch.object(checker, 'frozen_check'), \
                    mock.patch.object(checker, 'qualified_format', side_effect=[123] * 5 + [ValueError('last binding')]) as qualify, \
                    mock.patch.object(checker.copy_check, 'main') as grouped:
                with self.assertRaisesRegex(ValueError, 'last binding'):
                    checker.main()
                self.assertEqual(qualify.call_args_list,
                                 [mock.call(record, {'ffprobe': 'qualified'}) for record in cases])
                grouped.assert_not_called()

    def test_frozen_agreement_or_other_failure_cannot_be_adapted(self):
        with mock.patch.object(checker, 'qualified_format', return_value=123), \
                mock.patch.object(checker, 'pcm_proof') as pcm, \
                mock.patch.object(checker, 'projected_packets') as projection:
            for problems in [[], ['video differs'], checker.FROZEN_DIFFERENCES + ['timing differs']]:
                with mock.patch.object(checker, 'LEGACY_ORACLE', return_value=problems):
                    self.assertTrue(checker.current_oracle({}, {}))
            pcm.assert_not_called()
            projection.assert_not_called()

    def test_pcm_must_equal_both_complete_inputs_and_decoder_failure_is_fatal(self):
        for change in ('none', 'sample', 'short', 'empty', 'decode', 'stderr', 'timeout'):
            with self.subTest(change=change):
                def invoke(command, env, output):
                    self.assertIn('-xerror', command)
                    self.assertIn('crccheck+explode', command)
                    self.assertLess(command.index('-ignore_editlist'), command.index('-i'))
                    data = b'abcd' if command[command.index('-i') + 1] == 'input' else b'abcdabcd'
                    if output.name == '1':
                        data = {'sample': b'abcdabce', 'short': b'abcd', 'empty': b''}.get(change, data)
                    output.write_bytes(data)
                    return (None if change == 'timeout' else 1 if change == 'decode' else 0,
                            b'', b'bad decode' if change == 'stderr' else b'')
                with mock.patch.object(checker.decoded_output, 'invoke', side_effect=invoke):
                    if change == 'none':
                        checker.pcm_proof({'source': 'input', 'output': 'output'}, {'ffmpeg': 'ffmpeg'})
                    else:
                        with self.assertRaisesRegex(ValueError, 'flac_.*pcm'):
                            checker.pcm_proof({'source': 'input', 'output': 'output'}, {'ffmpeg': 'ffmpeg'})

    def exercise(self, change=None):
        with tempfile.TemporaryDirectory() as temporary:
            source, output = (Path(temporary) / name for name in ('source', 'output'))
            inputs = rows(source, [packet(0), packet(1)])
            data = [packet(n, True) for n in [0, 32, 64, 96]]
            if change == 'subframe':
                data[2] = packet(64, True, body=b'\0\0\1\0\0\0')
            if change == 'restart':
                data[2] = packet(0, True)
            outputs = rows(output, data)
            if change == 'timing':
                outputs[2]['pts'] += 99
            if change == 'receipt':
                outputs[2]['data_hash'] = 'SHA256:wrong'
            original_rows = copy.deepcopy(outputs)

            def reader(ffprobe, path, index, raw=False):
                return copy.deepcopy(outputs if Path(path) == output else inputs)

            info = checker.stream_info(config())
            description = {'streams': [{'codec_name': 'flac', 'extradata': dump(config())}]}
            with mock.patch.object(checker.join_check.cc, 'packets', reader), \
                    mock.patch.object(checker.join_check.cc, 'ffprobe_json', return_value=description):
                self.assertEqual(checker.join_check.cc.packets('probe', output, 0), original_rows)
                with checker.projected_packets(output, info['format']):
                    normalized = checker.join_check.cc.packets('probe', output, 0)
                    original = checker.join_check.cc.packets('probe', source, 0)
                    for old, new in zip(original_rows, normalized):
                        self.assertEqual({k: v for k, v in old.items() if k not in ('size', 'data_hash')},
                                         {k: v for k, v in new.items() if k not in ('size', 'data_hash')})
                    counts, found = {'agree': 0, 'disagree': 0}, []
                    checker.join_check.compare_packets('audio', [original, original],
                        [Fraction(0), Fraction(64, 48000)], normalized, Fraction(1, 48000), counts, found, {})
                self.assertIs(checker.join_check.cc.packets, reader)
                return found

    def test_only_valid_header_differences_are_projected(self):
        self.assertEqual(self.exercise(), [])
        self.assertTrue(self.exercise('subframe'))
        self.assertTrue(self.exercise('timing'))
        for change in ('restart', 'receipt'):
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.exercise(change)

    def test_non_flac_and_other_jobs_keep_the_legacy_judgment(self):
        reader = lambda *args, **kwargs: [{'size': '7', 'data_hash': 'unchanged', 'pts': 123}]
        with mock.patch.object(checker.join_check.cc, 'packets', reader), \
                mock.patch.object(checker.join_check.cc, 'ffprobe_json', return_value={'streams': [{'codec_name': 'aac'}]}):
            with checker.projected_packets('output', 0):
                self.assertEqual(checker.join_check.cc.packets('probe', 'path', 0), reader())
        record, tools = {'job': 'remux'}, object()
        with mock.patch.object(checker, 'LEGACY_ORACLE', return_value=['original failure']) as legacy:
            self.assertEqual(checker.current_oracle(record, tools), ['original failure'])
            legacy.assert_called_once_with(record, tools)

    def test_the_frozen_owners_cannot_change_under_the_adapter(self):
        checker.frozen_check()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            expected = {}
            for name in checker.FROZEN:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(name)
                expected[name] = hashlib.sha256(path.read_bytes()).hexdigest()
            registration = root / 'docs/measurements/compat-0.1.0/judging.json'
            registration.parent.mkdir(parents=True)
            registration.write_text(json.dumps({'files': expected}))
            checker.frozen_check(root)
            (root / checker.FROZEN[0]).write_text('changed')
            with self.assertRaisesRegex(ValueError, 'frozen_mismatch'):
                checker.frozen_check(root)

    def test_only_current_writer_ci_uses_the_adapter(self):
        workflow = (checker.ROOT / '.github/workflows/ci.yml').read_text()
        self.assertEqual(workflow.count('python3 -I tools/compat-corpus/flac_concat_ci.py --records'), 1)
        self.assertIn('python3 -I tools/compat-corpus/copy_check.py --records', workflow)
        frozen = (checker.ROOT / '.github/workflows/compat-0.1.0.yml').read_text()
        self.assertNotIn('flac_concat_ci', frozen)


@unittest.skipUnless(os.environ.get('COREAV_CURRENT_CI_ORACLE') == '1', 'needs generated current-CI records')
class GeneratedRecords(unittest.TestCase):
    @unittest.skipUnless(sys.platform == 'linux', 'the fixture digest uses the pinned Linux oracle')
    def test_fixture_generation_pins_override_cpu_requests_and_match_the_digest(self):
        import test_oracle
        tools = test_oracle.require()
        with tempfile.TemporaryDirectory() as temporary:
            for index, (count, flags) in enumerate(((1, 'sse2'), (8, 'ssse3'))):
                output = Path(temporary) / f'{index}.mp4'
                command = flac_concat_fixture.command(tools.ffmpeg, output)
                command[1:1] = ['-cpucount', str(count), '-cpuflags', flags]
                at = command.index('-c:v')
                command[at:at] = ['-x264-params', f'asm={flags}']
                done = subprocess.run(command, capture_output=True, timeout=120, check=False)
                self.assertEqual(done.returncode, 0, done.stderr)
                self.assertEqual(hashlib.sha256(output.read_bytes()).hexdigest(), checker.FIXTURE_SHA256)

    def test_all_six_flac_concat_variants_keep_the_complete_oracle(self):
        import test_oracle
        tools = test_oracle.require()
        programs = {'ffmpeg': str(tools.ffmpeg), 'ffprobe': str(tools.ffprobe), 'mediainfo': None}
        records = json.loads((Path(os.environ['COREAV_FFMPEG_OUT']) / 'records.json').read_text())
        selected = [record for record in records if record.get('status') == 'written'
                    and checker.qualified_format(record, programs) is not None]
        self.assertEqual(len(selected), 6)
        checker.exact_cases(selected)
        for record in selected:
            with self.subTest(args=record['args']):
                self.assertEqual(checker.LEGACY_ORACLE(record, programs), checker.FROZEN_DIFFERENCES)
                self.assertEqual(checker.current_oracle(record, programs), [])
