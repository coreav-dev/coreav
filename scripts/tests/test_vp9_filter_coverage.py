import copy
import hashlib
import importlib.util
import json
import pathlib
import struct
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
from fnmatch import fnmatchcase

PATH = pathlib.Path(__file__).resolve().parents[1] / 'vp9_filter_coverage.py'
SPEC = importlib.util.spec_from_file_location('vp9_filter_coverage', PATH)
coverage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(coverage)
frames = coverage.frames


def template(q=20, tile_rows=0):
    prefix = '10000010' + f'{0x498342:024b}' + '0010'
    prefix += f'{31:016b}' * 2 + '0' + '00' + '00'
    loop = '100000' + '000' + '11' + '1' + '0000010' + '00000'
    quant = f'{q:08b}' + '000'
    bits = prefix + loop + quant + '0' + str(tile_rows) + f'{1:016b}'
    bits += '0' * ((-len(bits)) % 8)
    return bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits), 8)) + b'\x00synthetic-tile'


class Rewriting(unittest.TestCase):
    def test_all_controls_keep_nonfilter_syntax_and_entropy(self):
        original = frames.keyframe(template())
        self.assertTrue(original.control['deltas'])
        plan = coverage.registration()
        for level in plan['levels']:
            for setting in plan['variants'].values():
                with self.subTest(level=level, setting=setting):
                    packet = frames.rewrite(original, level, setting)
                    changed = frames.keyframe(packet)
                    self.assertEqual(changed.control, dict(level=level, deltas=False, **setting))
                    self.assertEqual(changed.entropy, original.entropy)
                    self.assertEqual(changed.quantization, original.quantization)
                    self.assertEqual(changed.prefix, original.prefix)
                    self.assertEqual(changed.suffix, original.suffix)
                    self.assertEqual(frames.ivf(frames.wrap(packet)), packet)

    def test_negative_delta_uses_a_magnitude_and_sign(self):
        original = frames.keyframe(template())
        setting = coverage.registration()['variants']['minus']
        packet = frames.rewrite(original, 32, setting)
        bits = ''.join(f'{byte:08b}' for byte in packet)
        start = len(original.prefix) + 10 + len(original.quantization)
        self.assertEqual(bits[start:start + 13], '1010' + '01' + '0100001')

    def test_invalid_controls_are_refused(self):
        original = frames.keyframe(template())
        good = coverage.registration()['variants']['plus']
        for field, value in [('alt_l', 64), ('alt_l', True), ('sharpness', 8),
                             ('segmentation', False), ('absolute', 'yes')]:
            with self.subTest(field=field), self.assertRaises(ValueError):
                frames.rewrite(original, 32, dict(good, **{field: value}))
        with self.assertRaises(ValueError):
            frames.rewrite(original, 32, dict(good, alt_l=-16, absolute=True))
        for level in (0, 64, True):
            with self.subTest(level=level), self.assertRaises(ValueError):
                frames.rewrite(original, level, good)

    def test_map_updates_unrelated_features_and_negative_absolute_data_are_refused(self):
        original = frames.keyframe(template())
        packet = frames.rewrite(original, 32, coverage.registration()['variants']['plus'])
        start = len(original.prefix) + 10 + len(original.quantization)
        control = ''.join(f'{byte:08b}' for byte in packet)
        for positions in ((start + 1,), (start + 4,), (start + 3, start + 12)):
            bits = list(control)
            for position in positions:
                bits[position] = '1'
            changed = bytes(int(''.join(bits[i:i + 8]), 2) for i in range(0, len(bits), 8))
            with self.subTest(positions=positions), self.assertRaises(ValueError):
                frames.keyframe(changed)

    def test_truncation_lossless_and_extra_ivf_frames_are_refused(self):
        for data in (b'', template()[:8], template(q=0), template(tile_rows=1)):
            with self.subTest(data=data[:8]), self.assertRaises(ValueError):
                frames.keyframe(data)
        encoded = frames.wrap(template())
        for data in (encoded[:-1], encoded + b'extra', encoded[:32] + struct.pack('<IQ', 1, 0) + encoded[44:]):
            with self.assertRaises(ValueError):
                frames.ivf(data)

    def test_picture_bytes_are_bounded_reproducible_and_planar(self):
        for pattern in ('steps', 'ramp', 'texture'):
            raw = frames.picture(pattern, 16, 16)
            self.assertEqual(raw, frames.picture(pattern, 16, 16))
            self.assertEqual(len(raw), 384)
            self.assertEqual(raw[256:], bytes([128]) * 128)
        self.assertEqual(frames.picture('steps', 16, 16)[:4], bytes([80, 80, 80, 80]))
        self.assertEqual(frames.picture('ramp', 16, 16)[:4], bytes([64, 67, 70, 73]))
        for width in (0, 15, 129, True):
            with self.assertRaises(ValueError):
                frames.picture('steps', width, 16)


def report(mode='measure'):
    plan = coverage.registration()
    width, height, _, _ = coverage.grid(plan, mode)
    rows = []
    for pattern, level, variant in coverage.roster(plan, mode):
        setting = plan['variants'][variant]
        alt = setting['alt_l']
        effective = level if alt is None else (alt if setting['absolute'] else max(0, min(63, level + alt)))
        md5 = hashlib.md5(f'{effective}:{setting["sharpness"]}'.encode()).hexdigest()
        native = dict(kind='frame', packet=0, frame=0, pts=0, width=width, height=height,
                      size=width * height * 3 // 2, md5=md5)
        rows.append(dict(pattern=pattern, level=level, variant=variant,
            control=dict(level=level, deltas=False, **setting), input_sha256='a' * 64,
            packet_sha256='b' * 64, entropy_sha256='c' * 64, encoded_sha256='d' * 64,
            raw_sha256=coverage.sha(frames.picture(pattern, width, height)),
            meta=dict(kind='stream', stream_index=0, profile=0, bit_depth=8,
                      subsampling=[1, 1], time_base=[1, 1000]),
            footer=dict(kind='complete', scope='first120', fed=1, packets=1, frames=1, cutoff=None,
                        coverage=dict(coded=1, sharpness=int(setting['sharpness'] != 0),
                                      segment_filter=int(alt is not None))),
            native=native, oracle={key: native[key] for key in ('pts', 'size', 'md5')},
            oracle_clock=[1, 1000]))
    return dict(schema=1, mode=mode, sources=coverage.hashes(), complete=True, commit='e' * 40,
                tools=dict(native='a' * 64, ffmpeg='b' * 64, ffprobe='c' * 64), streams=rows)


class Judgment(unittest.TestCase):
    def test_complete_roster_pixel_contrasts_and_negative_controls(self):
        value = coverage.validate(report())
        self.assertEqual(value['streams'], 54)
        self.assertTrue(value['coverage_pass'])
        self.assertTrue(all(count >= 1 for count in value['effects'].values()))

    def test_smoke_is_disjoint_and_cannot_claim_coverage(self):
        value = coverage.validate(report('smoke'))
        self.assertEqual(value['streams'], 6)
        self.assertIsNone(value['coverage_pass'])

    def test_missing_duplicate_reordered_stale_and_aborted_evidence_fail(self):
        original = report()
        for mode in ('missing', 'duplicate', 'reordered', 'stale', 'aborted', 'binary'):
            value = copy.deepcopy(original)
            if mode == 'missing':
                value['streams'].pop()
            elif mode == 'duplicate':
                value['streams'][-1] = value['streams'][0]
            elif mode == 'reordered':
                value['streams'].reverse()
            elif mode == 'stale':
                value['sources']['scripts/vp9_filter_frames.py'] = '0' * 64
            elif mode == 'binary':
                value['tools'].pop('native')
            else:
                value['complete'] = False
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                coverage.validate(value)

    def test_wrong_pixels_timestamps_entropy_controls_and_noeffect_identity_fail(self):
        original = report()
        for field in ('pixel', 'timestamp', 'entropy', 'control', 'observation', 'identity'):
            value = copy.deepcopy(original)
            row = value['streams'][1]
            if field == 'pixel':
                row['oracle']['md5'] = '0' * 32
            elif field == 'timestamp':
                row['oracle']['pts'] = 1
            elif field == 'entropy':
                row['entropy_sha256'] = '0' * 64
            elif field == 'control':
                row['control']['sharpness'] = 7
            elif field == 'observation':
                row['footer']['coverage']['segment_filter'] = 1
            else:
                row['native']['md5'] = row['oracle']['md5'] = '0' * 32
            with self.subTest(field=field), self.assertRaises(ValueError):
                coverage.validate(value)

    def test_equal_pixels_cannot_substitute_for_a_feature_witness(self):
        value = report()
        for row in value['streams']:
            row['native']['md5'] = row['oracle']['md5'] = '0' * 32
        with self.assertRaisesRegex(ValueError, 'no pixel-affecting witness'):
            coverage.validate(value)

    def test_a_comparator_that_accepts_crossed_pixels_is_detected(self):
        with mock.patch.object(coverage.corpus, 'judge', return_value=1):
            with self.assertRaisesRegex(ValueError, 'crossed negative control'):
                coverage.validate(report())

    def test_boolean_and_float_substitutes_are_not_numeric_evidence(self):
        original = report()
        for field in ('schema', 'level', 'control', 'timestamp', 'tool'):
            value = copy.deepcopy(original)
            if field == 'schema':
                value['schema'] = True
            elif field == 'level':
                value['streams'][0]['level'] = 16.0
            elif field == 'control':
                value['streams'][0]['control']['sharpness'] = False
            elif field == 'timestamp':
                value['streams'][0]['oracle']['pts'] = False
            else:
                value['tools']['native'] = True
            with self.subTest(field=field), self.assertRaises(ValueError):
                coverage.validate(value)

    def test_capture_failure_retains_only_a_closed_error_and_partial_work(self):
        with tempfile.TemporaryDirectory() as temporary:
            out = pathlib.Path(temporary) / 'report.json'
            args = SimpleNamespace(mode='smoke', out=out, ffmpeg='synthetic-ffmpeg')
            with mock.patch.object(coverage.sys, 'platform', 'linux'), \
                    mock.patch.object(coverage.os, 'uname', return_value=SimpleNamespace(machine='x86_64')), \
                    mock.patch.object(coverage.corpus, 'child', side_effect=RuntimeError('credential-marker')):
                with self.assertRaises(RuntimeError):
                    coverage.capture(args)
            value = json.loads(out.read_text())
            self.assertFalse(value['complete'])
            self.assertEqual(value['streams'], [])
            self.assertEqual(value['error'], 'RuntimeError')
            self.assertNotIn('credential-marker', out.read_text())

    def test_judged_capture_is_refused_before_any_child_outside_merged_main(self):
        args = SimpleNamespace(mode='measure')
        with mock.patch.object(coverage.sys, 'platform', 'linux'), \
                mock.patch.object(coverage.os, 'uname', return_value=SimpleNamespace(machine='x86_64')), \
                mock.patch.dict(coverage.os.environ, GITHUB_ACTIONS='true', GITHUB_REF='refs/pull/1/merge',
                                GITHUB_EVENT_NAME='pull_request'), \
                mock.patch.object(coverage.corpus, 'child') as child:
            with self.assertRaisesRegex(ValueError, 'merged main'):
                coverage.capture(args)
            child.assert_not_called()


class Workflow(unittest.TestCase):
    def test_measurement_is_explicit_and_prs_only_run_smoke(self):
        from test_registered_candidate import load_workflow
        workflow = load_workflow('vp9-filter.yml')
        jobs = workflow['jobs']
        self.assertEqual(jobs['filter-capture']['needs'], 'filter-contract')
        self.assertEqual(jobs['filter-capture']['timeout-minutes'], coverage.registration()['workflow_minutes'])
        steps = jobs['filter-capture']['steps']
        run = next(step for step in steps if step.get('name') == 'Capture the registered filter controls')
        self.assertEqual(run['env']['MODE'], "${{ github.event_name == 'workflow_dispatch' && inputs.mode || 'smoke' }}")
        self.assertIn("github.ref == 'refs/heads/main'", jobs['filter-capture']['if'])
        self.assertIn('"$MODE"', run['run'])
        triggers = workflow.get('on', workflow.get('true'))
        for event in ('push', 'pull_request'):
            for path in (*coverage.SOURCES, 'scripts/install-oracle-ffmpeg.sh', 'scripts/oracle_from_r2.py'):
                with self.subTest(event=event, path=path):
                    self.assertTrue(any(fnmatchcase(path, pattern) for pattern in triggers[event]['paths']))
        for job in jobs.values():
            for step in job['steps']:
                if 'checkout@' in step.get('uses', ''):
                    self.assertFalse(step['with']['persist-credentials'])
                self.assertNotIn('secrets.', json.dumps(step))
