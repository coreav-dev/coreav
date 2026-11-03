"""Synthetic evidence and mocked processes only; no registered command is executed."""
import copy
import fnmatch
import json
import os
import pathlib
import shutil
import sys
import tempfile
import types
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))
import png_bmp_yuv as image


def description(codec='png', width=2, height=1):
    pixel = 'bgr24' if codec == 'bmp' else 'rgb24'
    stream = {'index': 0, 'codec_type': 'video', 'codec_name': codec, 'width': width, 'height': height,
              'pix_fmt': pixel, 'time_base': '1/25', 'start_pts': 0, 'duration_ts': 1}
    frame = {'stream_index': 0, 'width': width, 'height': height, 'pix_fmt': pixel,
             'best_effort_timestamp': 0, 'duration': 1}
    return {'streams': [stream], 'frames': [frame]}


class Evidence(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name).resolve()
        self.source = self.root / 'input.mp4'
        self.source.write_bytes(b'synthetic source identity, not a media file')
        self.harness = self.root / 'image-harness'
        self.harness.write_bytes(b'synthetic executable identity')
        self.capsule = self.root / ('a' * 32)
        self.capsule.mkdir()
        self.outputs, self.normal, self.reference = (self.root / name for name in ('harness-out', 'normal', 'reference'))
        for directory in (self.outputs, self.normal, self.reference):
            directory.mkdir()
        self.args = ['-i', str(self.source), '-frames:v', '2', '%03d.png']
        self.request = {'args': self.args, 'capture': str(self.capsule)}
        self.source_listing = {'streams': [{'index': 2, 'codec_type': 'video', 'pix_fmt': 'yuv420p',
                                           'time_base': '1/1000'}],
                               'frames': [{'stream_index': 2, 'best_effort_timestamp': n * 40,
                                           'width': 2, 'height': 1} for n in range(2)]}
        self.rgb = b'\x00\x01\x02\x03\x04\xff'
        frames = []
        for ordinal in range(2):
            name, rgb_file = f'{ordinal + 1:03d}.png', f'{ordinal:06d}.rgb'
            (self.capsule / rgb_file).write_bytes(self.rgb)
            for directory in (self.outputs, self.normal, self.reference):
                (directory / name).write_bytes(b'encoded identity' + bytes([ordinal]))
            frames.append({'ordinal': ordinal, 'pts': ordinal * 40, 'presentation_time': {'num': ordinal, 'den': 25},
                           'source_format': 'yuv420p', 'color': {'matrix': None, 'full_range': None, 'chroma_location': 'Left'},
                           'width': 2, 'height': 1, 'pix_fmt': 'rgb24', 'rgb_file': rgb_file,
                           'rgb_bytes': 6, 'rgb_sha256': image.eb.sha256(self.capsule / rgb_file),
                           'output': name, 'output_sha256': image.eb.sha256(self.outputs / name)})
        self.receipt = {**image.registration(), 'schema': 'coreav.image-input/1', 'call': self.capsule.name,
                        'source_sha256': image.eb.sha256(self.source), 'input': str(self.source),
                        'harness_sha256': image.eb.sha256(self.harness), 'command': self.args,
                        'cwd': str(self.outputs), 'strict_safe': False, 'complete': True, 'stream': 2, 'frames': frames}
        self.write_receipt()
        self.scratch = self.root / 'scratch'
        self.scratch.mkdir()
        self.cx = types.SimpleNamespace(entry={'output': '%03d.png', 'args': ['-i', 'INPUT', '-frames:v', '2', 'OUTPUT']},
                                        jobs=[image.eb.Job(0, 0, 2, 'png')],
                                        run=types.SimpleNamespace(cwd=self.normal, args=self.args),
                                        reference=types.SimpleNamespace(cwd=self.reference, args=self.args),
                                        tc=types.SimpleNamespace(ffmpeg='pinned-ffmpeg', ffprobe='pinned-ffprobe'),
                                        opts=types.SimpleNamespace(env={}), scratch=self.scratch)

    def write_receipt(self):
        (self.capsule / 'receipt.json').write_text(json.dumps(self.receipt))
        (self.capsule / 'frames.json').write_text(json.dumps(self.receipt['frames']))

    def validate(self):
        return image.validate_receipt(self.capsule, self.request, self.source, self.source_listing, 2,
                                       self.outputs, image.eb.sha256(self.harness))

    def rejected(self, reason=None):
        with self.assertRaises(image.Invalid) as caught:
            self.validate()
        if reason:
            self.assertEqual(caught.exception.reason, reason)

    def test_complete_sequence_receipt_and_inventory(self):
        self.assertEqual(self.validate(), self.receipt['frames'])
        self.assertEqual(image.image_names('%03d.png', self.outputs), ['001.png', '002.png'])
        self.assertEqual(set(image.same_inventory(self.normal, self.outputs)), {'001.png', '002.png'})

    def test_receipt_binds_call_registration_command_source_executable_and_execution(self):
        changes = {'schema': 'old', 'registration': 'prior', 'registration_sha256': '0' * 64,
                   'call': 'b' * 32, 'source_sha256': '0' * 64, 'input': '/another/input',
                   'harness_sha256': '0' * 64, 'command': ['different'], 'cwd': '/another/cwd',
                   'strict_safe': True, 'complete': False, 'stream': 3}
        original = copy.deepcopy(self.receipt)
        for key, value in changes.items():
            with self.subTest(field=key):
                self.receipt = copy.deepcopy(original)
                self.receipt[key] = value
                self.write_receipt()
                self.rejected('receipt_binding')

    def test_boolean_numeric_bindings_and_missing_completion_fail(self):
        for key, value in (('stream', True), ('complete', 1), ('strict_safe', 0)):
            original = self.receipt[key]
            self.receipt[key] = value
            self.write_receipt()
            self.rejected('receipt_binding')
            self.receipt[key] = original
        del self.receipt['complete']
        self.write_receipt()
        self.rejected('receipt_binding')

    def test_source_and_executable_replacement_are_rejected(self):
        for path in (self.source, self.harness):
            original = path.read_bytes()
            path.write_bytes(original + b'changed')
            self.rejected('receipt_binding')
            path.write_bytes(original)

    def test_malformed_duplicate_nonfinite_and_nonobject_json_fail(self):
        for data in ('{"complete":true,"complete":false}', '{', '[]', 'NaN'):
            (self.capsule / 'receipt.json').write_text(data)
            self.rejected()

    def test_frame_identity_and_layout_mutations_fail(self):
        changes = {'ordinal': True, 'pts': 1 << 63, 'presentation_time': {'num': 0, 'den': 0},
                   'source_format': 'yuv422p', 'width': 3, 'height': 0, 'pix_fmt': 'rgba',
                   'rgb_bytes': 5, 'rgb_sha256': '0' * 64, 'rgb_file': '../outside.rgb',
                   'output': '../001.png', 'output_sha256': '0' * 64,
                   'color': {'matrix': True, 'full_range': None, 'chroma_location': 'Left'}}
        original = copy.deepcopy(self.receipt)
        for key, value in changes.items():
            with self.subTest(field=key):
                self.receipt = copy.deepcopy(original)
                self.receipt['frames'][0][key] = value
                self.write_receipt()
                self.rejected()

    def test_missing_duplicated_reordered_and_nonstrict_times_fail(self):
        original = copy.deepcopy(self.receipt)
        variants = ([], original['frames'][:1], original['frames'][::-1],
                    [original['frames'][0], original['frames'][0]])
        for frames in variants:
            with self.subTest(frames=frames):
                self.receipt = copy.deepcopy(original)
                self.receipt['frames'] = copy.deepcopy(frames)
                self.write_receipt()
                self.rejected()
        self.receipt = copy.deepcopy(original)
        self.receipt['frames'][1]['presentation_time'] = {'num': 0, 'den': 25}
        self.write_receipt()
        self.rejected('receipt_frames')

    def test_payload_truncation_append_and_one_sample_change_fail(self):
        path = self.capsule / '000000.rgb'
        for data in (self.rgb[:-1], self.rgb + b'\0', bytes([1]) + self.rgb[1:]):
            path.write_bytes(data)
            self.rejected('capture_payload')

    def test_missing_extra_symlink_and_disagreeing_frame_rosters_fail(self):
        path = self.capsule / 'extra'
        path.write_bytes(b'extra')
        self.rejected('capture_roster')
        path.unlink()
        (self.capsule / 'frames.json').write_text('[]')
        self.rejected('receipt_frames')
        self.write_receipt()
        path = self.capsule / '000000.rgb'
        path.unlink()
        path.symlink_to(self.source)
        self.rejected('capture_payload')

    def test_capture_budgets_fail_instead_of_comparing_a_prefix(self):
        with mock.patch.object(image, 'MAX_FRAMES', 1):
            self.rejected('receipt_frames')
        with mock.patch.object(image, 'MAX_RGB', 11):
            self.rejected('capture_payload')
        with mock.patch.object(image, 'MAX_JSON', 2):
            self.rejected('receipt_json')

    def test_known_source_color_and_full_range_aliases_bind_the_capture(self):
        source = self.source_listing['streams'][0]
        for field, value in (('color_range', 'pc'), ('color_space', 'bt709'), ('chroma_location', 'center')):
            source[field] = value
            self.rejected('source_identity')
            del source[field]
        source['pix_fmt'] = 'yuvj420p'
        self.rejected('source_identity')
        for frame in self.receipt['frames']:
            frame['color']['full_range'] = True
        self.write_receipt()
        self.assertEqual(self.validate(), self.receipt['frames'])

    def test_interior_source_clock_and_geometry_mutations_fail(self):
        original = copy.deepcopy(self.receipt)
        for key, value in (('pts', 41), ('presentation_time', {'num': 1, 'den': 26}), ('width', 3)):
            self.receipt = copy.deepcopy(original)
            self.receipt['frames'][1][key] = value
            self.write_receipt()
            self.rejected('source_identity')
        self.receipt = copy.deepcopy(original)
        self.receipt['frames'][1]['width'], self.receipt['frames'][1]['height'] = 1, 2
        self.write_receipt()
        self.validate()

    def test_unspecified_capture_chroma_uses_the_existing_left_default(self):
        self.source_listing['streams'][0]['chroma_location'] = 'left'
        for frame in self.receipt['frames']:
            frame['color']['chroma_location'] = None
        self.write_receipt()
        self.validate()
        self.source_listing['streams'][0]['chroma_location'] = 'center'
        self.rejected('source_identity')

    def test_sequence_names_are_numeric_complete_and_canonical(self):
        for name in ('001.png', '002.png'):
            (self.outputs / name).unlink()
        for name in ('1.png', '2.png', '10.png'):
            (self.outputs / name).write_bytes(b'encoded')
        with self.assertRaises(image.Invalid):
            image.image_names('%d.png', self.outputs)
        (self.outputs / '10.png').unlink()
        for index in range(3, 12):
            (self.outputs / f'{index}.png').write_bytes(b'encoded')
        self.assertEqual(image.image_names('%d.png', self.outputs)[-2:], ['10.png', '11.png'])
        (self.outputs / '01.png').write_bytes(b'encoded')
        with self.assertRaises(image.Invalid):
            image.image_names('%d.png', self.outputs)

    def test_normal_harness_output_identity_requires_all_bytes_and_files(self):
        (self.outputs / '001.png').write_bytes(b'changed encoded output')
        with self.assertRaises(image.Invalid):
            image.same_inventory(self.normal, self.outputs)
        (self.outputs / '001.png').write_bytes((self.normal / '001.png').read_bytes())
        (self.outputs / 'extra.png').write_bytes(b'extra')
        with self.assertRaises(image.Invalid):
            image.same_inventory(self.normal, self.outputs)

    def compare(self, candidate=None, reference=None, probe=None):
        def invoke(argv, env, output):
            source = pathlib.Path(argv[argv.index('-i') + 1])
            native = 'bgr24' if self.cx.jobs[0].kind == 'bmp' else 'rgb24'
            self.assertEqual(argv[argv.index('-pix_fmt') + 1], native)
            self.assertIn('-noautorotate', argv)
            self.assertIn('-noautoscale', argv)
            data = (reference if source.parent == self.reference else candidate) or self.rgb
            if native == 'bgr24':
                data = b''.join(data[i:i + 3][::-1] for i in range(0, len(data), 3))
            output.write_bytes(data)
            return 0, b'', b''
        with mock.patch.object(image.decoded, 'probe', side_effect=probe or (lambda *a: description(self.cx.jobs[0].kind))), \
                mock.patch.object(image.decoded, 'invoke', side_effect=invoke):
            return image.image_comparisons(self.cx, self.receipt['frames'], self.capsule)

    def test_exact_png_and_bmp_controls_pass_both_gates(self):
        for codec in ('png', 'bmp'):
            self.cx.jobs[0] = image.eb.Job(0, 0, 2, codec)
            result = self.compare()
            self.assertEqual(result['failures'], [])
            self.assertTrue(result['encoder']['exact'])
            self.assertTrue(result['reference']['exact'])
            self.assertEqual(result['encoder']['total_samples'], 12)

    def test_reference_only_error_does_not_blame_encoder(self):
        changed = b'\x02' + self.rgb[1:]
        result = self.compare(reference=changed)
        self.assertTrue(result['encoder']['exact'])
        self.assertFalse(result['reference']['exact'])
        self.assertEqual(result['reference']['max_abs_error'], [2, 0, 0])
        self.assertEqual(result['reference']['different_samples'], 2)
        self.assertEqual(result['reference']['different_pixels'], 2)
        self.assertEqual(result['reference']['different_sample_share'], 2 / 12)
        self.assertEqual(result['reference']['different_samples_rgb'], [2, 0, 0])
        self.assertEqual(result['reference']['squared_error_rgb'], [8, 0, 0])
        self.assertAlmostEqual(result['reference']['psnr_db'], 10 * image.math.log10(255 ** 2 * 12 / 8))
        self.assertEqual(result['failures'], ['reference_pixels'])

    def test_encoder_only_error_and_both_errors_remain_distinct(self):
        changed = b'\x02' + self.rgb[1:]
        result = self.compare(candidate=changed)
        self.assertFalse(result['encoder']['exact'])
        self.assertTrue(result['reference']['exact'])
        self.assertEqual(result['failures'], ['encoder_pixels'])
        result = self.compare(candidate=changed, reference=changed)
        self.assertEqual(result['failures'], ['encoder_and_reference_pixels'])
        self.assertFalse(result['encoder']['exact'])
        self.assertFalse(result['reference']['exact'])

    def test_wrong_but_self_consistently_hashed_capture_fails_both_pixel_gates(self):
        for frame in self.receipt['frames']:
            path = self.capsule / frame['rgb_file']
            path.write_bytes(b'\x01' + self.rgb[1:])
            frame['rgb_sha256'] = image.eb.sha256(path)
        self.write_receipt()
        self.validate()
        result = self.compare()
        self.assertEqual(result['failures'], ['encoder_and_reference_pixels'])

    def test_metadata_timing_and_structural_misses_do_not_hide_reference_metrics(self):
        def probe(path, *args):
            result = description()
            if path.parent == self.normal:
                result['streams'][0]['sample_aspect_ratio'] = '2:1'
                result['frames'][0]['duration'] = 2
            return result
        result = self.compare(reference=b'\x01' + self.rgb[1:], probe=probe)
        self.assertIn('stream_metadata', result['failures'])
        self.assertIn('frame_timing', result['failures'])
        self.assertIn('reference_pixels', result['failures'])

    def test_bad_candidate_shape_retains_reference_comparison(self):
        def probe(path, *args):
            result = description()
            if path.parent == self.normal:
                result['frames'].append(copy.deepcopy(result['frames'][0]))
            return result
        result = self.compare(probe=probe)
        self.assertFalse(result['encoder']['available'])
        self.assertTrue(result['reference']['exact'])
        self.assertIn('candidate_shape', result['failures'])

    def test_shape_mismatch_has_no_truncated_metrics(self):
        self.receipt['frames'][0]['width'] = 1
        result = self.compare()
        self.assertFalse(result['encoder']['available'])
        self.assertFalse(result['reference']['available'])
        other = self.root / 'short.rgb'
        other.write_bytes(self.rgb[:-1])
        self.assertFalse(image.statistics(self.capsule / '000000.rgb', other)['available'])

    def test_native_reference_format_and_exactly_one_frame_are_required(self):
        for key, value in (('pix_fmt', 'rgba'), ('codec_name', 'jpeg'), ('width', 0)):
            doc = description()
            doc['streams'][0][key] = value
            with mock.patch.object(image.decoded, 'probe', return_value=doc), self.assertRaises(image.Invalid) as caught:
                image.image_description(self.source, 'ffprobe', {}, 'png', True)
            self.assertEqual(caught.exception.reason, 'reference_shape')

    def test_unrelated_and_rgb_commands_delegate_without_replacing_the_old_oracle(self):
        self.cx.jobs = [image.eb.Job(0, 0, 2, 'flac')]
        with mock.patch.object(image.eb, 'oracle', return_value=('oracle', {'check': 'old'})) as oracle:
            self.assertEqual(image.oracle(self.cx), ('oracle', {'check': 'old'}))
            oracle.assert_called_once_with(self.cx)
        self.cx.jobs = [image.eb.Job(0, 0, 2, 'png')]
        self.source_listing['streams'][0]['pix_fmt'] = 'rgb24'
        with mock.patch.object(image.eb, 'source_of', return_value=('fixture', self.source)), \
                mock.patch.object(image.decoded, 'probe', return_value=self.source_listing), \
                mock.patch.object(image.eb, 'oracle', return_value=None) as oracle:
            self.assertIsNone(image.oracle(self.cx))
            oracle.assert_called_once_with(self.cx)

    def test_pipes_update_and_multiple_streams_remain_unjudged(self):
        with mock.patch.object(image.eb, 'source_of', return_value=('fixture', self.source)), \
                mock.patch.object(image.decoded, 'probe', return_value=self.source_listing):
            for output, args in (('-', ['-i', 'INPUT', 'OUTPUT']),
                                 ('out.png', ['-i', 'INPUT', '-update', '1', 'OUTPUT']),
                                 ('out.png', ['-i', 'INPUT', '-f', 'rawvideo', 'OUTPUT'])):
                self.cx.entry = {'output': output, 'args': args}
                self.assertEqual(image.oracle(self.cx), ('no_oracle', {'check': 'reference_shape'}))

    def test_capture_launch_uses_fresh_explicit_request_and_does_not_accept_failed_harness(self):
        self.cx.base = self.root / 'case'
        self.cx.base.mkdir()
        self.cx.assignment = ['fixture.mp4']
        self.cx.opts.image_harness = str(self.harness)
        self.cx.opts.tools = {'image_harness': image.eb.sha256(self.harness)}
        run = types.SimpleNamespace(args=self.args, cwd=self.outputs)
        for code, timed_out, reason in ((1, False, 'harness_failed'), (0, True, 'harness_timeout')):
            with mock.patch.object(image.eb, 'prepare', return_value=run), \
                    mock.patch.object(image.eb, 'execute', return_value=image.eb.Ran(code, timed_out, '')) as execute, \
                    self.assertRaises(image.Invalid) as caught:
                image.image_oracle(self.cx, self.source, self.source_listing)
            self.assertEqual(caught.exception.reason, reason)
            argv, cwd, env, seconds = execute.call_args.args
            self.assertEqual(argv[1:], ['--ignored', '--exact', 'capture_image_inputs', '--test-threads', '1'])
            request = json.loads(pathlib.Path(env['COREAV_IMAGE_REQUEST']).read_text())
            self.assertEqual(request['args'], self.args)
            self.assertRegex(pathlib.Path(request['capture']).name, r'^[0-9a-f]{32}$')
            self.assertFalse(pathlib.Path(request['capture']).exists())

    def test_complete_mocked_harness_acquisition_validates_request_and_both_gates(self):
        self.cx.base = self.root / 'case'
        self.cx.base.mkdir()
        self.cx.assignment = ['fixture.mp4']
        self.cx.opts.image_harness = str(self.harness)
        self.cx.opts.tools = {'image_harness': image.eb.sha256(self.harness)}
        run = types.SimpleNamespace(args=self.args, cwd=self.outputs)
        def execute(argv, cwd, env, seconds):
            request = json.loads(pathlib.Path(env['COREAV_IMAGE_REQUEST']).read_text())
            directory = pathlib.Path(request['capture'])
            shutil.copytree(self.capsule, directory)
            receipt = copy.deepcopy(self.receipt)
            receipt['call'] = directory.name
            (directory / 'receipt.json').write_text(json.dumps(receipt))
            return image.eb.Ran(0, False, '')
        def invoke(argv, env, output):
            output.write_bytes(self.rgb)
            return 0, b'', b''
        with mock.patch.object(image.eb, 'prepare', return_value=run), \
                mock.patch.object(image.eb, 'execute', side_effect=execute), \
                mock.patch.object(image.decoded, 'probe', return_value=description()), \
                mock.patch.object(image.decoded, 'invoke', side_effect=invoke):
            result = image.image_oracle(self.cx, self.source, self.source_listing)
        self.assertEqual(result['failures'], [])
        self.assertTrue(result['encoder']['exact'])
        self.assertTrue(result['reference']['exact'])

    def test_capture_fallback_evidence_cannot_count(self):
        self.cx.base = self.root / 'case'
        self.cx.base.mkdir()
        self.cx.assignment = ['fixture.mp4']
        self.cx.opts.image_harness = str(self.harness)
        self.cx.opts.tools = {'image_harness': image.eb.sha256(self.harness)}
        run = types.SimpleNamespace(args=self.args, cwd=self.outputs)
        with mock.patch.object(image.eb, 'prepare', return_value=run), \
                mock.patch.object(image.eb, 'execute', return_value=image.eb.Ran(0, False, '')), \
                mock.patch.object(image.eb, 'fallback_lines', return_value=[{'ran': 'ffmpeg'}]), \
                self.assertRaises(image.Invalid) as caught:
            image.image_oracle(self.cx, self.source, self.source_listing)
        self.assertEqual(caught.exception.reason, 'harness_failed')


class RunIdentity(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.opts = types.SimpleNamespace(build_manifest=self.root / 'build.json')
        self.build = {'schema': 'coreav.image-build/1', 'source_sha': 'a' * 40,
                      'cargo_lock_sha256': image.eb.sha256(image.ROOT / 'Cargo.lock'),
                      'toolchain': 'rustc synthetic', 'target': 'test-target', 'binaries': {}}
        for name in ('coreav', 'harness', 'image_harness'):
            path = self.root / name
            path.write_bytes(name.encode())
            setattr(self.opts, name, path)
            self.build['binaries'][name] = {'sha256': image.eb.sha256(path),
                                             'features': ['release-full'] + (['input-capture'] if name == 'image_harness' else [])}
        self.write_build()

    def write_build(self):
        self.opts.build_manifest.write_text(json.dumps(self.build))

    def test_build_manifest_binds_all_executables_source_lockfile_features_and_toolchain(self):
        self.assertEqual(image.validate_build(self.opts, 'a' * 40), self.build)
        original = copy.deepcopy(self.build)
        for key, value in (('source_sha', 'b' * 40), ('cargo_lock_sha256', '0' * 64), ('toolchain', ''), ('target', '')):
            self.build = copy.deepcopy(original)
            self.build[key] = value
            self.write_build()
            with self.subTest(key=key), self.assertRaises(image.Invalid):
                image.validate_build(self.opts, 'a' * 40)
        for name in original['binaries']:
            for key, value in (('sha256', '0' * 64), ('features', ['other'])):
                self.build = copy.deepcopy(original)
                self.build['binaries'][name][key] = value
                self.write_build()
                with self.subTest(name=name, key=key), self.assertRaises(image.Invalid):
                    image.validate_build(self.opts, 'a' * 40)

    def test_self_consistent_partial_build_is_not_the_registered_full_build(self):
        for record in self.build['binaries'].values():
            record['features'] = ['native-h264']
        self.build['binaries']['image_harness']['features'].append('input-capture')
        self.write_build()
        with self.assertRaises(image.Invalid):
            image.validate_build(self.opts, 'a' * 40)

    def test_dirty_or_unmerged_source_refuses_before_program_or_input_acquisition(self):
        clean = types.SimpleNamespace(returncode=0, stdout='')
        head = types.SimpleNamespace(returncode=0, stdout='a' * 40)
        dirty = types.SimpleNamespace(returncode=0, stdout='?? new-file\n')
        unmerged = types.SimpleNamespace(returncode=1, stdout='')
        for replies in ([dirty], [clean, head, unmerged]):
            with mock.patch.object(image, 'validate_host'), \
                    mock.patch.object(image.subprocess, 'run', side_effect=replies), \
                    mock.patch.object(image.eb, 'oracle_programs') as programs, \
                    mock.patch.object(image.eb, 'choose') as choose, \
                    self.assertRaises(image.Invalid) as caught:
                image.cmd_run(types.SimpleNamespace())
            self.assertEqual(caught.exception.reason, 'source_commit')
            programs.assert_not_called()
            choose.assert_not_called()

    def test_clean_merged_source_returns_the_owned_commit(self):
        replies = [types.SimpleNamespace(returncode=0, stdout=value) for value in ('', 'a' * 40, '')]
        with mock.patch.object(image.subprocess, 'run', side_effect=replies) as run:
            self.assertEqual(image.validate_source_commit(), 'a' * 40)
        self.assertEqual(run.call_args.args[0][-4:], ['merge-base', '--is-ancestor', 'HEAD', 'origin/main'])

    def test_acquisition_refuses_other_hosts_before_source_or_media_reads(self):
        for system, machine in (('Darwin', 'arm64'), ('Linux', 'aarch64'), ('Windows', 'AMD64')):
            with self.subTest(system=system, machine=machine), \
                    mock.patch.object(image.platform, 'system', return_value=system), \
                    mock.patch.object(image.platform, 'machine', return_value=machine), \
                    mock.patch.object(image, 'validate_source_commit') as source, \
                    self.assertRaises(image.Invalid) as caught:
                image.cmd_run(types.SimpleNamespace())
            self.assertEqual(caught.exception.reason, 'host')
            source.assert_not_called()
        with mock.patch.object(image.platform, 'system', return_value='Linux'), \
                mock.patch.object(image.platform, 'machine', return_value='x86_64'):
            image.validate_host()

    def test_summary_rejects_prior_mixed_relabelled_and_unknown_command_rows(self):
        entry = {'class': 'media', 'container': 'image', 'args': ['-i', 'INPUT', 'OUTPUT'], 'output': 'out.png'}
        manifest = {'schema': 'coreav.image-run/1', **image.registration(),
                    'commandset_sha256': image.eb.sha256(image.eb.SET), 'methods': image.method_identity()}
        record = {**image.eb.header(0, entry), **image.registration(), 'run_identity_sha256': image.digest(manifest),
                  'counted': False, 'miss': 'no_input', 'detail': {}}
        with mock.patch.object(image.eb, 'commands', return_value=[(0, entry)]):
            self.assertEqual(image.summarize([record], manifest)['v2']['of'], 1)
            for field, value in (('registration', 'old'), ('registration_sha256', '0' * 64),
                                 ('run_identity_sha256', '0' * 64), ('index', 1), ('args', ['changed'])):
                with self.subTest(field=field), self.assertRaises(image.Invalid):
                    image.summarize([{**record, field: value}], manifest)
            self.assertTrue(image.summarize([record, record], manifest)['void']['void'])
            self.assertTrue(image.summarize([], manifest)['void']['void'])
            with self.assertRaises(image.Invalid):
                image.summarize([{**record, 'miss': None}], manifest)

    def test_no_default_registration_selection(self):
        with mock.patch.object(sys, 'argv', ['png_bmp_yuv.py', 'run']), self.assertRaises(SystemExit) as caught:
            image.main()
        self.assertEqual(caught.exception.code, 2)

    def test_standalone_acquisition_keeps_all_v2_rows_and_writes_identity(self):
        for name in ('ffmpeg', 'ffprobe', 'x264'):
            path = self.root / name
            path.write_bytes(name.encode())
            setattr(self.opts, name, path)
        fixtures = self.root / 'fixtures'
        fixtures.mkdir()
        for name in image.eb.FIXTURES:
            (fixtures / name).write_bytes(name.encode())
        self.opts.fixtures = fixtures
        self.opts.work = self.root / 'work'
        self.opts.records = self.root / 'records.jsonl'
        self.opts.summary = self.root / 'summary.json'
        self.opts.manifest = self.root / 'manifest.json'
        self.opts.jobs, self.opts.mediainfo = 1, None
        entries = [(index, {'class': 'media', 'container': 'image', 'args': ['-i', 'INPUT', 'OUTPUT'],
                            'output': 'out.png'}) for index in range(3)]
        def evaluate(opts, index, entry):
            return {**image.eb.header(index, entry), **image.registration(), 'run_identity_sha256': opts.run_identity,
                    'counted': False, 'miss': 'no_input', 'detail': {}}
        with mock.patch.object(image, 'validate_host'), \
                mock.patch.object(image, 'validate_source_commit', return_value='a' * 40), \
                mock.patch.object(image.eb, 'oracle_programs', return_value={'ffmpeg': {'verified': True}}) as oracle, \
                mock.patch.object(image.eb, 'commands', return_value=entries) as commands, \
                mock.patch.object(image, 'evaluate', side_effect=evaluate) as acquire, \
                mock.patch.object(image.eb, 'brief', return_value=0):
            self.assertEqual(image.cmd_run(self.opts), 0)
        self.assertEqual(acquire.call_count, 3)
        self.assertTrue(all(call.args == ('v2',) for call in commands.call_args_list))
        oracle.assert_called_once_with(self.opts)
        manifest = json.loads(self.opts.manifest.read_text())
        self.assertEqual(set(manifest['tools']), {'coreav', 'harness', 'image_harness', 'ffmpeg', 'ffprobe', 'x264'})
        self.assertEqual(set(manifest['inputs']), set(image.eb.FIXTURES))
        self.assertEqual(manifest['build'], self.build)
        records = [json.loads(row) for row in self.opts.records.read_text().splitlines()]
        self.assertEqual([row['index'] for row in records], [0, 1, 2])
        self.assertTrue(all(row['run_identity_sha256'] == image.digest(manifest) for row in records))
        self.assertEqual(json.loads(self.opts.summary.read_text())['v2']['of'], 3)


class Orchestrator(unittest.TestCase):
    def test_existing_failure_and_encoder_start_order_precede_image_oracle(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            entry = {'class': 'media', 'container': 'image', 'args': ['-i', 'INPUT', 'OUTPUT'], 'output': 'out.png'}
            opts = types.SimpleNamespace(run_identity='run', ffmpeg='oracle', env={}, tc=object())
            reference = types.SimpleNamespace(cwd=root / 'ref', args=[])
            prepared = types.SimpleNamespace(cwd=root, args=[])
            c = types.SimpleNamespace(ran=image.eb.Ran(0, False, ''), run=prepared, lines=[], starts=[])
            with mock.patch.object(image.eb, 'choose', return_value=(['fixture'], reference)), \
                    mock.patch.object(image.eb, 'prepare', return_value=prepared), \
                    mock.patch.object(image.eb, 'execute', return_value=image.eb.Ran(0, False, '')), \
                    mock.patch.object(image.eb, 'coreav_run', return_value=c), \
                    mock.patch.object(image.eb, 'failure', return_value=('refused', {'reason': 'existing'})), \
                    mock.patch.object(image, 'oracle') as oracle:
                result = image.judge(opts, 0, entry, root)
            self.assertEqual(result['miss'], 'refused')
            oracle.assert_not_called()
            c.starts = [{'encoder': False}]
            with mock.patch.object(image.eb, 'choose', return_value=(['fixture'], reference)), \
                    mock.patch.object(image.eb, 'prepare', return_value=prepared), \
                    mock.patch.object(image.eb, 'execute', return_value=image.eb.Ran(0, False, '')), \
                    mock.patch.object(image.eb, 'coreav_run', return_value=c), \
                    mock.patch.object(image.eb, 'failure', return_value=None), \
                    mock.patch.object(image, 'oracle') as oracle:
                result = image.judge(opts, 0, entry, root)
            self.assertEqual(result['miss'], 'ffmpeg_job')
            oracle.assert_not_called()

    def test_success_retains_image_diagnostics_and_runs_existing_listing_check(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            entry = {'class': 'media', 'container': 'image', 'args': ['-i', 'INPUT', 'OUTPUT'], 'output': 'out.png'}
            opts = types.SimpleNamespace(run_identity='run', ffmpeg='oracle', env={}, tc=object())
            prepared = types.SimpleNamespace(cwd=root, args=[])
            c = types.SimpleNamespace(ran=image.eb.Ran(0, False, ''), run=prepared, lines=[], starts=[])
            metrics = {'encoder': {'exact': True}, 'reference': {'exact': True}, 'failures': []}
            with mock.patch.object(image.eb, 'choose', return_value=(['fixture'], prepared)), \
                    mock.patch.object(image.eb, 'prepare', return_value=prepared), \
                    mock.patch.object(image.eb, 'execute', return_value=image.eb.Ran(0, False, '')), \
                    mock.patch.object(image.eb, 'coreav_run', return_value=c), \
                    mock.patch.object(image.eb, 'failure', return_value=None), \
                    mock.patch.object(image, 'oracle', return_value=(None, metrics)), \
                    mock.patch.object(image.eb, 'listings', return_value=None) as listings:
                result = image.judge(opts, 0, entry, root)
            self.assertTrue(result['counted'])
            self.assertIsNone(result['miss'])
            self.assertEqual(result['detail']['image_comparisons'], metrics)
            listings.assert_called_once()


class WorkflowScope(unittest.TestCase):
    def test_synthetic_workflow_covers_build_inputs_without_running_a_registered_roster(self):
        text = (image.ROOT / '.github/workflows/png-bmp-yuv.yml').read_text()
        for path in ('crates/**', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'scripts/**',
                     'tools/compat-corpus/**', 'docs/design/png-bmp-yuv-v1.md', '.github/workflows/png-bmp-yuv.yml'):
            self.assertEqual(text.count('      - ' + path + '\n'), 2, path)
        self.assertIn('runs-on: ubuntu-24.04', text)
        self.assertIn('COREAV_IMAGE_VP9:', text)
        self.assertIn('--test image_capture', text)
        self.assertIn('--features native-h264,libvpx,input-capture', text)
        self.assertIn('libvpx-dev', text)
        self.assertNotIn('encode_breadth.py run', text)
        self.assertNotIn('png_bmp_yuv.py run', text)

    def test_this_change_does_not_trigger_the_old_judged_workflow(self):
        text = (image.ROOT / '.github/workflows/codec-coverage.yml').read_text()
        patterns = [line.removeprefix('      - ') for line in text.splitlines() if line.startswith('      - ')]
        for path in ('Cargo.lock', 'crates/coreav-cli/Cargo.toml', 'crates/coreav-cli/src/ffmpeg/frames.rs',
                     'crates/coreav-cli/src/ffmpeg/mod.rs', 'crates/coreav-cli/src/ffmpeg/image_capture.rs',
                     'crates/coreav-cli/tests/image_capture.rs', 'tools/compat-corpus/png_bmp_yuv.py',
                     'tools/compat-corpus/tests/test_png_bmp_yuv.py', 'docs/design/png-bmp-yuv-v1.md',
                     '.github/workflows/png-bmp-yuv.yml'):
            self.assertFalse(any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns), path)


@unittest.skipUnless(os.environ.get('COREAV_IMAGE_ORACLE') == '1', 'opt-in pinned-oracle synthetic controls')
class RealImageCapture(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        names = ('COREAV', 'COREAV_IMAGE_HARNESS', 'COREAV_FFMPEG', 'COREAV_FFPROBE')
        missing = [name for name in names if not os.environ.get(name)]
        if missing:
            raise AssertionError('missing required image-control programs: ' + ', '.join(missing))
        cls.programs = {name: str(pathlib.Path(os.environ[name]).resolve(strict=True)) for name in names}
        opts = types.SimpleNamespace(ffmpeg=cls.programs['COREAV_FFMPEG'], ffprobe=cls.programs['COREAV_FFPROBE'])
        if sys.platform == 'darwin':
            import oracle_pin_macos
            directory = pathlib.Path(opts.ffmpeg).parent
            if pathlib.Path(opts.ffmpeg) != directory / 'ffmpeg' or pathlib.Path(opts.ffprobe) != directory / 'ffprobe':
                raise AssertionError('synthetic controls require the installed pinned executable pair')
            problems = list(oracle_pin_macos.verify_bin(directory))
            if problems:
                raise AssertionError('; '.join(problems))
            cls.verified = {name: image.eb.sha256(getattr(opts, name)) for name in ('ffmpeg', 'ffprobe')}
        else:
            cls.verified = image.eb.oracle_programs(opts)

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='coreav-image-control-')
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name).resolve()
        self.fixtures = self.root / 'inputs'
        self.fixtures.mkdir()
        self.env = image.eb.environment()
        self.env.update(COREAV_FALLBACK=image.eb.NO_FALLBACK, COREAV_FFMPEG=image.eb.NO_FALLBACK,
                        COREAV_X264=image.eb.NO_FALLBACK)
        self.ffmpeg = self.programs['COREAV_FFMPEG']
        self.ffprobe = self.programs['COREAV_FFPROBE']

    def checked(self, argv, cwd=None):
        code, stdout, stderr = image.decoded.invoke(argv, self.env, cwd=cwd)
        self.assertEqual(code, 0, stderr.decode(errors='replace'))
        return stdout

    def source(self, textured=False, vp9=False):
        output = self.fixtures / (('texture' if textured else 'neutral') + ('.webm' if vp9 else '.mp4'))
        if textured:
            before = ['-f', 'lavfi', '-i', 'testsrc2=size=18x10:rate=2:duration=1']
        else:
            raw = self.root / 'neutral.yuv'
            chroma = bytes([128]) * (18 * 10 // 2)
            raw.write_bytes(bytes(18 * 10) + chroma + bytes([255]) * (18 * 10) + chroma)
            before = ['-f', 'rawvideo', '-pixel_format', 'yuv420p', '-video_size', '18x10', '-framerate', '2',
                      '-i', str(raw)]
        encoder = (['-c:v', 'libvpx-vp9', '-lossless', '1', '-cpu-used', '8', '-threads', '1'] if vp9 else
                   ['-c:v', 'libx264', '-qp', '1', '-preset', 'ultrafast', '-threads', '1', '-bf', '0', '-g', '1'])
        self.checked([self.ffmpeg, '-nostdin', '-v', 'error', *before, '-frames:v', '2', '-an',
                      *encoder, '-pix_fmt', 'yuv420p', *([] if textured else ['-color_range', 'pc']), str(output)])
        if not textured:
            listing = image.decoded.probe(output, self.ffprobe, self.env)
            self.assertIsNotNone(listing)
            restored = self.checked([self.ffmpeg, '-nostdin', '-v', 'error', '-i', str(output),
                                     '-pix_fmt', listing['streams'][0]['pix_fmt'], '-fps_mode', 'passthrough',
                                     '-f', 'rawvideo', '-'])
            self.assertEqual(restored, raw.read_bytes())
        return output

    def acquire(self, source, codec, sequence, tag):
        base = self.root / tag
        base.mkdir()
        entry = {'class': 'media', 'container': 'image',
                 'args': ['-v', 'error', '-i', 'INPUT', '-c:v', codec, '-frames:v', '2' if sequence else '1', 'OUTPUT'],
                 'output': 'frame-%03d.' + codec if sequence else 'frame.' + codec}
        opts = types.SimpleNamespace(fixtures=self.fixtures, env=self.env,
                                      image_harness=self.programs['COREAV_IMAGE_HARNESS'],
                                      tools={'image_harness': image.eb.sha256(self.programs['COREAV_IMAGE_HARNESS'])})
        assignment = (source.name,)
        normal = image.eb.prepare(opts, entry, base, 'normal', assignment)
        reference = image.eb.prepare(opts, entry, base, 'reference', assignment)
        self.checked([self.programs['COREAV'], 'ffmpeg', *normal.args], normal.cwd)
        self.checked([self.ffmpeg, *reference.args], reference.cwd)
        scratch = base / 'scratch'
        scratch.mkdir()
        listing = image.decoded.probe(source, self.ffprobe, self.env)
        self.assertIsNotNone(listing)
        videos = [s for s in listing['streams'] if s.get('codec_type') == 'video']
        self.assertEqual(len(videos), 1)
        cx = types.SimpleNamespace(opts=opts, entry=entry, base=base, assignment=assignment,
                                   jobs=[image.eb.Job(0, 0, videos[0]['index'], codec)], run=normal,
                                   reference=reference, scratch=scratch,
                                   tc=types.SimpleNamespace(ffmpeg=self.ffmpeg, ffprobe=self.ffprobe))
        result = image.image_oracle(cx, source, listing)
        request = image.document(base / 'image-request.json')
        capsule = pathlib.Path(request['capture'])
        receipt = image.document(capsule / 'receipt.json')
        harness_output = base / 'image-harness' / 'out'
        image.same_inventory(normal.cwd, harness_output)
        self.assertEqual(len(receipt['frames']), 2 if sequence else 1)
        return cx, capsule, receipt, result

    def test_neutral_png_bmp_single_and_sequences_are_exact(self):
        source = self.source()
        for codec in ('png', 'bmp'):
            for sequence in (False, True):
                with self.subTest(codec=codec, sequence=sequence):
                    cx, capsule, receipt, result = self.acquire(source, codec, sequence, f'{codec}-{sequence}')
                    self.assertEqual(result['failures'], [], result)
                    self.assertTrue(result['encoder']['exact'])
                    self.assertTrue(result['reference']['exact'])
                    self.assertEqual((capsule / receipt['frames'][0]['rgb_file']).read_bytes(), bytes(18 * 10 * 3))
                    if sequence:
                        self.assertEqual((capsule / receipt['frames'][1]['rgb_file']).read_bytes(), bytes([255]) * (18 * 10 * 3))

    def test_textured_images_keep_exact_encoder_and_report_reference_metrics(self):
        source = self.source(textured=True)
        for codec in ('png', 'bmp'):
            with self.subTest(codec=codec):
                _, _, _, result = self.acquire(source, codec, True, codec)
                self.assertTrue(result['encoder']['exact'], result)
                self.assertTrue(result['reference']['available'], result)
                self.assertEqual(result['reference']['total_samples'], 18 * 10 * 3 * 2)
                print(json.dumps({'synthetic_control': 'textured-' + codec,
                                  'encoder': result['encoder'], 'reference': result['reference']}, sort_keys=True))

    @unittest.skipUnless(os.environ.get('COREAV_IMAGE_VP9') == '1', 'Linux worker-capable capture build')
    def test_vp9_worker_preserves_normal_and_captured_outputs(self):
        source = self.source(vp9=True)
        for codec in ('png', 'bmp'):
            with self.subTest(codec=codec):
                _, _, _, result = self.acquire(source, codec, True, codec)
                self.assertEqual(result['failures'], [], result)
                self.assertTrue(result['encoder']['exact'])
                self.assertTrue(result['reference']['exact'])

    def changed_image(self, destination, codec):
        native = image.image_description(destination, self.ffprobe, self.env, codec)
        width, height = native['streams'][0]['width'], native['streams'][0]['height']
        pixels = bytearray(self.checked([self.ffmpeg, '-nostdin', '-v', 'error', '-noautorotate',
                                        '-i', str(destination), '-frames:v', '1', '-pix_fmt', 'rgb24',
                                        '-f', 'rawvideo', '-']))
        pixels[0] = 1 if pixels[0] == 0 else pixels[0] - 1
        raw = self.root / ('changed-' + codec + '.rgb')
        raw.write_bytes(pixels)
        self.checked([self.ffmpeg, '-nostdin', '-v', 'error', '-y', '-f', 'rawvideo', '-pixel_format', 'rgb24',
                      '-video_size', f'{width}x{height}', '-framerate', '25', '-i', str(raw),
                      '-frames:v', '1', '-c:v', codec, str(destination)])

    def test_real_reference_and_encoded_pixel_mutations_fail_separate_gates(self):
        source = self.source()
        for codec in ('png', 'bmp'):
            with self.subTest(codec=codec):
                cx, capsule, receipt, result = self.acquire(source, codec, False, codec)
                self.assertEqual(result['failures'], [])
                name = receipt['frames'][0]['output']
                reference = cx.reference.cwd / name
                original = reference.read_bytes()
                self.changed_image(reference, codec)
                result = image.image_comparisons(cx, receipt['frames'], capsule)
                self.assertEqual(result['failures'], ['reference_pixels'], result)
                self.assertTrue(result['encoder']['exact'])
                self.assertEqual(result['reference']['different_samples'], 1)
                reference.write_bytes(original)
                candidate = cx.run.cwd / name
                self.changed_image(candidate, codec)
                harness_output = cx.base / 'image-harness' / 'out'
                shutil.copyfile(candidate, harness_output / name)
                receipt['frames'][0]['output_sha256'] = image.eb.sha256(candidate)
                (capsule / 'receipt.json').write_text(json.dumps(receipt))
                (capsule / 'frames.json').write_text(json.dumps(receipt['frames']))
                request = image.document(cx.base / 'image-request.json')
                staged_source = pathlib.Path(request['args'][request['args'].index('-i') + 1])
                listing = image.decoded.probe(staged_source, self.ffprobe, self.env)
                frames = image.validate_receipt(capsule, request, staged_source, listing, cx.jobs[0].stream,
                                                 harness_output, cx.opts.tools['image_harness'])
                image.same_inventory(cx.run.cwd, harness_output)
                result = image.image_comparisons(cx, frames, capsule)
                self.assertEqual(result['failures'], ['encoder_pixels'], result)
                self.assertTrue(result['reference']['exact'])
                self.assertEqual(result['encoder']['different_samples'], 1)


if __name__ == '__main__':
    unittest.main()
