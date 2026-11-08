"""Synthetic controls only. No registered command or judged fixture is executed."""
import json
import copy
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
import wave
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import decoded_output
import github_shapes as run
import github_shapes_inputs as inputs
import github_shapes_set as roster
import test_oracle


def shape(tool='ffprobe', options=()):
    return {'tool': tool, 'options': list(options),
            'inputs': [{'container': 'unknown', 'container_hint': 'unknown', 'options': []}],
            'outputs': [] if tool == 'ffprobe' else [
                {'container': 'unknown', 'container_hint': 'unknown', 'options': []}]}


def row(number=1, weight=1):
    return {'rank': number, 'repository_count': weight, 'shape_id': str(number),
            'shape': shape(options=[['-show_entries', 'format=control_sentinel']])}


class SetAndInputs(unittest.TestCase):
    def test_frozen_counts_only_roster(self):
        rows = roster.load()
        self.assertEqual(sum(r['repository_count'] for r in rows), 1470)
        self.assertEqual(set(rows[0]), {'rank', 'shape_id', 'repository_count', 'shape'})
        self.assertNotIn('https://', roster.encoded(rows).decode())
        with self.assertRaises(ValueError):
            roster.validate(rows[:-1])
        changed = json.loads(json.dumps(rows))
        changed[0]['repository_count'] += 1
        with self.assertRaises(ValueError):
            roster.validate(changed)

    def test_all_rows_have_pure_representative_or_closed_miss(self):
        results = []
        for entry in roster.load():
            try:
                command = inputs.instantiate(entry['shape'])
                self.assertNotIn('<value>', command.args)
                results.append('representative')
            except inputs.Uninstantiable as error:
                self.assertIn(str(error), ('lost_value', 'lost_filter', 'lost_construction',
                                          'external_input', 'missing_value'))
                results.append('uninstantiable')
        self.assertEqual(len(results), 200)

    def test_order_selectors_and_input_options_stay_in_their_groups(self):
        value = shape('ffmpeg')
        value['inputs'][0]['options'] = [['-ss', '<value>']]
        value['outputs'][0]['options'] = [['-map', 'N:v:N'], ['-map', 'N:a'],
                                         ['-c:v', 'copy'], ['-t', '<value>']]
        command = inputs.instantiate(value)
        self.assertEqual(command.args, ('-ss', '0.2', '-i', 'in0/source.mp4', '-map', '0:v:0',
                                        '-map', '0:a', '-c:v', 'copy', '-t', '1', 'output0.mp4'))
        reference = run.expected_invocation(command, False)
        at = reference.index('-i')
        self.assertEqual(reference[at - len(run.AUDIO_DECODE_INPUT):at], list(run.AUDIO_DECODE_INPUT))
        self.assertEqual(run.expected_invocation(command, True), ['$COREAV', 'ffmpeg', *command.args])

    def test_no_guess_for_wrapper_graph_unknown_value_or_device(self):
        for option in ('-wrapper-unresolved', '-filter_complex', '-lavfi'):
            with self.subTest(option=option), self.assertRaises(inputs.Uninstantiable):
                inputs.instantiate(shape(options=[(option, 'overlay')]))
        for value in ('<codec>', '<selector>', '<enum>'):
            with self.assertRaises(inputs.Uninstantiable):
                inputs.options([('-c:v', value)])
        value = shape()
        value['inputs'][0]['container'] = 'device'
        with self.assertRaises(inputs.Uninstantiable):
            inputs.instantiate(value)

    def test_pipe_raw_and_multiple_output_roster(self):
        value = shape('ffmpeg')
        value['inputs'][0].update(container='pipe', options=[['-f', 's16le'], ['-ar', '<value>']])
        value['outputs'].append({'container': 'pipe', 'container_hint': 'unknown', 'options': []})
        command = inputs.instantiate(value)
        self.assertEqual(command.stdin, 'in0/source.s16')
        self.assertEqual(command.outputs, ('output0.mp4', '-'))
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            for name in ('image-10.png', 'image-02.png'):
                (folder / name).touch()
            self.assertEqual([p.name for p in inputs.output_files(folder, 'image-%02d.png')],
                             ['image-02.png', 'image-10.png'])
            (folder / 'image-2.png').touch()
            with self.assertRaises(ValueError):
                inputs.output_files(folder, 'image-%02d.png')


class AcquisitionAndReplay(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.bundle = self.root / 'bundle'
        self.bundle.mkdir()
        fixture = self.bundle / 'fixtures'
        fixture.mkdir()
        (fixture / 'source.mp4').write_bytes(b'synthetic control, not media')
        self.fake = self.root / 'fake'
        self.fake.write_text('#!' + sys.executable + '\nimport sys\n' +
                             'print(\'{"format":{"control_sentinel":"control result"}}\' if "json" in sys.argv '
                             'else "control_sentinel=control result")\n')
        self.fake.chmod(0o755)
        self.programs = {key: str(self.fake) for key in ('$COREAV', '$FFMPEG', '$FFPROBE', '$X264')}
        self.record = run.acquire_one(row(), self.bundle, self.programs, run.environment())

    def judge(self, record=None):
        return run.judge(row(), record or self.record, self.bundle, self.programs,
                         run.environment(), self.root)

    def test_actual_fake_probe_capture_and_rejudge(self):
        self.assertEqual(self.judge(), 'counted')
        self.record['outcome'] = 'counted'
        candidate = self.bundle / 'calls/1/candidate'
        (candidate / 'stdout').write_bytes(b'control_sentinel=wrong value\n')
        (candidate / 'command.stdout').write_bytes(b'control_sentinel=wrong value\n')
        self.assertEqual(self.judge(), 'different')

    def test_refusal_timeout_and_error_are_misses(self):
        candidate = self.record['arms']['candidate']
        candidate['code'] = 4
        self.assertEqual(self.judge(), 'refused')
        candidate['timed_out'] = True
        self.assertEqual(self.judge(), 'timeout')
        candidate.update(code=1, timed_out=False)
        self.assertEqual(self.judge(), 'failed')

    def test_missing_and_executed_fallback_void_capture(self):
        log = self.bundle / 'calls/1/candidate/fallback.jsonl'
        for text in ('not json', '{}', '{"ran":"ffmpeg"}'):
            log.write_text(text)
            with self.assertRaises((ValueError, json.JSONDecodeError)):
                self.judge()
        log.unlink()
        with self.assertRaises(OSError):
            self.judge()

    def test_substituted_command_input_or_stdout_is_rejected(self):
        self.record['arms']['candidate']['args'][0] = '/bin/true'
        with self.assertRaisesRegex(ValueError, 'invocation_binding'):
            self.judge()
        self.record['arms']['candidate']['args'][0] = '$COREAV'
        source = self.bundle / 'calls/1/candidate/in0/source.mp4'
        source.write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'input_binding'):
            self.judge()

    def test_probe_witness_is_bound_and_unavailable_or_failed_witnesses_miss(self):
        witness = self.record['eligibility']
        witness['args'][0] = '/bin/true'
        with self.assertRaisesRegex(ValueError, 'invocation_binding'):
            self.judge()
        witness['args'][0] = '$FFPROBE'
        witness['code'] = 1
        self.assertEqual(self.judge(), 'no_oracle')
        witness.update(code=0, timed_out=True)
        self.assertEqual(self.judge(), 'no_oracle')
        witness['timed_out'] = False
        (self.bundle / 'calls/1/reference/eligibility.stdout').write_bytes(b'{"format":{}}')
        self.assertEqual(self.judge(), 'no_oracle')
        self.record.pop('eligibility')
        with self.assertRaisesRegex(ValueError, 'probe_witness_missing'):
            self.judge()

    def test_unexpected_encoder_does_not_count(self):
        path = self.bundle / 'calls/1/candidate/starts.json'
        run.write(path, [{'program': 'x264', 'args': []}])
        self.assertEqual(self.judge(), 'unexpected_program')

    def test_reference_failure_keeps_the_row(self):
        ref = self.record['arms']['reference']
        ref['code'] = 1
        self.record['arms'].pop('candidate')
        self.assertEqual(self.judge(), 'reference_failed')

    def test_replay_checks_inventory_and_recomputes_instead_of_trusting_labels(self):
        opts = types.SimpleNamespace(bundle=self.bundle, scratch=self.root / 'scratch',
                                     summary=self.root / 'summary.json', **{
                                         k: str(self.fake) for k in ('coreav', 'ffmpeg', 'ffprobe', 'x264', 'pns_diagnostic')})
        identity = {'coreav': {'sha256': run.digest(self.fake)}}
        build = {'commit': 'a' * 40, 'command': run.BUILD, 'coreav_sha256': run.digest(self.fake),
                 'lock_sha256': run.digest(run.ROOT / 'Cargo.lock')}
        self.record['outcome'] = 'different'
        rows = [row(i, 1271 if i == 2 else 1) for i in range(1, 201)]
        records = [self.record]
        for entry in rows[1:]:
            entry['shape']['options'].append(['-wrapper-unresolved', '<value>'])
            records.append({'rank': entry['rank'], 'uninstantiable': 'lost_construction'})
        run.write(self.bundle / 'records.json', records)
        manifest = {'method': {}, 'programs': identity, 'features': run.FEATURES,
                    'build_command': run.BUILD, 'build': build, 'commit': 'a' * 40,
                    'inventory': run.inventory(self.bundle)}
        run.write(self.bundle / 'manifest.json', manifest)
        with mock.patch.object(run, 'method', return_value={}), \
             mock.patch.object(run, 'verify_programs', return_value=identity), \
             mock.patch.object(roster, 'load', return_value=rows):
            self.assertEqual(run.replay(opts), 0)
            summary = json.loads(opts.summary.read_text())
            self.assertEqual(summary['counted'], 1)
            self.record['outcome'] = run.COMPATIBLE
            run.write(self.bundle / 'records.json', records)
            manifest['inventory'] = run.inventory(self.bundle)
            run.write(self.bundle / 'manifest.json', manifest)
            opts.scratch = self.root / 'replay-again'
            self.assertEqual(run.replay(opts), 0)
            summary = json.loads(opts.summary.read_text())
            self.assertEqual((summary['counted'], summary[run.COMPATIBLE]), (1, 0))
            self.record['outcome'] = run.aac.COMPATIBLE
            run.write(self.bundle / 'records.json', records)
            manifest['inventory'] = run.inventory(self.bundle)
            run.write(self.bundle / 'manifest.json', manifest)
            opts.scratch = self.root / 'replay-aac-label'
            self.assertEqual(run.replay(opts), 0)
            summary = json.loads(opts.summary.read_text())
            self.assertEqual((summary['counted'], summary[run.aac.COMPATIBLE]), (1, 0))
            self.fake.write_text('changed executable')
            with mock.patch.object(run, 'verify_programs', return_value={'changed': {}}), \
                 self.assertRaisesRegex(ValueError, 'program_identity'):
                run.replay(opts)
            (self.bundle / 'calls/1/candidate/stdout').write_bytes(b'mutation')
            with self.assertRaisesRegex(ValueError, 'evidence_identity'):
                run.replay(opts)

    def test_manifest_rejects_symlinks(self):
        (self.bundle / 'escape').symlink_to(self.fake)
        with self.assertRaisesRegex(ValueError, 'evidence_file_type'):
            run.inventory(self.bundle)


class ExactPredicate(unittest.TestCase):
    def test_raw_output_description_uses_output_options_not_input_demuxer(self):
        value = shape('ffmpeg')
        value['inputs'][0]['options'] = [['-f', 's16le']]
        value['outputs'][0]['options'] = [['-f', 'mp3']]
        self.assertEqual(run.raw_description(inputs.instantiate(value), 0, None), [])
        value['outputs'][0]['options'] = [['-f', 'f32le'], ['-ar', '<value>'], ['-ac', '<value>']]
        self.assertEqual(run.raw_description(inputs.instantiate(value), 0, None),
                         ['-f', 'f32le', '-sample_rate', '48000', '-ch_layout', 'mono'])
        value['outputs'][0]['options'] = [['-f', 'f32le']]
        with self.assertRaises(ValueError):
            run.raw_description(inputs.instantiate(value), 0, None)

    def test_lossy_is_exact_not_quality_and_does_not_change_default_eligibility(self):
        stream = {'index': 0, 'codec_type': 'video', 'codec_name': 'h264', 'width': 1, 'height': 1,
                  'pix_fmt': 'yuv420p', 'time_base': '1/10', 'start_pts': 0, 'duration_ts': 1}
        frame = {'stream_index': 0, 'width': 1, 'height': 1, 'pix_fmt': 'yuv420p',
                 'best_effort_timestamp': 0, 'duration': 1}
        document = {'streams': [stream], 'frames': [frame]}
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(decoded_output, 'probe', return_value=document):
            folder = Path(tmp)
            def invoke(argv, env, output=None):
                Path(output).write_bytes(b'pixel')
                return 0, b'', b''
            with mock.patch.object(decoded_output, 'invoke', side_effect=invoke):
                args = ('candidate', 'reference', 'ffmpeg', 'ffprobe', {}, folder)
                self.assertEqual(decoded_output.compare(*args)[0], 'unverified')
                self.assertEqual(decoded_output.compare(*args, registered_codecs=run.EXACT_CODECS)[0], 'verified')
            def changed(argv, env, output=None):
                Path(output).write_bytes(b'one' if 'candidate' in argv else b'two')
                return 0, b'', b''
            with mock.patch.object(decoded_output, 'invoke', side_effect=changed):
                self.assertEqual(decoded_output.compare(*args, registered_codecs=run.EXACT_CODECS),
                                 ('failed', 'decoded pixels'))

    def test_copy_needs_packets_even_when_decoded_output_agrees(self):
        command = inputs.instantiate(shape('ffmpeg'))
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for arm in ('reference', 'candidate'):
                folder = base / arm
                folder.mkdir()
                (folder / 'output0.mp4').write_bytes(b'control')
                (folder / 'command.stderr').write_text('Stream #0:4 -> #0:0 (copy)\n')
            description = {'streams': [{'codec_name': 'h264'}]}
            programs = {'$FFMPEG': 'ffmpeg', '$FFPROBE': 'ffprobe'}
            with mock.patch.object(decoded_output, 'probe', return_value=description), \
                 mock.patch.object(decoded_output, 'compare', return_value=('verified', 'exact')), \
                 mock.patch.object(decoded_output, 'invoke', return_value=(1, b'', b'no packet evidence')), \
                 mock.patch.object(run, 'packet_rows', side_effect=[[(0, 'one')], [(0, 'two')]]):
                self.assertEqual(run.compare_outputs(command, base, programs, {}, base), 'different')

    def test_packet_timing_uses_output_stream_and_rational_clocks(self):
        doc = {'streams': [{'index': 1, 'time_base': '1/10'}],
               'packets': [{'stream_index': 1, 'data_hash': 'SHA256:test', 'pts': 3, 'dts': 2, 'duration': 1,
                            'flags': 'K__'}]}
        with mock.patch.object(decoded_output, 'invoke', return_value=(0, json.dumps(doc).encode(), b'')):
            self.assertEqual(run.packet_rows('output', 'probe', {}, ()), [(1, 'SHA256:test', '3/10', '1/5', '1/10', True)])
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            (folder / 'command.stderr').write_text('Stream #0:7 -> #1:0 (copy)\nStream #0:1 -> #0:2 (copy)\n')
            self.assertEqual(run.jobs(folder), {(1, 0): 'copy', (0, 2): 'copy'})

    def test_exact_copy_rejects_each_keyframe_only_difference(self):
        command = inputs.instantiate(shape('ffmpeg'))
        document = {'streams': [{'index': i, 'time_base': '1/10', 'codec_name': codec}
                                for i, codec in enumerate(('h264', 'aac'))],
                    'packets': [{'stream_index': i, 'data_hash': 'SHA256:control', 'pts': pts,
                                 'dts': pts, 'duration': 1, 'flags': 'K__'}
                                for pts in (0, 1) for i in (0, 1)]}
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for arm in ('reference', 'candidate'):
                folder = base / arm
                folder.mkdir()
                (folder / 'output0.mp4').write_bytes(b'control')
                (folder / 'command.stderr').write_text(
                    'Stream #0:0 -> #0:0 (copy)\nStream #0:1 -> #0:1 (copy)\n')
            candidate = copy.deepcopy(document)
            def invoke(argv, env, **kwargs):
                source = Path(argv[argv.index('-i') + 1])
                return 0, json.dumps(candidate if source.parent.name == 'candidate' else document).encode(), b''
            with mock.patch.object(decoded_output, 'probe', return_value=document), \
                 mock.patch.object(decoded_output, 'compare', return_value=('verified', 'exact')), \
                 mock.patch.object(decoded_output, 'invoke', side_effect=invoke):
                def result():
                    return run.compare_outputs(command, base, {'$FFMPEG': 'ffmpeg', '$FFPROBE': 'ffprobe'}, {}, base)
                self.assertEqual(result(), 'counted')
                for index in range(len(candidate['packets'])):
                    for ours, theirs in (('___', 'K__'), ('K__', '___')):
                        with self.subTest(packet=index, ours=ours, theirs=theirs):
                            candidate['packets'][index]['flags'] = ours
                            document['packets'][index]['flags'] = theirs
                            self.assertEqual(result(), 'different')
                    candidate['packets'][index]['flags'] = document['packets'][index]['flags'] = 'K__'
                candidate['packets'][0]['flags'] = 'KD_'
                self.assertEqual(result(), 'counted')


class Avc1Carriage(unittest.TestCase):
    SPS, PPS = b'\x67\x42\x00\x0a', b'\x68\x12'
    SLICE, SEI = b'\x65\x01\x02', b'\x06\x03'

    @staticmethod
    def dump(data):
        return '\n' + '\n'.join(f'{at:08x}: ' + ' '.join(
            data[i:i + 2].hex() for i in range(at, min(at + 16, len(data)), 2)).ljust(39) + '  text'
            for at in range(0, len(data), 16)) + '\n'

    @staticmethod
    def sample(*units, length=4):
        return b''.join(len(unit).to_bytes(length, 'big') + unit for unit in units)

    def packet(self, data, index=0, pts=0):
        return {'stream_index': index, 'pts': pts, 'dts': pts, 'duration': 1, 'size': str(len(data)),
                'data': self.dump(data), 'data_hash': 'SHA256:' + hashlib.sha256(data).hexdigest(), 'flags': 'K__'}

    def configuration(self, data, document):
        stream = document['streams'][0]
        stream.update(extradata=self.dump(data), extradata_size=len(data),
                      extradata_hash='SHA256:' + hashlib.sha256(data).hexdigest())

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.folder = Path(self.tmp.name)
        video = {'index': 0, 'codec_type': 'video', 'codec_name': 'h264', 'codec_tag_string': 'avc1',
                 'width': 2, 'height': 2, 'pix_fmt': 'yuv420p', 'time_base': '1/10',
                 'start_pts': 0, 'duration_ts': 1}
        audio = {'index': 1, 'codec_type': 'audio', 'codec_name': 'aac', 'sample_fmt': 'dbl',
                 'sample_rate': '10', 'channels': 1, 'time_base': '1/10', 'start_pts': 0, 'duration_ts': 1}
        self.actual = {'streams': [video, audio], 'packets': [self.packet(self.sample(self.SEI, self.SLICE)),
                                                            self.packet(b'audio', 1)]}
        self.config = b'\x01\x42\x00\x0a\xff\xe1' + len(self.SPS).to_bytes(2, 'big') + self.SPS + (
            b'\x01' + len(self.PPS).to_bytes(2, 'big') + self.PPS)
        self.configuration(self.config, self.actual)
        self.reference = copy.deepcopy(self.actual)
        self.reference['packets'][0] = self.packet(self.sample(self.SEI, self.SPS, self.PPS, self.SLICE))
        self.frames = [{'stream_index': 0, 'width': 2, 'height': 2, 'pix_fmt': 'yuv420p',
                        'best_effort_timestamp': 0, 'duration': 1},
                       {'stream_index': 1, 'best_effort_timestamp': 0, 'duration': 1, 'nb_samples': 1}]
        self.audio_shift, self.changed_pixels, self.changed_audio = False, False, False

    def invoke(self, argv, env, output=None):
        candidate = argv[argv.index('-i') + 1] == 'candidate'
        document = copy.deepcopy(self.actual if candidate else self.reference)
        if '-show_packets' in argv:
            self.assertIn('-show_data', argv)
            return 0, json.dumps(document).encode(), b''
        if '-show_frames' in argv:
            document['frames'] = copy.deepcopy(self.frames)
            if candidate and self.audio_shift:
                document['frames'][1]['best_effort_timestamp'] += 1
            return 0, json.dumps(document).encode(), b''
        audio = '-c:a' in argv
        changed = candidate and (self.changed_audio if audio else self.changed_pixels)
        Path(output).write_bytes((b'\0' * 7 + bytes([changed])) if audio else b'pixel' + bytes([changed]))
        return 0, b'', b''

    def compatible(self, copied=(0, 1)):
        with mock.patch.object(decoded_output, 'invoke', side_effect=self.invoke):
            return decoded_output.avc1_carriage('candidate', 'reference', 'ffmpeg', 'ffprobe', {}, self.folder,
                                                copied, registered_codecs=run.EXACT_CODECS)

    def test_only_identical_configuration_parameter_sets_are_removed(self):
        self.assertTrue(self.compatible())
        for length in (1, 2, 4):
            config = self.config[:4] + bytes([0xFC | (length - 1)]) + self.config[5:]
            for document in (self.actual, self.reference):
                self.configuration(config, document)
            self.actual['packets'][0] = self.packet(self.sample(self.SEI, self.SLICE, length=length))
            self.reference['packets'][0] = self.packet(self.sample(self.SEI, self.SPS, self.PPS,
                                                                  self.SPS, self.SLICE, length=length))
            self.assertTrue(self.compatible())

    def test_parameter_set_changes_candidate_sets_and_other_nal_changes_fail(self):
        for units in ((self.SEI, self.SPS + b'changed', self.PPS, self.SLICE),
                      (self.SEI, self.SPS, self.PPS + b'changed', self.SLICE),
                      (self.SPS, self.PPS, self.SLICE),
                      (self.SEI, self.SPS, self.PPS, self.SLICE + b'changed'),
                      (self.SEI, self.SPS, self.PPS, self.SLICE, b'\x09\x10')):
            with self.subTest(units=units):
                self.reference['packets'][0] = self.packet(self.sample(*units))
                self.assertFalse(self.compatible())
        self.reference['packets'][0] = self.packet(self.sample(self.SPS, self.PPS, self.SEI, self.SLICE))
        for unit in (self.SPS, self.PPS):
            self.actual['packets'][0] = self.packet(self.sample(unit, self.SEI, self.SLICE))
            self.assertFalse(self.compatible())

    def test_avc3_and_differing_or_malformed_configuration_fail(self):
        for document in (self.actual, self.reference):
            document['streams'][0]['codec_tag_string'] = 'avc3'
            self.assertFalse(self.compatible())
            document['streams'][0]['codec_tag_string'] = 'avc1'
        for data in (self.config[:-1], self.config + b'junk', b'\x02' + self.config[1:],
                     self.config[:4] + b'\xfe' + self.config[5:],
                     self.config[:5] + b'\xe0' + self.config[6:],
                     self.config[:4] + b'\x03' + self.config[5:]):
            with self.subTest(config=data):
                for document in (self.actual, self.reference):
                    self.configuration(data, document)
                self.assertFalse(self.compatible())
        self.configuration(self.config, self.actual)
        self.configuration(self.config.replace(self.SPS, self.SPS[:-1] + b'\x0b'), self.reference)
        self.assertFalse(self.compatible())

    def test_malformed_packet_lengths_or_hashes_fail(self):
        for document in (self.actual, self.reference):
            original = document['packets'][0]
            for data in (b'', b'\0', b'\0\0\0\0', b'\0\0\0\x08\x65', self.sample(self.SLICE) + b'\0'):
                document['packets'][0] = self.packet(data)
                self.assertFalse(self.compatible())
            document['packets'][0] = copy.deepcopy(original)
            document['packets'][0]['data_hash'] = 'SHA256:' + '0' * 64
            self.assertFalse(self.compatible())
            document['packets'][0] = copy.deepcopy(original)
            document['packets'][0]['size'] = '1'
            self.assertFalse(self.compatible())
            document['packets'][0] = original

    def test_timing_packet_order_counts_and_copied_audio_bytes_stay_exact(self):
        original = copy.deepcopy(self.actual['packets'])
        for key in ('pts', 'dts', 'duration'):
            self.actual['packets'][0][key] += 1
            self.assertFalse(self.compatible())
            self.actual['packets'] = copy.deepcopy(original)
        for packets in (list(reversed(original)), original + original[:1], original[:1],
                        [original[0], self.packet(b'changed audio', 1)]):
            self.actual['packets'] = packets
            self.assertFalse(self.compatible())
        self.actual['packets'] = original
        self.actual['streams'][0]['time_base'] = '1/20'
        self.actual['packets'][0]['duration'] = 2
        self.actual['streams'][0]['duration_ts'] = 2
        self.assertFalse(self.compatible())

    def test_compatible_copy_rejects_each_keyframe_only_difference(self):
        for document in (self.actual, self.reference):
            document['packets'].extend([self.packet(self.sample(self.SLICE), pts=1), self.packet(b'audio', 1, pts=1)])
        self.assertTrue(self.compatible())
        for index in range(len(self.actual['packets'])):
            for ours, theirs in (('___', 'K__'), ('K__', '___')):
                with self.subTest(packet=index, ours=ours, theirs=theirs):
                    self.actual['packets'][index]['flags'] = ours
                    self.reference['packets'][index]['flags'] = theirs
                    self.assertFalse(self.compatible())
            self.actual['packets'][index]['flags'] = self.reference['packets'][index]['flags'] = 'K__'
        self.actual['packets'][0]['flags'] = 'KD_'
        self.assertTrue(self.compatible())

    def test_decoded_pixels_samples_and_all_audio_frame_timelines_stay_exact(self):
        for attribute in ('audio_shift', 'changed_pixels', 'changed_audio'):
            setattr(self, attribute, True)
            for copied in ((0,), (0, 1)):
                self.assertFalse(self.compatible(copied))
            setattr(self, attribute, False)
        self.assertTrue(self.compatible((0,)))

    def test_compatible_requires_known_positive_video_frame_timing_without_changing_exact_defaults(self):
        for key in ('best_effort_timestamp', 'duration'):
            original = self.frames[0][key]
            for unavailable in (None, 'N/A'):
                self.frames[0][key] = unavailable
                self.assertFalse(self.compatible())
                with mock.patch.object(decoded_output, 'invoke', side_effect=self.invoke):
                    self.assertEqual(decoded_output.compare('candidate', 'reference', 'ffmpeg', 'ffprobe', {},
                        self.folder, registered_codecs=run.EXACT_CODECS)[0], 'verified')
            self.frames[0][key] = original
        for duration in (0, -1):
            self.frames[0]['duration'] = duration
            self.assertFalse(self.compatible())

    def test_no_removals_noncopy_and_malformed_evidence_fail(self):
        self.assertFalse(self.compatible((1,)))
        original = self.reference['packets'][0]
        self.reference['packets'][0] = self.actual['packets'][0]
        self.assertFalse(self.compatible())
        self.reference['packets'][0] = original
        for document in (None, {}, {'streams': None}, {'streams': [], 'packets': []}):
            with mock.patch.object(decoded_output, 'packet_data', return_value=document):
                self.assertFalse(decoded_output.avc1_carriage('a', 'b', 'ffmpeg', 'ffprobe', {}, self.folder, (0,)))
        for text in ('00000001: 1234', '00000000: zz', '00000000: 12\n00000010: 34'):
            with self.assertRaises(ValueError):
                decoded_output.hexdump(text)

    def test_extractor_failures_and_configuration_hash_mismatch_fail(self):
        for code, data, err in ((1, b'{}', b''), (0, b'{}', b'error'), (0, b'not json', b''),
                                (0, b'[]', b''), (0, b'{"streams":{},"packets":[]}', b'')):
            with mock.patch.object(decoded_output, 'invoke', return_value=(code, data, err)):
                self.assertIsNone(decoded_output.packet_data('file', 'probe', {}))
        self.actual['streams'][0]['extradata_hash'] = 'SHA256:' + '0' * 64
        self.assertFalse(self.compatible())

    def test_high_profile_extensions_parse_completely_and_never_leave_parameter_sets_in_samples(self):
        config = self.config[:1] + b'\x64' + self.config[2:]
        extension = b'\x6d\x01'
        extended = config + b'\xfd\xf8\xf8\x01' + len(extension).to_bytes(2, 'big') + extension
        for document in (self.actual, self.reference):
            self.configuration(extended, document)
        self.assertTrue(self.compatible())
        for invalid in (extended[:-1], extended + b'junk', config + b'\x01\xf8\xf8\0'):
            for document in (self.actual, self.reference):
                self.configuration(invalid, document)
            self.assertFalse(self.compatible())
        for document in (self.actual, self.reference):
            self.configuration(self.config, document)
        for unit in (extension, b'\x6f\x01'):
            self.actual['packets'][0] = self.packet(self.sample(unit, self.SEI, self.SLICE))
            self.reference['packets'][0] = self.packet(self.sample(unit, self.SPS, self.PPS, self.SEI, self.SLICE))
            self.assertFalse(self.compatible())


class Avc1Outcomes(unittest.TestCase):
    def check(self, packet_pairs, compatible=True, verdicts=None, copied=True):
        command = inputs.instantiate(shape('ffmpeg'))
        command = inputs.Command(command.tool, command.args, command.sources,
                                 tuple(f'output{i}.mp4' for i in range(len(packet_pairs))),
                                 tuple(() for _ in packet_pairs))
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for arm in ('reference', 'candidate'):
                folder = base / arm
                folder.mkdir()
                for name in command.outputs:
                    (folder / name).write_bytes(b'generated control')
                (folder / 'command.stderr').write_text(''.join(
                    f'Stream #0:0 -> #{i}:0 ({"copy" if copied else "h264 (libx264)"})\n'
                    for i in range(len(packet_pairs))))
            description = {'streams': [{'codec_name': 'h264'}]}
            with mock.patch.object(decoded_output, 'probe', return_value=description), \
                 mock.patch.object(decoded_output, 'compare', side_effect=verdicts or [('verified', 'exact')] * (2 * len(packet_pairs))) as compare, \
                 mock.patch.object(run, 'packet_rows', side_effect=[p for pair in packet_pairs for p in pair]), \
                 mock.patch.object(decoded_output, 'avc1_carriage', return_value=compatible) as predicate:
                result = run.compare_outputs(command, base, {'$FFMPEG': 'ffmpeg', '$FFPROBE': 'ffprobe'}, {}, base)
                for call in compare.call_args_list[len(packet_pairs):]:
                    self.assertTrue(call.kwargs['exact_frame_timeline'])
                return result, predicate.call_count

    def test_strict_first_and_only_compatible_packet_mismatch_changes_class(self):
        equal = ([(0, 'same')], [(0, 'same')])
        different = ([(0, 'ours')], [(0, 'theirs')])
        self.assertEqual(self.check([equal]), ('counted', 0))
        self.assertEqual(self.check([different]), (run.COMPATIBLE, 1))
        self.assertEqual(self.check([different], compatible=False), ('different', 1))
        self.assertEqual(self.check([different], copied=False), ('counted', 0))
        self.assertEqual(self.check([different], verdicts=[('failed', 'pixels')]), ('different', 0))
        self.assertEqual(self.check([different], verdicts=[('unverified', 'probe')]), ('no_oracle', 0))

    def test_later_failure_overrides_compatible_without_changing_original_miss_class(self):
        different = ([(0, 'ours')], [(0, 'theirs')])
        equal = ([(0, 'same')], [(0, 'same')])
        self.assertEqual(self.check([different, equal]), (run.COMPATIBLE, 1))
        self.assertEqual(self.check([equal, different]), (run.COMPATIBLE, 1))
        for verdict in ('failed', 'unverified'):
            self.assertEqual(self.check([different, equal], verdicts=[('verified', 'exact'), (verdict, 'later')]),
                             ('different', 1))
        self.assertEqual(self.check([different, equal], verdicts=[('unverified', 'first')]), ('no_oracle', 0))

    def test_all_outputs_need_exact_audio_timeline_when_any_is_compatible(self):
        different = ([(0, 'ours')], [(0, 'theirs')])
        equal = ([(0, 'same')], [(0, 'same')])
        for pairs in ([different, equal], [equal, different]):
            self.assertEqual(self.check(pairs, verdicts=[('verified', 'exact'), ('verified', 'exact'),
                                                        ('failed', 'audio timeline')]), ('different', 1))


class ProbeAvailability(unittest.TestCase):
    def capture(self, options, reference, candidate, witness=None):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bundle = root / 'bundle'
            (bundle / 'fixtures').mkdir(parents=True)
            (bundle / 'fixtures/source.mp4').write_bytes(b'synthetic probe control')
            fake = root / 'fake'
            if witness is None:
                witness = reference
            fake.write_text('#!' + sys.executable + '\nimport sys\n' +
                            f'sys.stdout.write({candidate!r} if sys.argv[1] == "ffprobe" else '
                            f'{witness!r} if "json" in sys.argv else {reference!r})\n')
            fake.chmod(0o755)
            programs = {key: str(fake) for key in ('$COREAV', '$FFMPEG', '$FFPROBE', '$X264')}
            entry = row()
            entry['shape'] = shape(options=options)
            record = run.acquire_one(entry, bundle, programs, run.environment())
            return run.judge(entry, record, bundle, programs, run.environment(), root)

    def test_empty_json_containers_and_missing_sections_never_count(self):
        options = [['-of', 'json'], ['-show_streams', None]]
        for payload in ('', ' \n', '{}', '{"streams":[]}', '{"streams":[{}]}',
                        '{"format":{"duration":"2"}}'):
            with self.subTest(payload=payload):
                self.assertEqual(self.capture(options, payload, payload), 'no_oracle')

    def test_unavailable_requested_scalar_and_omitted_field_never_count(self):
        cases = [([['-of', 'json'], ['-show_entries', 'format=control_sentinel']],
                  '{"format":{"control_sentinel":"N/A"}}'),
                 ([['-of', 'json'], ['-show_entries', 'format=control_sentinel']],
                  '{"format":{"filename":"input"}}'),
                 ([['-of', 'default=nk=<value>:nw=<value>'], ['-show_entries', 'format=control_sentinel']], 'N/A\n')]
        for options, payload in cases:
            with self.subTest(payload=payload):
                self.assertEqual(self.capture(options, payload, payload), 'no_oracle')

    def test_empty_or_unavailable_output_on_either_side_is_a_miss(self):
        options = [['-of', 'json'], ['-show_entries', 'stream=index']]
        valid = '{"streams":[{"index":0}]}'
        for unavailable in ('', '{}', '{"streams":[]}', '{"streams":[{"index":"N/A"}]}'):
            for reference, candidate in ((unavailable, valid), (valid, unavailable)):
                with self.subTest(reference=reference, candidate=candidate):
                    self.assertNotEqual(self.capture(options, reference, candidate), 'counted')

    def test_populated_response_counts_only_when_exact(self):
        options = [['-of', 'json'], ['-show_streams', None]]
        valid = '{"streams":[{"index":0,"codec_name":"control"}]}'
        self.assertEqual(self.capture(options, valid, valid), 'counted')
        self.assertEqual(self.capture(options, valid, valid.replace('control', 'changed')), 'different')

    def test_availability_uses_one_json_witness_for_every_writer(self):
        cases = [('csv=p=<value>', 'stream,7\n', 'stream,\n'),
                 ('default=nk=<value>:nw=<value>', '7\n', 'N/A\n'),
                 ('compact', 'stream|index=7\n', 'stream\n'),
                 ('flat=s=<value>', 'streams,stream,0,index=7\n', 'streams,stream,0,index=""\n')]
        for writer, valid, empty in cases:
            command = inputs.instantiate(shape(options=[['-of', writer], ['-show_entries', 'stream=index']]))
            with self.subTest(writer=writer):
                self.assertEqual(self.capture([['-of', writer], ['-show_entries', 'stream=index']], valid, valid,
                                              '{"streams":[{"index":7}]}'), 'counted')
                self.assertEqual(self.capture([['-of', writer], ['-show_entries', 'stream=index']], empty, empty,
                                              '{"streams":[{"index":"N/A"}]}'), 'no_oracle')
                self.assertTrue(run.probe_available(command, b'{"streams":[{"index":7}]}'))

    def test_flat_separator_is_part_of_the_registered_writer(self):
        options = [['-of', 'flat=s=<value>'], ['-show_entries', 'stream=index']]
        witness = '{"streams":[{"index":7}]}'
        self.assertEqual(self.capture(options, 'streams,stream,0,index=7\n', 'streams,stream,0,index=7\n', witness), 'counted')
        self.assertEqual(self.capture(options, 'streams,stream,0,index=7\n', 'streams,stream,0,index=8\n', witness), 'different')
        self.assertEqual(self.capture(options, 'streams,stream,0,index=7\n', 'streams.stream.0.index=7\n', witness), 'different')
        command = inputs.instantiate(shape(options=options))
        self.assertIn('flat=s=,', command.args)
        self.assertIn('flat=s=,', run.expected_invocation(command, False))
        reference = run.expected_invocation(command, False)
        reference[reference.index('-of') + 1] = 'json'
        self.assertEqual(run.probe_witness_invocation(command), reference)

    def test_broad_witness_allows_missing_optional_values_but_not_empty_records(self):
        command = inputs.instantiate(shape(options=[['-show_streams', None]]))
        self.assertTrue(run.probe_available(command, b'{"streams":[{"index":0,"duration":"N/A"}]}'))
        for witness in (b'{"streams":[{"duration":"N/A"}]}', b'{"streams":[{"duration":null}]}',
                        b'{"streams":[{"index":0},{}]}'):
            self.assertFalse(run.probe_available(command, witness))

    def test_each_requested_json_section_needs_substantive_data(self):
        options = [['-of', 'json'], ['-show_format', None], ['-show_streams', None]]
        payload = '{"format":{"duration":"2"},"streams":[]}'
        self.assertEqual(self.capture(options, payload, payload), 'no_oracle')

    def test_every_instantiated_probe_writer_has_a_real_oracle_control(self):
        writers = set()
        for entry in roster.load():
            if entry['shape']['tool'] != 'ffprobe':
                continue
            try:
                command = inputs.instantiate(entry['shape'])
            except inputs.Uninstantiable:
                continue
            writers.add(command.args[command.args.index('-of') + 1] if '-of' in command.args else 'default')
        self.assertLessEqual(writers, set(PinnedProbeWriters.WRITERS))


class LiveOracleSelection(unittest.TestCase):
    def test_writer_controls_retain_the_complete_owner_pair(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            selected = root / 'selected'
            pair = test_oracle.Oracle(root / 'verified/ffmpeg', root / 'verified/ffprobe')
            with mock.patch.dict(os.environ, COREAV_SHAPES_ORACLE=str(selected)), \
                    mock.patch.multiple(PinnedProbeWriters, oracle=None, temp=None, folder=None, control=None, create=True), \
                    mock.patch.object(test_oracle, 'require', return_value=pair) as qualify, \
                    mock.patch.object(run.oracle_pin, 'verify_bin', return_value=[]) as legacy:
                try:
                    PinnedProbeWriters.setUpClass()
                    qualify.assert_called_once_with(selected / 'ffmpeg', selected / 'ffprobe')
                    self.assertIs(PinnedProbeWriters.oracle, pair)
                    legacy.assert_not_called()
                    with mock.patch.object(run.subprocess, 'run', return_value=types.SimpleNamespace(stdout=b'{}', stderr=b'')) as execute:
                        PinnedProbeWriters().capture('json')
                    self.assertEqual(len(execute.call_args_list), 2)
                    self.assertTrue(all(call.args[0][0] == str(pair.ffprobe) for call in execute.call_args_list))
                finally:
                    if PinnedProbeWriters.temp is not None:
                        PinnedProbeWriters.tearDownClass()


@unittest.skipUnless(os.environ.get('COREAV_SHAPES_ORACLE'), 'pinned writer controls run in dedicated CI')
class PinnedProbeWriters(unittest.TestCase):
    WRITERS = ('default', 'json', 'csv', 'csv=p=0', 'csv=p=1', 'csv=p=1:s=,', 'flat', 'flat=s=,', 'ini', 'compact',
               'default=nk=1', 'default=nw=1', 'default=nk=1:nw=1',
               'default=nokey=1', 'default=noprint_wrappers=1', 'default=nokey=1:noprint_wrappers=1')

    @classmethod
    def setUpClass(cls):
        selected = Path(os.environ['COREAV_SHAPES_ORACLE']).resolve()
        cls.oracle = test_oracle.require(selected / 'ffmpeg', selected / 'ffprobe')
        cls.temp = tempfile.TemporaryDirectory()
        cls.folder = Path(cls.temp.name)
        cls.control = cls.folder / 'independent-writer-control.wav'
        with wave.open(str(cls.control), 'wb') as output:
            output.setparams((1, 2, 8000, 0, 'NONE', 'not compressed'))
            output.writeframes(bytes(range(32)))

    @classmethod
    def tearDownClass(cls):
        cls.temp.cleanup()

    def capture(self, writer, selection=None, entries='stream=index,sample_rate,channels', show_format=False):
        args = ['-show_entries', entries] if entries is not None else ['-show_streams']
        args.extend(['-of', writer])
        if show_format:
            args.append('-show_format')
        if selection:
            args.extend(['-select_streams', selection])
        args.extend(['-i', str(self.control)])
        result = subprocess.run([str(self.oracle.ffprobe), '-v', 'error', *run.AUDIO_DECODE_INPUT, *args],
                                env=run.environment(), capture_output=True, timeout=10, check=True)
        self.assertEqual(result.stderr, b'')
        command = inputs.Command('ffprobe', tuple(args), (), (), ())
        witness = run.probe_witness_invocation(command)
        witness[0] = str(self.oracle.ffprobe)
        witness = subprocess.run(witness, env=run.environment(), capture_output=True, timeout=10, check=True)
        self.assertEqual(witness.stderr, b'')
        return command, result.stdout, witness.stdout

    def test_all_writer_variants_parse_real_populated_and_empty_oracle_outputs(self):
        for writer in self.WRITERS:
            with self.subTest(writer=writer):
                command, populated, witness = self.capture(writer)
                self.assertIn(b'8000', populated)
                self.assertTrue(run.probe_available(command, witness), witness)
                empty_command, _, empty_witness = self.capture(writer, selection='s')
                self.assertFalse(run.probe_available(empty_command, empty_witness), empty_witness)
                unavailable_command, _, unavailable_witness = self.capture(writer, entries='stream=nb_frames')
                self.assertFalse(run.probe_available(unavailable_command, unavailable_witness), unavailable_witness)

    def test_real_oracle_output_has_exact_different_and_either_side_miss_outcomes(self):
        for writer in self.WRITERS:
            with self.subTest(writer=writer), tempfile.TemporaryDirectory() as tmp:
                command, populated, witness = self.capture(writer)
                _, empty, empty_witness = self.capture(writer, selection='s')
                folder = Path(tmp)
                reference, candidate = folder / 'reference', folder / 'candidate'
                reference.mkdir()
                candidate.mkdir()
                (reference / 'eligibility.stdout').write_bytes(witness)
                (reference / 'stdout').write_bytes(populated)
                (candidate / 'stdout').write_bytes(populated)
                self.assertEqual(run.compare_outputs(command, folder, {}, {}, folder), 'counted')
                (candidate / 'stdout').write_bytes(populated.replace(b'8000', b'8001'))
                self.assertEqual(run.compare_outputs(command, folder, {}, {}, folder), 'different')
                for left, right, available in ((empty, populated, empty_witness), (populated, empty, witness)):
                    (reference / 'stdout').write_bytes(left)
                    (candidate / 'stdout').write_bytes(right)
                    (reference / 'eligibility.stdout').write_bytes(available)
                    self.assertNotEqual(run.compare_outputs(command, folder, {}, {}, folder), 'counted')

    def test_broad_sections_stay_substantive_with_unavailable_optional_fields(self):
        for writer in self.WRITERS:
            for show_format in (False, True):
                with self.subTest(writer=writer, show_format=show_format), tempfile.TemporaryDirectory() as tmp:
                    command, populated, witness = self.capture(writer, entries=None, show_format=show_format)
                    self.assertTrue(run.probe_available(command, witness), witness)
                    if writer in ('default', 'flat', 'flat=s=,'):
                        self.assertIn(b'N/A', populated)
                    folder = Path(tmp)
                    for arm in ('reference', 'candidate'):
                        (folder / arm).mkdir()
                        (folder / arm / 'stdout').write_bytes(populated)
                    (folder / 'reference/eligibility.stdout').write_bytes(witness)
                    self.assertEqual(run.compare_outputs(command, folder, {}, {}, folder), 'counted')

    def test_independent_avc1_carriage_control_uses_complete_real_packet_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            ffmpeg, ffprobe = str(self.oracle.ffmpeg), str(self.oracle.ffprobe)
            calls = [
                ['-f', 'lavfi', '-i', 'testsrc2=size=32x32:rate=4', '-t', '1', '-an',
                 '-c:v', 'libx264', '-profile:v', 'baseline', '-bf', '0', '-g', '2', 'independent.mp4'],
                ['-f', 'concat', '-safe', '0', '-i', 'independent.txt', '-c', 'copy', 'reference.mp4'],
                ['-i', 'reference.mp4', '-c', 'copy', '-bsf:v', 'filter_units=remove_types=7|8', 'candidate.mp4'],
            ]
            (folder / 'independent.txt').write_text("file 'independent.mp4'\n")
            for args in calls:
                subprocess.run([ffmpeg, '-nostdin', '-v', 'error', *args], cwd=folder,
                               env=run.environment(), capture_output=True, timeout=20, check=True)
            actual, reference = folder / 'candidate.mp4', folder / 'reference.mp4'
            args = (actual, reference, ffmpeg, ffprobe, run.environment(), folder)
            self.assertEqual(decoded_output.compare(*args, registered_codecs=run.EXACT_CODECS)[0], 'verified')
            self.assertNotEqual(run.packet_rows(actual, ffprobe, run.environment(), ()),
                                run.packet_rows(reference, ffprobe, run.environment(), ()))
            self.assertTrue(decoded_output.avc1_carriage(*args, (0,), registered_codecs=run.EXACT_CODECS))
            self.assertFalse(decoded_output.avc1_carriage(actual, actual, *args[2:], (0,),
                                                         registered_codecs=run.EXACT_CODECS))
            base = folder / 'comparison'
            for arm, source in (('candidate', actual), ('reference', reference)):
                target = base / arm
                target.mkdir(parents=True)
                (target / 'output0.mp4').write_bytes(source.read_bytes())
                (target / 'command.stderr').write_text('Stream #0:0 -> #0:0 (copy)\n')
            command = inputs.instantiate(shape('ffmpeg'))
            self.assertEqual(run.compare_outputs(command, base, {'$FFMPEG': ffmpeg, '$FFPROBE': ffprobe},
                                                 run.environment(), folder), run.COMPATIBLE)


class AccountingAndGate(unittest.TestCase):
    def test_compatible_counts_are_separate_from_exact_shares(self):
        rows = [row(i, 1271 if i == 2 else 1) for i in range(1, 201)]
        outcomes = [{'rank': r['rank'], 'outcome': 'counted' if r['rank'] == 1 else 'different'} for r in rows]
        original = run.summarize(rows, outcomes)
        outcomes[1]['outcome'] = run.COMPATIBLE
        summary = run.summarize(rows, outcomes)
        for key in ('counted', 'counted_pairs', 'top_200_pair_share', 'whole_sample_presence_lower_bound'):
            self.assertEqual(summary[key], original[key])
        self.assertEqual((summary[run.COMPATIBLE], summary[run.COMPATIBLE + '_pairs']), (1, 1271))
        self.assertEqual(summary['outcomes'][run.COMPATIBLE], 1)
        self.assertEqual(summary['tools']['ffprobe']['pairs'][run.COMPATIBLE], 1271)
        self.assertEqual(sum(summary['outcome_pairs'].values()), 1470)
        self.assertEqual(sum(summary['outcomes'].values()), 200)
        outcomes[1]['outcome'] = 'unknown'
        with self.assertRaisesRegex(ValueError, 'result_class'):
            run.summarize(rows, outcomes)

    def test_all_200_stay_and_weighted_share_has_fixed_denominator(self):
        rows = [row(i, 1271 if i == 1 else 1) for i in range(1, 201)]
        outcomes = [{'rank': r['rank'], 'outcome': 'counted' if r['rank'] == 1 else 'no_oracle'} for r in rows]
        summary = run.summarize(rows, outcomes)
        self.assertEqual((summary['counted'], summary['counted_pairs']), (1, 1271))
        self.assertEqual(summary['top_200_pair_share'], 1271 / 1470)
        self.assertIsNone(summary['bar'])
        for bad in (outcomes[:-1], outcomes + outcomes[:1], list(reversed(outcomes))):
            with self.assertRaises(ValueError):
                run.summarize(rows, bad)

    def test_no_generation_before_main_membership_and_clean_source(self):
        with mock.patch.dict(os.environ, {'GITHUB_EVENT_NAME': 'pull_request', 'GITHUB_REF': 'refs/heads/main'}), \
             self.assertRaisesRegex(ValueError, 'main_dispatch_required'):
            run.merged_source('a' * 40)
        with mock.patch.dict(os.environ, {'GITHUB_EVENT_NAME': 'workflow_dispatch', 'GITHUB_REF': 'refs/heads/main'}):
            good = subprocess.CompletedProcess([], 0, 'a' * 40 + '\n', '')
            clean = subprocess.CompletedProcess([], 0, '', '')
            missing = subprocess.CompletedProcess([], 1, '', '')
            with mock.patch.object(run, 'git', side_effect=[good, clean, missing]), \
                 self.assertRaisesRegex(ValueError, 'candidate_not_merged'):
                run.merged_source('a' * 40)
            with mock.patch.object(run, 'git', side_effect=[good, clean, clean]):
                run.merged_source('a' * 40)

    def test_finite_process_deadline_and_binary_stdout(self):
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp)
            (folder / 'input').write_bytes(b'\x00\xff')
            receipt = run.execute([sys.executable, '-c', 'import sys;sys.stdout.buffer.write(sys.stdin.buffer.read())'],
                                  folder, run.environment(), 10, 'control', 'input')
            self.assertEqual(receipt['code'], 0)
            self.assertEqual((folder / 'control.stdout').read_bytes(), b'\x00\xff')
            receipt = run.execute([sys.executable, '-c', 'import time;time.sleep(10)'],
                                  folder, run.environment(), 0.01, 'timeout')
            self.assertTrue(receipt['timed_out'])
            self.assertNotEqual(receipt['code'], 0)

    def test_acquisition_is_manual_only_and_pr_controls_use_the_pinned_oracle(self):
        text = (run.ROOT / '.github/workflows/github-shapes.yml').read_text()
        self.assertIn('workflow_dispatch:', text)
        self.assertIn("if: github.event_name == 'workflow_dispatch'", text.split('  measure:', 1)[1])
        self.assertIn('COREAV_SHAPES_ORACLE="$RUNNER_TEMP/writer-oracle"', text)
        self.assertIn('scripts/install-oracle-ffmpeg.sh "$RUNNER_TEMP/writer-oracle"', text)
        self.assertIn(' '.join(run.BUILD), text)
        self.assertLess(text.index('candidate_not_merged'), text.index('cargo build'))

    def test_measure_runs_the_candidate_only_on_the_dispatching_owner_pair(self):
        measure = (run.ROOT / '.github/workflows/github-shapes.yml').read_text().split('  measure:', 1)[1].split('\n  aac-controls:', 1)[0]
        copy = measure.index('"$RUNNER_TEMP/oracle-owner/scripts/"')
        self.assertLess(copy, measure.index('git checkout --detach "$COMMIT"'))
        self.assertIn('cp scripts/test_oracle.py scripts/oracle_pin.py scripts/oracle_pin_macos.py scripts/oracle-ffmpeg.env \\\n'
                      '            scripts/oracle-ffmpeg-macos.env "$RUNNER_TEMP/oracle-owner/scripts/"', measure)
        resolve = measure.index('"$RUNNER_TEMP/oracle-owner/scripts/test_oracle.py" --resolve')
        driver = measure.index('github_shapes.py run')
        self.assertLess(resolve, driver)
        command = measure[driver:measure.index('--summary', driver)]
        self.assertIn('--ffmpeg "$FFMPEG"', command)
        self.assertIn('--ffprobe "$FFPROBE"', command)
        self.assertNotIn('oracle/ffmpeg', command)
        self.assertNotIn('oracle/ffprobe', command)


if __name__ == '__main__':
    unittest.main()
