import pathlib
import ast
import contextlib
from fractions import Fraction
import io
import struct
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import native_h264_cli as cli
import native_h264_matroska as mkv
import oracle_audio


def element(kind, body):
    identifier = kind.to_bytes((kind.bit_length() + 7) // 8, 'big')
    width = next(width for width in range(1, 9) if len(body) < (1 << (7 * width)) - 1)
    return identifier + ((1 << (7 * width)) | len(body)).to_bytes(width, 'big') + body


def integer(kind, value):
    return element(kind, value.to_bytes(max(1, (value.bit_length() + 7) // 8), 'big'))


def generated_sps(rate=Fraction(30), sar=Fraction(1)):
    aspect = '0' if sar == 1 else '1' + f'{255:08b}{sar.numerator:016b}{sar.denominator:016b}'
    bits = (f'{0x42c033:024b}' + '11011010' + '0' + '11' + '1101' + aspect
            + '0110101' + f'{0x010101:024b}' + '1111'
            + f'{rate.denominator:032b}{2 * rate.numerator:032b}' + '100001')
    bits += '0' * (-len(bits) % 8)
    escaped = bytearray()
    for value in int(bits, 2).to_bytes(len(bits) // 8, 'big'):
        if escaped[-2:] == b'\0\0' and value <= 3:
            escaped.append(3)
        escaped.append(value)
    return b'\x67' + escaped


def generated_matroska(*, rate=Fraction(30), sar=Fraction(1), scale=1_000_000,
                       duration=None, pts=None, block_duration=False, wrong_key=False,
                       private_suffix=b'', changed_picture=False, display=None, duplicate_duration=False,
                       sps_rate=None, sps_sar=None, lacing=False, group=False):
    sps = generated_sps(sps_rate or rate, sps_sar or sar)
    pps = b'\x68\x80'
    private = b'\x01\x42\xc0\x33\xff\xe1' + struct.pack('>H', len(sps)) + sps + b'\x01\x00\x02' + pps
    pictures = [b'\x65\x80', b'\x41\x80', b'\x41\x80', b'\x41\x80']
    packets = [struct.pack('>I', len(picture)) + picture for picture in pictures]
    period = duration if duration is not None else mkv.round_half_up(Fraction(1_000_000_000) / rate)
    dimensions = integer(0xb0, 18) + integer(0xba, 34)
    dw, dh = display or ((Fraction(18, 34) * sar).numerator, (Fraction(18, 34) * sar).denominator)
    dimensions += integer(0x54b2, 3) + integer(0x54b0, dw) + integer(0x54ba, dh)
    track = (integer(0xd7, 2) + integer(0x83, 1) + element(0x86, b'V_MPEG4/ISO/AVC')
             + element(0x63a2, private + private_suffix) + integer(0x23e383, period) + element(0xe0, dimensions))
    if duplicate_duration:
        track += integer(0x23e383, period)
    tracks = element(0xae, integer(0xd7, 1) + integer(0x83, 2)) + element(0xae, track)
    blocks = integer(0xe7, 0)
    pts = pts if pts is not None else [mkv.round_half_up(Fraction(i, 1) / rate * 1000) for i in range(4)]
    for index, timestamp in enumerate(pts):
        key = index == 0
        flags = (0x80 if key != (wrong_key and index == 0) else 0) | (2 if lacing else 0)
        payload = packets[index] + (b'\0' if changed_picture and index == 1 else b'')
        block = b'\x82' + struct.pack('>hB', timestamp, flags) + payload
        if group or block_duration:
            contents = element(0xa1, block)
            if not key:
                contents += element(0xfb, b'\xff')
            if block_duration:
                contents += integer(0x9b, 33)
            blocks += element(0xa0, contents)
        else:
            blocks += element(0xa3, block)
    body = (element(0x1549a966, integer(0x2ad7b1, scale)) + element(0x1654ae6b, tracks)
            + element(0x1f43b675, blocks))
    data = element(0x1a45dfa3, b'') + element(0x18538067, body)
    return data, (private, packets, [1, 0, 0, 0])


def generated_opus_mp4(*, media_start=0, edit_duration=63936, pre_skip=312):
    def box(kind, body):
        return struct.pack('>I4s', len(body) + 8, kind) + body

    entry = bytearray(28)
    struct.pack_into('>H', entry, 16, 1)
    struct.pack_into('>I', entry, 24, 48_000 << 16)
    entry += box(b'dOps', struct.pack('>BBHIhB', 0, 1, pre_skip, 48_000, 0, 0))
    table = box(b'stsd', struct.pack('>II', 0, 1) + box(b'Opus', entry))
    table += box(b'stts', struct.pack('>IIII', 0, 1, 9, 960))
    table += box(b'stsz', struct.pack('>III', 0, 2, 9))
    table += box(b'stsc', struct.pack('>IIIII', 0, 1, 1, 9, 1))
    table += box(b'stco', struct.pack('>III', 0, 1, 8))
    media = box(b'mdhd', struct.pack('>IIIIIHH', 0, 0, 0, 48_000, 8640, 0, 0))
    media += box(b'hdlr', bytes(8) + b'soun') + box(b'minf', box(b'stbl', table))
    edit = box(b'elst', struct.pack('>IIIihh', 0, 1, edit_duration, media_start, 1, 0))
    track = box(b'tkhd', struct.pack('>IIIIII', 0, 0, 0, 1, 0, edit_duration))
    track += box(b'edts', edit) + box(b'mdia', media)
    movie = box(b'mvhd', struct.pack('>IIIII', 0, 0, 0, 384_000, edit_duration)) + box(b'trak', track)
    return box(b'mdat', b'\xf8\0' * 9) + box(b'moov', movie)


def generated_opus_matroska(*, delay=6_500_000, padding=13_500_000):
    private = struct.pack('<8sBBHIhB', b'OpusHead', 1, 1, 312, 48_000, 0, 0)
    track = (integer(0xd7, 1) + integer(0x83, 2) + element(0x86, b'A_OPUS')
             + element(0x63a2, private) + integer(0x56aa, delay))
    blocks = integer(0xe7, 0)
    for index in range(9):
        block = element(0xa1, b'\x81' + struct.pack('>hB', index * 20 + int(index > 0), 0) + b'\xf8\0')
        if index == 8:
            block += element(0x75a2, padding.to_bytes(8, 'big', signed=True))
        blocks += element(0xa0, block)
    segment = (element(0x1549a966, integer(0x2ad7b1, 1_000_000))
               + element(0x1654ae6b, element(0xae, track)) + element(0x1f43b675, blocks))
    return element(0x1a45dfa3, b'') + element(0x18538067, segment)


class NativeCli(unittest.TestCase):
    def test_actual_driver_audio_commands_follow_the_shared_oracle_input_policy(self):
        driver = ROOT / 'scripts/native_h264_cli.py'
        self.assertEqual(list(oracle_audio.python_problems(driver.read_text(), 'scripts/native_h264_cli.py')), [])

    def test_edited_opus_preservation_command_copies_audio_and_matches_all_build_modes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)

            def executable(name, mode):
                path = root / name
                path.write_text(f'#!{sys.executable}\nimport json, pathlib, sys\n'
                                f'pathlib.Path(sys.argv[-1]).write_text(json.dumps([{mode!r}, sys.argv[1:-1]]))\n')
                path.chmod(0o755)
                return path

            args = SimpleNamespace(candidate=executable('candidate', 'native-decoder'),
                                   baseline_external=executable('baseline-external', 'native-decoder'),
                                   default_candidate=executable('default-candidate', 'default-decoder'),
                                   baseline_default=executable('baseline-default', 'default-decoder'))
            command = ['-c:v', 'libx264', '-c:a', 'copy', '-fps_mode', 'passthrough']
            source = root / 'audio-mp4-opus-source.mp4'
            skip = {('mkv', 'unselected')}
            groups = cli.unchanged_outputs(args, root, source, None, None,
                                          commands=[('opus-source-edit', command)], skip=skip)
            compared = [('mp4', 'unselected'), ('mp4', 'default'), ('mkv', 'default')]
            self.assertEqual(cli.byte_groups(groups), {
                f'{container}-{mode}': {'count': 1, 'cases': ['opus-source-edit']} for container, mode in compared})
            for container, mode in compared:
                marker = 'native-decoder' if mode == 'unselected' else 'default-decoder'
                for suffix in ('', '-base'):
                    output = root / f'opus-source-edit-{mode}{suffix}.{container}'
                    self.assertEqual(cli.json.loads(output.read_text()),
                                     [marker, ['ffmpeg', '-i', str(source), *command]])
            self.assertFalse((root / 'opus-source-edit-unselected.mkv').exists())
            args.baseline_external = args.baseline_default
            with self.assertRaisesRegex(ValueError, 'actual-base/mp4/opus-source-edit/unselected/whole bytes differs'):
                cli.unchanged_outputs(args, root, source, None, None,
                                      commands=[('opus-source-edit', command)], skip=skip)

    def test_ordinary_matroska_opus_copy_must_present_the_source_edit(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source = root / 'audio-mp4-opus-source.mp4'
            command = ['-c:v', 'libx264', '-c:a', 'copy', '-fps_mode', 'passthrough']
            args = SimpleNamespace(candidate='candidate')
            facts = {'declared_samples': 2, 'channels': 1, 'start_samples': 0, 'pre_skip': 312}
            expected = bytes(8)
            runs = []

            def decode(_, output, __):
                return outputs[output.name]

            outputs = {'opus-source-edit-unselected.mkv': expected}
            patches = (mock.patch.object(cli, 'opus_source_pcm', return_value=(expected, facts)),
                       mock.patch.object(cli, 'run', side_effect=lambda command, env: runs.append((command, env))),
                       mock.patch.object(cli, 'oracle_audio_pcm', side_effect=decode))
            with patches[0], patches[1], patches[2]:
                result = cli.edited_opus_matroska(args, 'ffmpeg', 'ffprobe', root, source, command, 'env')
                self.assertEqual(result, dict(mode='unselected', declared_samples=2, start_samples=0, pre_skip=312))
                self.assertEqual(runs, [(['candidate', 'ffmpeg', '-i', source, *command,
                                          root / 'opus-source-edit-unselected.mkv'], 'env')])
                outputs['opus-source-edit-unselected.mkv'] = expected[4:]
                with self.assertRaisesRegex(ValueError, 'ordinary-matroska/opus-source-edit/unselected: '
                                                        'copied Opus: original declared sample count differs'):
                    cli.edited_opus_matroska(args, 'ffmpeg', 'ffprobe', root, source, command, 'env')

    def test_opus_source_oracle_uses_original_edit_and_only_zeroes_clone_informative_preskip(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source = root / 'original.mp4'
            original = generated_opus_mp4()
            source.write_bytes(original)
            raw = b''.join(struct.pack('<f', sample) for sample in range(8640))
            commands = []

            def decode(command):
                commands.append(command)
                self.assertEqual(command[command.index('-ignore_editlist') + 1], '1')
                clone = pathlib.Path(command[command.index('-i') + 1])
                self.assertNotEqual(clone, source)
                expected_clone = original.replace(struct.pack('>BBHIhB', 0, 1, 312, 48_000, 0, 0),
                                                  struct.pack('>BBHIhB', 0, 1, 0, 48_000, 0, 0), 1)
                self.assertEqual(clone.read_bytes(), expected_clone)
                pathlib.Path(command[-1]).write_bytes(raw)

            with mock.patch.object(cli, 'run', side_effect=decode):
                expected, facts = cli.opus_source_pcm(source, root / 'reference', 'pinned-ffmpeg', 'pinned-ffprobe')
            self.assertEqual(len(commands), 1)
            self.assertEqual(source.read_bytes(), original)
            self.assertEqual(facts['original_edit'], [63936, 0, 1])
            self.assertEqual((facts['declared_samples'], facts['pre_skip']), (7992, 312))
            self.assertEqual(facts['oracle_decoded_samples'], 8640)
            self.assertEqual(expected, raw[:7992 * 4])
            self.assertEqual(len(facts['packet_sha256']), 9)

    def test_opus_source_pcm_rejects_double_preskip_shortfall_wrong_edit_and_changed_samples(self):
        with tempfile.TemporaryDirectory() as directory:
            source = pathlib.Path(directory) / 'original.mp4'
            source.write_bytes(generated_opus_mp4())
            facts = mkv.opus_mp4_source(source)
            raw = b''.join(struct.pack('<f', sample) for sample in range(8640))
            expected = mkv.opus_presentation_slice(raw, facts)
            mkv.compare_opus_pcm(expected, expected, facts)
            for actual, reason in [(expected[312 * 4:], 'declared sample count'),
                                   (raw[312 * 4:(312 + 7992) * 4], 'original-source PCM'),
                                   (expected[:-4] + b'fail', 'original-source PCM')]:
                with self.subTest(reason=reason), self.assertRaisesRegex(ValueError, reason):
                    mkv.compare_opus_pcm(actual, expected, facts)
            with self.assertRaisesRegex(ValueError, 'reference does not match original'):
                mkv.compare_opus_pcm(expected, expected[312 * 4:], facts)
            source.write_bytes(generated_opus_mp4(media_start=312))
            wrong_edit = mkv.opus_mp4_source(source)
            with self.assertRaisesRegex(ValueError, 'original-source PCM'):
                mkv.compare_opus_pcm(mkv.opus_presentation_slice(raw, wrong_edit), expected, facts)
            for change in ({'media_start': -1}, {'edit_duration': 63935}):
                source.write_bytes(generated_opus_mp4(**change))
                with self.subTest(change=change), self.assertRaisesRegex(ValueError, 'source Opus'):
                    mkv.opus_mp4_source(source)

    def test_original_matroska_opus_pcm_uses_declared_delay_padding_without_header_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source = root / 'original.mkv'
            original = generated_opus_matroska()
            source.write_bytes(original)
            raw = bytes(7680 * 4)

            def decode(command):
                if command[0] == 'pinned-ffprobe':
                    return SimpleNamespace(stdout=cli.json.dumps({'streams': [{'time_base': '1/1000'}],
                                                                 'packets': [{'pts': value} for value in (-7, 14, 34, 54, 74, 94, 114, 134, 154)]}).encode())
                self.assertNotIn('-ignore_editlist', command)
                self.assertEqual(command[command.index('-i') + 1], source)
                pathlib.Path(command[-1]).write_bytes(raw)

            with mock.patch.object(cli, 'run', side_effect=decode):
                expected, facts = cli.opus_source_pcm(source, root / 'reference', 'pinned-ffmpeg', 'pinned-ffprobe')
            self.assertEqual(source.read_bytes(), original)
            self.assertEqual(expected, raw)
            self.assertEqual((facts['declared_samples'], facts['codec_delay_samples'], facts['discard_padding_samples']),
                             (7680, 312, 648))
            self.assertEqual(facts['clocks'][0], Fraction(-312, 48_000))
            self.assertEqual(facts['reference_method'], 'original-matroska-declared-presentation')
            self.assertEqual(facts['reported_pts'][:3], ['-7/1000', '7/500', '17/500'])
            for change in ({'delay': 0}, {'padding': 0}):
                wrong = mkv.opus_matroska_source(generated_opus_matroska(**change))
                with self.subTest(change=change), self.assertRaisesRegex(ValueError, 'reference does not match original'):
                    mkv.compare_opus_pcm(expected, expected, wrong)

    def test_original_matroska_opus_clock_preserves_true_delay_and_source_probe_readback(self):
        source = mkv.opus_matroska_source(generated_opus_matroska())
        self.assertEqual(source['clocks'][:3], [Fraction(value, 2000) for value in (-13, 29, 69)])
        source['reported_pts'] = [str(Fraction(value, 1000)) for value in (-7, 14, 34, 54, 74, 94, 114, 134, 154)]
        output = {**source, 'clocks': list(source['clocks'])}
        reported = [Fraction(value) for value in source['reported_pts']]
        mkv.compare_opus_matroska_timeline(output, reported, source)
        guessed = [Fraction(mkv.round_timestamp(value * 1000), 1000) for value in source['clocks']]
        self.assertEqual(guessed[:3], [Fraction(value, 1000) for value in (-7, 15, 35)])
        with self.assertRaisesRegex(ValueError, 'oracle PTS readback'):
            mkv.compare_opus_matroska_timeline(output, guessed, source)
        for field, changed in [('clocks', [Fraction(-7, 1000), *source['clocks'][1:]]),
                               ('codec_delay_samples', 0), ('declared_samples', 7368),
                               ('discard_padding_samples', 0), ('pre_skip', 0)]:
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, field):
                mkv.compare_opus_matroska_timeline({**output, field: changed}, reported, source)

    def test_source_opus_reference_is_prepared_before_native_peer_outputs(self):
        tree = ast.parse((ROOT / 'scripts/native_h264_cli.py').read_text())
        matrix, = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'matroska_matrix']
        source_at, = [index for index, node in enumerate(matrix.body) if isinstance(node, ast.Assign)
                      and isinstance(node.targets[0], ast.Name) and node.targets[0].id == 'opus_sources']
        native_loop_at, = [index for index, node in enumerate(matrix.body) if isinstance(node, ast.For)
                           and ast.unparse(node.target) == '(label, path, rate, extra)']
        self.assertLess(source_at, native_loop_at)

    def test_oracle_field_clock_exception_is_limited_to_29_over_7_with_exact_average(self):
        self.assertEqual(cli.oracle_rates({'r_frame_rate': '58/7', 'avg_frame_rate': '29/7'}, Fraction(29, 7), False),
                         {'oracle_r_frame_rate': '58/7', 'oracle_avg_frame_rate': '29/7',
                          'oracle_rate_interpretation': 'known-29/7-sps-field-clock'})
        for rate, r_frame_rate, average, common in [
            (Fraction(29, 7), '58/7', '30/7', False),
            (Fraction(29, 7), '116/7', '29/7', False),
            (Fraction(25, 2), '25/1', '25/2', False),
            (Fraction(29, 7), '58/7', '29/7', True),
            (Fraction(29, 7), '58/7', str(Fraction(29, 7) + Fraction(1, 10**12)), False),
        ]:
            with self.subTest(rate=rate, r_frame_rate=r_frame_rate, average=average, common=common):
                with self.assertRaisesRegex(ValueError, 'oracle'):
                    cli.oracle_rates({'r_frame_rate': r_frame_rate, 'avg_frame_rate': average}, rate, common)

    def test_oracle_nominal_rates_retain_common_exactness_and_unusual_period_bounds(self):
        for text, common in [('25/1', True), ('30/1', True), ('24000/1001', True),
                             ('30000/1001', True), ('25/2', False), ('1000/33', False), ('29/7', False)]:
            result = cli.oracle_rates({'r_frame_rate': text, 'avg_frame_rate': text}, Fraction(text), common)
            self.assertEqual(result['oracle_rate_interpretation'], 'nominal-frame-rate')
        for field in ('r_frame_rate', 'avg_frame_rate'):
            for value in ('0/1', '31/1'):
                with self.subTest(field=field, value=value), self.assertRaisesRegex(ValueError, 'oracle'):
                    cli.oracle_rates({**dict(r_frame_rate='1000/33', avg_frame_rate='1000/33'), field: value},
                                     Fraction(1000, 33), False)

    def test_copied_audio_axes_include_every_admitted_container_codec_pair(self):
        root = pathlib.Path('/generated')
        with mock.patch.object(cli, 'run') as run:
            sources = cli.copied_audio_sources(root, 'pinned-ffmpeg', 'video30.mp4', 'video25.mp4')
        pairs = set()
        for (label, path, rate), call in zip(sources, run.call_args_list):
            command, = call.args
            self.assertEqual(command[0], 'pinned-ffmpeg')
            encoder = command[command.index('-c:a') + 1]
            pairs.add((path.suffix, encoder))
            self.assertEqual(command[command.index('-c:v') + 1], 'copy')
            self.assertEqual(command[command.index('-i') + 1], 'video25.mp4' if path.suffix == '.mkv' else 'video30.mp4')
            self.assertEqual(rate, 25 if path.suffix == '.mkv' else 30)
        self.assertEqual(pairs, {('.mp4', 'aac'), ('.mp4', 'libmp3lame'), ('.mp4', 'libopus'),
                                 ('.mkv', 'aac'), ('.mkv', 'libopus')})
        self.assertEqual(len(sources), 7)
        tree = ast.parse((ROOT / 'scripts/native_h264_cli.py').read_text())
        matrix, = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'matroska_matrix']
        append_loop, = [node for node in matrix.body if isinstance(node, ast.For)
                        and ast.unparse(node.iter) == 'audio_sources']
        self.assertEqual(ast.unparse(append_loop.target), '(label, path, rate)')
        self.assertIn("axes.append((label, path, rate, ['-c:a', 'copy', '-fps_mode', 'passthrough']))",
                      ast.unparse(append_loop))

    def test_default_and_unselected_whole_bytes_use_matching_baseline_build_modes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)

            def executable(name, marker):
                path = root / name
                path.write_text(f'#!{sys.executable}\nimport json, pathlib, sys\n'
                                f'pathlib.Path(sys.argv[-1]).write_text({marker!r} + json.dumps(sys.argv[1:-1]))\n')
                path.chmod(0o755)
                return path

            args = SimpleNamespace(candidate=executable('candidate', 'native-decoder:'),
                                   baseline_external=executable('baseline-external', 'native-decoder:'),
                                   default_candidate=executable('default-candidate', 'default-decoder:'),
                                   baseline_default=executable('baseline-default', 'default-decoder:'))
            groups = cli.unchanged_outputs(args, root, root / 'source.mp4', None, None)
            self.assertEqual(cli.byte_groups(groups), {
                f'{container}-{mode}': {'count': 2, 'cases': ['default-copy', 'external']}
                for container in ('mp4', 'mkv') for mode in ('unselected', 'default')})
            for container in ('mp4', 'mkv'):
                for label in ('default-copy', 'external'):
                    for mode, marker in (('unselected', 'native-decoder:'), ('default', 'default-decoder:')):
                        previous = root / f'{label}-{mode}-base.{container}'
                        current = root / f'{label}-{mode}.{container}'
                        self.assertTrue(previous.read_text().startswith(marker))
                        self.assertEqual(current.read_bytes(), previous.read_bytes())
            args.default_candidate = executable('default-candidate', 'changed-default:')
            with self.assertRaisesRegex(ValueError, 'actual-base/mp4/default-copy/default/whole bytes differs'):
                cli.unchanged_outputs(args, root, root / 'source.mp4', None, None)

    def test_unchanged_matroska_uses_deterministic_full_fallback_without_changing_native_encoder(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            oracle = root / 'oracle.py'
            oracle.write_text(f'#!{sys.executable}\nimport json, pathlib, sys\n'
                              'pathlib.Path(sys.argv[-1]).write_text(json.dumps(sys.argv[1:-1]))\n')
            oracle.chmod(0o755)
            adapter = cli.deterministic_fallback(oracle, root / 'fallback-bin')

            def executable(name, full_fallback):
                path = root / name
                path.write_text(f'#!{sys.executable}\nimport os, pathlib, subprocess, sys\n'
                                'if "-c:v" not in sys.argv:\n'
                                '    pathlib.Path(sys.argv[-1]).write_bytes(b"native copy bytes")\n'
                                'else:\n'
                                f'    program = os.environ[{"COREAV_FALLBACK" if full_fallback else "COREAV_FFMPEG"!r}]\n'
                                '    subprocess.run([program, *sys.argv[2:]], check=True)\n')
                path.chmod(0o755)
                return path

            args = SimpleNamespace(candidate=executable('candidate', False),
                                   baseline_external=executable('baseline-external', False),
                                   default_candidate=executable('default-candidate', True),
                                   baseline_default=executable('baseline-default', True))
            environment = {**cli.os.environ, 'COREAV_FFMPEG': str(oracle), 'COREAV_FALLBACK': str(oracle)}
            fallback_environment = {**environment, 'COREAV_FALLBACK': str(adapter)}
            groups = cli.unchanged_outputs(args, root, root / 'source.mp4', environment, fallback_environment)
            self.assertEqual(sum(map(len, groups.values())), 8)
            for container in ('mp4', 'mkv'):
                for mode in ('default', 'unselected'):
                    for suffix in ('', '-base'):
                        actual = cli.json.loads((root / f'external-{mode}{suffix}.{container}').read_text())
                        expected = ['-i', str(root / 'source.mp4'), '-an', '-c:v', 'libx264', '-crf', '23']
                        if container == 'mkv' and mode == 'default':
                            expected.extend(['-fflags', '+bitexact'])
                        self.assertEqual(actual, expected)

    def test_baseline_revision_receipt_requires_a_full_sha_and_reaches_verification(self):
        arguments = ['native_h264_cli.py']
        for name in ('candidate', 'default-candidate', 'baseline-default', 'baseline-external',
                     'baseline-native', 'driver', 'ffmpeg', 'output'):
            arguments.extend(['--' + name, name])
        revision = '796acf71b7629e1406b2b79dbba6b0f9f83ab5fd'
        with mock.patch.object(sys, 'argv', [*arguments, '--baseline-revision', revision]), mock.patch.object(cli, 'verify') as verify:
            cli.main()
            self.assertEqual(verify.call_args.args[0].baseline_revision, revision)
            self.assertTrue(verify.call_args.args[0].baseline_native.is_absolute())
        for invalid in ('796acf71', 'not a revision', ''):
            with mock.patch.object(sys, 'argv', [*arguments, '--baseline-revision', invalid]), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as error:
                    cli.main()
                self.assertEqual(error.exception.code, 2)

    def test_existing_axes_are_wired_to_actual_base_not_candidate_self_comparisons(self):
        tree = ast.parse((ROOT / 'scripts/native_h264_cli.py').read_text())
        verify, = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'verify']
        calls = [node for node in ast.walk(verify) if isinstance(node, ast.Call)
                 and isinstance(node.func, ast.Name) and node.func.id == 'native']
        self.assertEqual({node.args[1].value for node in calls},
                         {'quality-and-threads', 'metadata-and-threads', 'legacy', 'gop'})
        bound, = [node for node in verify.body if isinstance(node, ast.FunctionDef) and node.name == 'native']
        pair, = [node for node in ast.walk(bound) if isinstance(node, ast.Call)
                 and isinstance(node.func, ast.Name) and node.func.id == 'native_pair']
        self.assertEqual(ast.unparse(pair.args[0]), 'args.candidate')
        self.assertEqual(ast.unparse(pair.args[1]), 'args.baseline_native')
        matrix, = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'matroska_matrix']
        matrix_calls = [node for node in ast.walk(matrix) if isinstance(node, ast.Call)
                        and isinstance(node.func, ast.Name) and node.func.id == 'native']
        self.assertEqual({node.args[1].value for node in matrix_calls}, {'source-audio-selection', 'matroska-peer'})

    def test_unchanged_output_preflight_runs_before_the_native_media_matrix(self):
        tree = ast.parse((ROOT / 'scripts/native_h264_cli.py').read_text())
        verify, = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'verify']
        assignments = {node.targets[0].id: index for index, node in enumerate(verify.body)
                       if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name)}
        self.assertLess(assignments['source_info'], assignments['fallback_environment'])
        self.assertEqual(assignments['unchanged_groups'], assignments['fallback_environment'] + 1)
        self.assertLess(assignments['unchanged_groups'], assignments['raw'])
        self.assertLess(assignments['unchanged_groups'], next(index for index, node in enumerate(verify.body)
                                                            if isinstance(node, ast.For)))
        calls = [node for node in ast.walk(verify) if isinstance(node, ast.Call)
                 and isinstance(node.func, ast.Name) and node.func.id == 'deterministic_fallback']
        self.assertEqual(len(calls), 1)

    def test_optional_mapping_is_an_explicit_fallback_not_a_native_axis(self):
        tree = ast.parse((ROOT / 'scripts/native_h264_cli.py').read_text())
        matrix, = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'matroska_matrix']
        self.assertNotIn('0:v:0?', ast.unparse(matrix))
        verify, = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'verify']
        fallback, = [node for node in ast.walk(verify) if isinstance(node, ast.Call)
                     and isinstance(node.func, ast.Name) and node.func.id == 'compare'
                     and len(node.args) > 2 and isinstance(node.args[2], ast.Constant)
                     and node.args[2].value == 'optional-map']
        self.assertEqual(ast.literal_eval(fallback.args[1]), ('-map', '0:v:0?'))

    def test_matroska_fallback_adapter_preserves_arguments_and_only_pins_oracle_output(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            executable = root / 'oracle.py'
            receipt = root / 'arguments.json'
            executable.write_text(f'#!{sys.executable}\nimport json, pathlib, sys\n'
                                  'pathlib.Path(sys.argv[-1]).write_text(json.dumps(sys.argv[1:]))\n')
            executable.chmod(0o755)
            adapter = cli.deterministic_fallback(executable, root / 'fallback-bin')
            cli.run([adapter, '-hide_banner', '-i', 'source with spaces.mp4', '-an', receipt])
            self.assertEqual(cli.json.loads(receipt.read_text()),
                             ['-hide_banner', '-i', 'source with spaces.mp4', '-an', '-fflags', '+bitexact', str(receipt)])

    def test_fallback_adapter_structurally_uses_the_recognized_ffmpeg_program_name(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = pathlib.Path(directory) / 'arbitrary-directory-name.py'
            adapter = cli.deterministic_fallback(pathlib.Path('/qualified/ffmpeg'), directory)
            self.assertTrue(directory.is_dir())
            self.assertEqual(adapter, directory / 'ffmpeg')
            self.assertTrue(adapter.is_file())
            self.assertTrue(cli.os.access(adapter, cli.os.X_OK))

    def test_supported_fallback_requires_success_and_replaced_destination(self):
        cli.require_fallback_success(0, b'complete Matroska', 'matroska-high-rate')
        for status, contents in ((4, b'preserved destination'), (0, b'preserved destination'), (1, b'partial output')):
            with self.subTest(status=status, contents=contents):
                with self.assertRaisesRegex(ValueError, 'fallback/matroska-high-rate/expected successful oracle output'):
                    cli.require_fallback_success(status, contents, 'matroska-high-rate')

    def test_actual_base_comparison_executes_both_binaries_without_external_encoders(self):
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / 'out.mp4'
            calls = []

            def execute(argv, *, env):
                calls.append((argv, env))
                pathlib.Path(argv[-1]).write_bytes(b'complete MP4')

            groups = {}
            environment = {'PATH': '/nonexistent', 'COREAV_X264': '/nonexistent',
                           'COREAV_FFMPEG': '/nonexistent', 'COREAV_FALLBACK': '/nonexistent'}
            with mock.patch.object(cli, 'run', side_effect=execute):
                cli.native_pair('candidate', 'actual-base', ['-i', 'source', '-an', '-c:v', 'libx264', output],
                                env=environment, group='quality', label='crf23-t2', groups=groups)
            self.assertEqual([call[0][0] for call in calls], ['candidate', 'actual-base'])
            self.assertEqual(calls[0][0][1:-1], calls[1][0][1:-1])
            self.assertEqual(calls[0][0][1:3], ['--native-h264-encoder', 'ffmpeg'])
            self.assertEqual([call[1] for call in calls], [environment] * 2)
            self.assertEqual(cli.byte_groups(groups), {'quality': {'count': 1, 'cases': ['crf23-t2']}})

    def test_actual_base_comparison_rejects_changed_bytes_and_does_not_count_a_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / 'out.mp4'
            groups = {}

            def execute(argv, *, env):
                pathlib.Path(argv[-1]).write_bytes(str(argv[0]).encode())

            with mock.patch.object(cli, 'run', side_effect=execute):
                with self.assertRaisesRegex(ValueError, 'actual-base/metadata/rate25/whole MP4 bytes differs'):
                    cli.native_pair('candidate', 'base', [output], env={}, group='metadata', label='rate25', groups=groups)
            self.assertEqual(groups, {})

    def test_actual_base_comparison_reports_the_case_when_a_binary_fails(self):
        with mock.patch.object(cli, 'run', side_effect=ValueError('no encoder program')):
            with self.assertRaisesRegex(ValueError, 'actual-base/gop/251: no encoder program'):
                cli.native_pair('candidate', 'base', [pathlib.Path('out.mp4')], env={}, group='gop', label='251', groups={})

    def test_matroska_reader_checks_common_and_unusual_exact_clocks_and_aspects(self):
        for rate in map(Fraction, ('25', '30', '24000/1001', '30000/1001', '25/2', '1000/33', '29/7', '1000')):
            for sar in map(Fraction, ('1', '12/11', '17/13', '65535/65534')):
                with self.subTest(rate=rate, sar=sar):
                    for group in (False, True):
                        data, mp4 = generated_matroska(rate=rate, sar=sar, group=group)
                        result = mkv.validate(data, rate=rate, sar=sar, width=18, height=34,
                                              times=[Fraction(i) / rate for i in range(4)],
                                              sps_reader=cli.sps_metadata, mp4=mp4)
                        self.assertEqual(result['duration'], mkv.round_half_up(Fraction(1_000_000_000) / rate))

    def test_matroska_mutations_reject_wrong_clocks_aspect_private_payload_and_key_flags(self):
        for change, reason in [
            ({'scale': 1}, 'TimestampScale'), ({'duration': 33_000_000}, 'DefaultDuration'),
            ({'pts': [0, 33, 66, 100]}, 'rounded PTS'),
            ({'block_duration': True}, 'conflicting BlockDuration'),
            ({'wrong_key': True}, 'keyframe flag'), ({'display': (1, 1)}, 'display DAR'),
            ({'private_suffix': b'\0'}, 'CodecPrivate: trailing'),
            ({'changed_picture': True}, 'access unit: truncated'),
            ({'duplicate_duration': True}, 'DefaultDuration: duplicate'),
            ({'sps_rate': Fraction(25)}, 'SPS: exact timing'),
            ({'sps_sar': Fraction(17, 13)}, 'SPS: exact timing'),
            ({'lacing': True}, 'unexpected lacing'),
        ]:
            with self.subTest(change=change):
                data, mp4 = generated_matroska(**change)
                with self.assertRaisesRegex(ValueError, reason):
                    mkv.validate(data, rate=Fraction(30), sar=Fraction(1), width=18, height=34,
                                 times=[Fraction(i, 30) for i in range(4)], sps_reader=cli.sps_metadata, mp4=mp4)

    def test_matroska_comparison_consumes_complete_access_units_not_only_coded_pictures(self):
        data, (private, packets, keys) = generated_matroska()
        for mp4, reason in [((private + b'\0', packets, keys), 'CodecPrivate: same-head MP4'),
                            ((private, [packets[0], *packets[1:-1]], keys), 'access units: same-head MP4'),
                            ((private, [packets[0] + b'\0', *packets[1:]], keys), 'access units: same-head MP4'),
                            ((private, packets, [1, 1, 0, 0]), 'MP4 key flags')]:
            with self.subTest(reason=reason):
                with self.assertRaisesRegex(ValueError, reason):
                    mkv.validate(data, rate=Fraction(30), sar=Fraction(1), width=18, height=34,
                                 times=[Fraction(i, 30) for i in range(4)], sps_reader=cli.sps_metadata, mp4=mp4)

    def test_matroska_reader_rejects_rounded_timestamp_collisions(self):
        rate = Fraction(2000)
        data, mp4 = generated_matroska(rate=rate)
        with self.assertRaisesRegex(ValueError, 'rounded PTS collision'):
            mkv.validate(data, rate=rate, sar=Fraction(1), width=18, height=34,
                         times=[Fraction(i) / rate for i in range(4)], sps_reader=cli.sps_metadata, mp4=mp4)

    def test_ebml_reader_rejects_truncated_and_unknown_size_elements(self):
        for data in (b'\0', b'\x81', b'\x81\xff', b'\x81\x82\0', b'\x01'):
            with self.subTest(data=data), self.assertRaisesRegex(ValueError, 'EBML'):
                list(mkv.elements(data))

    def test_nominal_duration_rounds_half_up_without_floating_point(self):
        self.assertEqual(mkv.round_half_up(Fraction(5, 2)), 3)
        self.assertEqual(mkv.round_half_up(Fraction(4_000_000_001, 2)), 2_000_000_001)
        self.assertEqual(mkv.round_half_up(Fraction(1_001_000_000_000, 30000)), 33_366_667)

    def test_timestamp_rounding_handles_negative_and_positive_half_millisecond_ties(self):
        for timestamp, expected in [(Fraction(-3, 2), -2), (Fraction(-1, 2), -1),
                                    (Fraction(-499, 1000), 0), (Fraction(499, 1000), 0),
                                    (Fraction(1, 2), 1), (Fraction(3, 2), 2)]:
            with self.subTest(timestamp=timestamp):
                self.assertEqual(mkv.round_timestamp(timestamp), expected)

    def test_terminal_duration_changes_only_the_last_generated_sample(self):
        def box(kind, body):
            return struct.pack('>I4s', len(body) + 8, kind) + body

        def movie(payload):
            for kind in (b'stbl', b'minf', b'mdia', b'trak', b'moov'):
                payload = box(kind, payload)
            return box(b'mdat', b'unchanged encoded pictures') + payload

        for count in (1, 4):
            original = movie(box(b'stts', struct.pack('>IIII', 0, 1, count, 512)))
            for duration in (0, 1024):
                pairs = (1, count, duration) if count == 1 else (2, count - 1, 512, 1, duration)
                expected = movie(box(b'stts', struct.pack('>' + 'I' * (len(pairs) + 1), 0, *pairs)))
                self.assertEqual(cli.terminal_duration(original, duration), expected)

    def test_explicit_sps_sample_aspect_rejects_zero_terms_with_a_label(self):
        def sps(width, height):
            bits = (f'{0x42c033:024b}' + '110110100111101111111111'
                    + f'{width:016b}{height:016b}' + '0110101'
                    + f'{0x010101:024b}' + '1111' + f'{1:032b}{60:032b}' + '100001')
            bits += '0' * (-len(bits) % 8)
            escaped = bytearray()
            for value in int(bits, 2).to_bytes(len(bits) // 8, 'big'):
                if escaped[-2:] == b'\0\0' and value <= 3:
                    escaped.append(3)
                escaped.append(value)
            return b'\x67' + escaped

        self.assertEqual(cli.sps_metadata(sps(17, 13)), (Fraction(30), Fraction(17, 13)))
        for width, height in ((0, 1), (1, 0), (0, 0)):
            with self.subTest(width=width, height=height):
                with self.assertRaisesRegex(ValueError, 'zero SPS sample aspect'):
                    cli.sps_metadata(sps(width, height))

    def test_sample_aspect_reader_uses_the_visual_entry_structure(self):
        def box(kind, body):
            return struct.pack('>I4s', len(body) + 8, kind) + body

        for ratio in (None, (16, 15), (65535, 65534)):
            body = bytes(78) + (box(b'pasp', struct.pack('>II', *ratio)) if ratio else b'')
            body = box(b'stsd', bytes(4) + struct.pack('>I', 1) + box(b'avc1', body))
            for kind in (b'stbl', b'minf', b'mdia', b'trak', b'moov'):
                body = box(kind, body)
            data = box(b'mdat', box(b'pasp', struct.pack('>II', 7, 9))) + body
            actual = cli.entry_aspect(data)
            self.assertEqual(struct.unpack_from('>II', data, actual[0]) if actual else None, ratio)
        with self.assertRaisesRegex(ValueError, 'missing generated video sample entry'):
            cli.entry_aspect(box(b'mdat', box(b'pasp', struct.pack('>II', 7, 9))))

    def test_oracle_annex_b_accepts_both_standard_delimiter_lengths(self):
        self.assertEqual(cli.annexb_units(b'\0\0\0\1\x67\x80\0\0\1\x68\x80\0\0\0\1\x65\x80'),
                         [b'\x67\x80', b'\x68\x80', b'\x65\x80'])
        for data in (b'', b'no delimiter', b'\0\0\1', b'\0\0\1\0\0\1\x65\x80'):
            with self.assertRaises(ValueError):
                cli.annexb_units(data)

    def test_byte_comparator_rejects_a_changed_output(self):
        cli.same(b'complete', b'complete', 'output')
        with self.assertRaisesRegex(ValueError, 'output differs'):
            cli.same(b'complete', b'changed', 'output')

    def test_workflow_builds_actual_base_and_candidate_modes(self):
        workflow = (ROOT / '.github/workflows/h264-encoder.yml').read_text()
        job = workflow.split('  native-h264-cli:', 1)[1].split('  h264-encoder-conformance:', 1)[0]
        for required in ('github.event.pull_request.base.sha || github.sha',
                         'cd ../baseline', 'default-base', 'external-base', 'default-candidate',
                         '--features native-h264-encoder,native-h264', '--features h264-native',
                         '--features full --example p', 'scripts/native_h264_cli.py',
                         'hostile_encoders_end_in_an_error_and_leave_nothing -- --exact',
                         '--test docs_site --test strict_safe_limit --test ffprobe_limits',
                         "for features in '' release-default release-full"):
            self.assertIn(required, job)
        base_step = job.split('- name: Preserve base-build byte references', 1)[1].split('- name:', 1)[0]
        self.assertIn('cd ../baseline', base_step)
        self.assertIn('cargo build --locked --release -p coreav-cli --features native-h264-encoder,native-h264\n'
                      '          cp target/release/coreav "$RUNNER_TEMP/native-base"', base_step)
        self.assertIn('--baseline-native "$RUNNER_TEMP/native-base"', job)
        self.assertIn('--baseline-revision "$(git -C ../baseline rev-parse HEAD)"', job)
        for event in ('push:', 'pull_request:'):
            selected = workflow.split('  ' + event, 1)[1].split('  workflow_dispatch:', 1)[0]
            for path in ('crates/coreav-cli/**', 'crates/coreav-transcode/**', 'scripts/native_h264_cli.py',
                         'scripts/native_h264_matroska.py', 'scripts/track_boxes.py'):
                self.assertIn(path, selected)

    def test_cli_oracles_install_nasm_before_x264(self):
        workflow = (ROOT / '.github/workflows/h264-encoder.yml').read_text()
        job = workflow.split('  native-h264-cli:', 1)[1].split('  h264-encoder-conformance:', 1)[0]
        self.assertLess(job.index('apt-get install -y --no-install-recommends nasm'),
                        job.index('scripts/install-x264.sh'))
        for stage in ('Installing pinned FFmpeg', 'Installing pinned x264',
                      'reason=pinned_ffmpeg_install_failed', 'reason=pinned_x264_install_failed'):
            self.assertIn(stage, job)


if __name__ == '__main__':
    unittest.main()
