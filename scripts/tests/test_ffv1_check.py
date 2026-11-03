"""Mutations cannot pass the FFV1 exact-pixel and probe gate."""

import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import ffv1_check as gate


def case():
    return {'name': 'odd-420', 'file': 'ffv1.mkv', 'raw': 'source.raw', 'width': 3, 'height': 3,
            'pix_fmt': 'yuv420p', 'frames': 3, 'pts_ns': [0, 40_000_000, 100_000_000],
            'duration_ns': [40_000_000, 60_000_000, 20_000_000], 'sample_aspect_ratio': '4:3',
            'color': {'color_range': 'tv', 'color_space': 'bt709',
                      'color_transfer': 'bt709', 'color_primaries': 'bt709'}}


def document():
    row = case()
    return {'streams': [{'codec_name': 'ffv1', 'codec_type': 'video', 'width': 3, 'height': 3,
                         'pix_fmt': 'yuv420p', 'bits_per_raw_sample': '8', 'sample_aspect_ratio': '4:3',
                         'display_aspect_ratio': '4:3', 'field_order': 'progressive',
                         'time_base': '1/1000', **row['color']}],
            'frames': [{'media_type': 'video', 'width': 3, 'height': 3, 'pix_fmt': 'yuv420p',
                        'key_frame': 1, 'interlaced_frame': 0, 'pts': pts // gate.SCALE_NS,
                        'duration': duration // gate.SCALE_NS}
                       for pts, duration in zip(row['pts_ns'], row['duration_ns'])]}


class ExactComparison(unittest.TestCase):
    def setUp(self):
        self.expected = gate.Expected.parse(case())
        self.raw = bytes(range(self.expected.frame_bytes * self.expected.frames))

    def test_exact_odd_subsampled_frames_and_nonuniform_timestamps_pass(self):
        self.assertEqual(self.expected.frame_bytes, 17)
        gate.compare_raw(self.expected, self.raw, self.raw)
        gate.compare_probe(self.expected, document())

    def test_one_pixel_add_drop_and_truncation_fail(self):
        pixel = bytearray(self.raw)
        pixel[19] ^= 1
        for name, data in (('pixel', pixel), ('added-frame', self.raw + self.raw[:17]),
                           ('dropped-frame', self.raw[:-17]), ('truncated', self.raw[:-1]),
                           ('appended-byte', self.raw + b'\0'), ('empty', b'')):
            with self.subTest(name=name), self.assertRaises(gate.GateError):
                gate.compare_raw(self.expected, self.raw, data)

    def test_truncated_original_cannot_agree_with_truncated_decode(self):
        with self.assertRaises(gate.GateError):
            gate.compare_raw(self.expected, self.raw[:-1], self.raw[:-1])

    def test_every_required_stream_field_is_checked(self):
        mutations = {'codec_name': 'h264', 'codec_type': 'audio', 'width': 4, 'height': 4,
                     'pix_fmt': 'yuv444p', 'bits_per_raw_sample': '10', 'bits_per_sample': '10',
                     'sample_aspect_ratio': '1:1', 'display_aspect_ratio': '1:1',
                     'field_order': 'tt', 'time_base': '1/1000000', 'color_range': 'pc',
                     'color_space': 'smpte170m', 'color_transfer': 'smpte170m', 'color_primaries': 'smpte170m'}
        for key, value in mutations.items():
            mutated = document()
            mutated['streams'][0][key] = value
            with self.subTest(key=key), self.assertRaises(gate.GateError):
                gate.compare_probe(self.expected, mutated)
        mutated = document()
        mutated['streams'].append(copy.deepcopy(mutated['streams'][0]))
        with self.assertRaises(gate.GateError):
            gate.compare_probe(self.expected, mutated)

    def test_each_frame_timestamp_duration_geometry_format_and_keyframe_is_checked(self):
        for key, value in (('pts', 41), ('duration', 59), ('width', 4), ('height', 4),
                           ('pix_fmt', 'yuv422p'), ('key_frame', 0), ('interlaced_frame', 1)):
            mutated = document()
            mutated['frames'][1][key] = value
            with self.subTest(key=key), self.assertRaises(gate.GateError):
                gate.compare_probe(self.expected, mutated)
        for count in (2, 4):
            mutated = document()
            mutated['frames'] = (mutated['frames'] * 2)[:count]
            with self.subTest(count=count), self.assertRaises(gate.GateError):
                gate.compare_probe(self.expected, mutated)
        mutated = document()
        mutated['frames'][1]['pts_time'] = '0.040000'
        del mutated['frames'][1]['pts']
        with self.assertRaises(gate.GateError):
            gate.compare_probe(self.expected, mutated)

    def test_unknown_colour_and_duration_have_absence_expectations(self):
        row = case()
        row['color'] = dict.fromkeys(gate.COLOR_FIELDS)
        row['duration_ns'][-1] = None
        expected = gate.Expected.parse(row)
        observed = document()
        for key in gate.COLOR_FIELDS:
            del observed['streams'][0][key]
        del observed['frames'][-1]['duration']
        gate.compare_probe(expected, observed)
        for key in gate.COLOR_FIELDS:
            mutated = copy.deepcopy(observed)
            mutated['streams'][0][key] = case()['color'][key]
            with self.subTest(key=key), self.assertRaises(gate.GateError):
                gate.compare_probe(expected, mutated)
        observed['frames'][-1]['duration'] = 20
        with self.assertRaises(gate.GateError):
            gate.compare_probe(expected, observed)

    def test_all_native_planar_byte_counts(self):
        for pix_fmt, size in (('yuv420p', 17), ('yuv422p', 21), ('yuv444p', 27)):
            row = {**case(), 'pix_fmt': pix_fmt}
            self.assertEqual(gate.Expected.parse(row).frame_bytes, size)

    def test_integer_tick_rounding_has_no_decimal_tolerance(self):
        self.assertEqual(gate.stored_ns(1, '1/2000'), 1_000_000)
        self.assertEqual(gate.stored_ns(3, '1/2000'), 2_000_000)
        self.assertEqual(gate.stored_ns(1, '1001/30000'), 33_000_000)
        row = case()
        row['pts_ns'][1] += 1
        with self.assertRaises(gate.GateError):
            gate.Expected.parse(row)

    def test_decode_preserves_native_format_and_disables_frame_synchronization(self):
        with patch.object(gate, 'run', return_value=subprocess.CompletedProcess([], 0, self.raw, b'')) as run:
            self.assertEqual(gate.decode('ffmpeg', 'ffv1.mkv', self.expected), self.raw)
        argv = run.call_args.args[0]
        self.assertEqual(argv[argv.index('-pix_fmt') + 1], 'yuv420p')
        self.assertEqual(argv[argv.index('-fps_mode') + 1], 'passthrough')
        self.assertIn('-xerror', argv)
        self.assertEqual(argv[argv.index('-err_detect') + 1], 'explode')
        self.assertNotIn('-vf', argv)
        self.assertEqual(run.call_args.kwargs['output_limit'], len(self.raw))


class BoundaryFailures(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def executable(self, name, body):
        path = self.root / name
        path.write_text('#!/bin/sh\n' + body + '\n')
        path.chmod(0o755)
        return path

    def fixtures(self, row=None):
        row = row or case()
        (self.root / 'ffv1.mkv').write_bytes(b'test only')
        (self.root / 'source.raw').write_bytes(bytes(range(51)))
        (self.root / 'manifest.json').write_text(json.dumps({'cases': [row]}))

    def test_missing_unpinned_failed_and_replaced_oracles_fail(self):
        for body in ('exit 1', "echo 'not a pinned oracle'", 'exit 0'):
            ffmpeg = self.executable('ffmpeg', body)
            ffprobe = self.executable('ffprobe', body)
            with self.subTest(body=body), self.assertRaises(gate.GateError):
                gate.verify_oracles(ffmpeg, ffprobe)
        (self.root / 'ffmpeg').unlink()
        with self.assertRaises(gate.GateError):
            gate.verify_oracles(ffmpeg, ffprobe)
        build = gate.oracle_pin.load()['BUILD']
        ffmpeg = self.executable('ffmpeg', f"echo 'ffmpeg version {build}'")
        ffprobe = self.executable('ffprobe', f"echo 'ffprobe version {build}'")
        self.assertEqual(gate.verify_oracles(ffmpeg, ffprobe), (ffmpeg.resolve(), ffprobe.resolve()))
        replacement = self.executable('different-ffmpeg', f"echo 'ffmpeg version {build}'")
        with self.assertRaises(gate.GateError):
            gate.verify_oracles(replacement, ffprobe)
        ffprobe.write_text("#!/bin/sh\necho 'ffprobe version wrong'\n")
        with self.assertRaises(gate.GateError):
            gate.verify_oracles(ffmpeg, ffprobe)

    def test_zero_exit_diagnostics_and_process_failure_are_not_passes(self):
        for body in ("echo 'decode error' >&2", 'exit 9'):
            binary = self.executable('fake-oracle', body)
            with self.subTest(body=body), self.assertRaises(gate.GateError):
                gate.run([binary])
        binary = self.executable('fake-oracle', "printf '0123456789'")
        with self.assertRaises(gate.GateError):
            gate.run([binary], output_limit=9)

    def test_oracle_version_diagnostic_is_not_accepted(self):
        build = gate.oracle_pin.load()['BUILD']
        ffmpeg = self.executable('ffmpeg', f"echo 'ffmpeg version {build}'\necho error >&2")
        ffprobe = self.executable('ffprobe', f"echo 'ffprobe version {build}'")
        with self.assertRaises(gate.GateError):
            gate.verify_oracles(ffmpeg, ffprobe)

    def test_timeout_fails_and_reaps_the_child(self):
        with patch.object(gate, 'TIMEOUT', 0.05), self.assertRaisesRegex(gate.GateError, 'timed out'):
            gate.run([sys.executable, '-c', 'import time; time.sleep(10)'])

    def test_manifest_rejects_missing_unknowns_duplicates_and_escaping_paths(self):
        self.fixtures()
        self.assertEqual(len(gate.manifest(self.root)), 1)
        for mutation in ('missing-colour', 'duplicate', 'escape', 'absolute', 'too-large', 'bad-clock'):
            row = case()
            rows = [row]
            if mutation == 'missing-colour':
                del row['color']['color_range']
            elif mutation == 'duplicate':
                rows.append(copy.deepcopy(row))
            elif mutation == 'escape':
                row['file'] = '../outside.mkv'
            elif mutation == 'absolute':
                row['raw'] = str(self.root / 'source.raw')
            elif mutation == 'too-large':
                row['width'] = 16384
                row['height'] = 16384
            else:
                row['pts_ns'][1] += 1
            (self.root / 'manifest.json').write_text(json.dumps({'cases': rows}))
            with self.subTest(mutation=mutation), self.assertRaises(gate.GateError):
                gate.manifest(self.root)

    def test_wrong_oracle_causes_a_failed_report_not_a_skip(self):
        work = self.root / 'work'
        with patch.object(gate, 'verify_oracles', side_effect=gate.GateError('wrong oracle')):
            self.assertEqual(gate.main(['--ffmpeg', 'wrong', '--ffprobe', 'wrong', '--coreav', 'wrong',
                                        '--fixtures', str(self.root), '--work', str(work)]), 1)
        self.assertEqual(json.loads((work / 'results.json').read_text())['result'], 'failed')

    def test_native_format_mismatch_fails_before_any_decode_or_conversion(self):
        self.fixtures()
        coreav = self.executable('coreav', 'exit 0')
        observed = document()
        observed['streams'][0]['pix_fmt'] = 'yuv444p'
        with patch.object(gate, 'verify_oracles', return_value=('ffmpeg', 'ffprobe')), \
                patch.object(gate, 'probe', return_value=observed), patch.object(gate, 'decode') as decode:
            self.assertEqual(gate.main(['--ffmpeg', 'ffmpeg', '--ffprobe', 'ffprobe', '--coreav', str(coreav),
                                        '--fixtures', str(self.root), '--work', str(self.root / 'work')]), 1)
        decode.assert_not_called()


class CliCoverage(unittest.TestCase):
    def exercise(self, work, mutation=None):
        commands = []

        def run(argv, **kwargs):
            argv = [str(value) for value in argv]
            commands.append((argv, kwargs))
            if argv[0] == 'ffmpeg' and argv[-1].endswith('.h264'):
                Path(argv[-1]).write_bytes(b'synthetic packet')
            if argv[0] == 'coreav':
                env = kwargs['env']
                if '-y' in argv:
                    self.assertEqual(env['COREAV_STRICT_SAFE'], '0')
                else:
                    self.assertIn(env['COREAV_STRICT_SAFE'], ('0', '1'))
                self.assertEqual(env['COREAV_FFMPEG'], str(work / 'renamed-failing-encoder'))
                self.assertEqual(env['COREAV_X264'], env['COREAV_FFMPEG'])
                self.assertEqual(env['COREAV_FALLBACK'], str(work / 'absent-fallback'))
                if mutation == 'encoder':
                    (work / 'encoder-invoked').write_text('invoked')
                if argv[-1].endswith('terminal-frame-change.mkv'):
                    if mutation == 'late-fallback-log':
                        (work / 'fallback.jsonl').write_text('{"not_run":"absent"}\n')
                    if mutation == 'late-destination':
                        Path(argv[-1]).write_text('replaced')
                    return subprocess.CompletedProcess(argv, 4 if mutation == 'late-fallback' else 1,
                                                       b'', b'video geometry changes')
                if '-y' in argv:
                    if mutation == 'destination' and argv[-1] != '-':
                        Path(argv[-1]).write_text('replaced')
                    return subprocess.CompletedProcess(argv, 4, b'', b'unsupported')
                if mutation == 'fallback':
                    (work / 'fallback.jsonl').write_text('{"ran":"external"}\n')
            return subprocess.CompletedProcess(argv, 0, b'', b'')

        def probe(_ffprobe, path):
            observed = document()
            observed['streams'][0].update(width=32, height=24, display_aspect_ratio='16:9')
            for frame in observed['frames']:
                frame.update(width=32, height=24)
            if path.name.startswith('source'):
                observed['streams'][0]['codec_name'] = 'h264'
            if path.name.startswith('source-'):
                observed['streams'][0]['pix_fmt'] = path.stem.removeprefix('source-')
            if path.name == 'changing.mkv':
                observed['frames'] = [{'width': 32, 'height': 24}, {'width': 64, 'height': 24}]
            return observed

        with patch.object(gate, 'run', side_effect=run), patch.object(gate, 'probe', side_effect=probe), \
                patch.object(gate, 'decode', return_value=b'\0' * 3456):
            records = []
            gate.cli_cases('coreav', 'ffmpeg', 'ffprobe', work, records)
        return commands, records

    def test_cli_covers_both_containers_options_native_proof_and_preserving_refusals(self):
        with tempfile.TemporaryDirectory() as directory:
            commands, records = self.exercise(Path(directory))
        exact = [row['case'] for row in records if row['result'] == 'exact']
        self.assertEqual(exact, [f'cli-{container}-{index}' for container in ('mp4', 'mkv') for index in range(5)])
        for container in ('mp4', 'mkv'):
            argv = next(argv for argv, _ in commands if argv[-1].endswith(f'source.{container}'))
            self.assertEqual(argv[argv.index('-vf') + 1],
                             'setsar=4/3,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709')
        modes = {kwargs['env']['COREAV_STRICT_SAFE'] for argv, kwargs in commands
                 if argv[0] == 'coreav' and '-y' not in argv}
        self.assertEqual(modes, {'0', '1'})
        refused = {row['case'] for row in records if row['case'].startswith('refused-')}
        for name in ('container-mp4', 'container-webm', 'container-avi', 'container-mka', 'container-arbitrary',
                     'stdout', 'explicit-stdout', 'explicit-arbitrary', 'muxer', 'raw', 'image',
                     'pixel-format', 'filter', 'resize', 'rate', 'seek',
                     'input-seek', 'trim', 'bitrate', 'quality', 'preset', 'tune', 'qscale', 'yuv420p10le',
                     'gbrp', 'extra-audio', 'extra-subtitle', 'extra-video'):
            self.assertIn('refused-' + name, refused)
        for kind, disabled in (('audio', '-an'), ('subtitle', '-sn')):
            argv = next(argv for argv, _ in commands if argv[-1].endswith(f'refused-extra-{kind}.mkv'))
            self.assertNotIn(disabled, argv)
        argv = next(argv for argv, _ in commands if argv[-1].endswith('cli-mp4-4.mkv'))
        self.assertNotIn('-an', argv)
        self.assertIn({'case': 'terminal-frame-change',
                       'result': 'failed, destination preserved, no fallback'}, records)

    def test_encoder_execution_fallback_and_destination_replacement_fail(self):
        for mutation in ('encoder', 'fallback', 'destination', 'late-fallback', 'late-fallback-log', 'late-destination'):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                with self.assertRaises(gate.GateError):
                    self.exercise(Path(directory), mutation)


if __name__ == '__main__':
    unittest.main()
