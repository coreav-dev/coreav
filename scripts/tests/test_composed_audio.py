import dataclasses
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from types import SimpleNamespace
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import composed_audio as composed
import composed_audio_fixtures as fixtures
import transcode_check as checker
import composed_audio_source as source_model


class ComparatorDownload(unittest.TestCase):
    def test_a_redirect_downloads_the_registered_archive_not_its_response_body(self):
        archive = b'registered archive'
        pin, identity = composed.registration()
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            stored = root / 'expected'
            stored.write_bytes(archive)
            pin['opus_compare']['sha256'] = composed.sha256_file(stored)

            def command(argv, env, **kwargs):
                if argv[0] == 'curl':
                    payload = archive if '--location' in argv else b'redirect response body'
                    Path(argv[argv.index('-o') + 1]).write_bytes(payload)
                elif argv[0] == 'tar':
                    license_path = Path(argv[argv.index('-C') + 1]) / argv[-1]
                    license_path.parent.mkdir(parents=True)
                    license_path.write_text('Fixture license')
                elif argv[0] == 'cc':
                    Path(argv[argv.index('-o') + 1]).write_bytes(b'comparator')
                else:
                    self.fail('unexpected program')

            with mock.patch.object(composed, 'registration', return_value=(pin, identity)), \
                    mock.patch.object(composed, 'bounded', side_effect=command):
                digest = fixtures.build_comparator(root / 'tools', {})
            metadata = json.loads((root / 'tools/comparator.json').read_text())
            self.assertEqual(metadata['archive_sha256'], pin['opus_compare']['sha256'])
            self.assertEqual(digest, composed.sha256_file(root / 'tools/opus_compare'))


def box(kind, payload):
    return struct.pack('>I4s', 8 + len(payload), kind) + payload


def edits_blob(entries, version=0):
    fmt = '>Iihh' if version == 0 else '>Qqhh'
    return bytes([version, 0, 0, 0]) + struct.pack('>I', len(entries)) + b''.join(
        struct.pack(fmt, *entry) for entry in entries)


def movie(entries, track_id=7, elst=None, extra_tables=b'', duplicate_track=False, opus=False):
    mvhd, tkhd, mdhd, hdlr, audio = (bytearray(n) for n in (100, 84, 24, 24, 28))
    struct.pack_into('>I', mvhd, 12, 1000)
    struct.pack_into('>I', tkhd, 12, track_id)
    struct.pack_into('>I', mdhd, 12, 48000)
    hdlr[8:12] = b'soun'
    struct.pack_into('>H', audio, 16, 1)
    struct.pack_into('>I', audio, 24, 48000 << 16)
    configuration = (box(b'dOps', struct.pack('>BBHIhB', 0, 1, 312, 48000, 0, 0)) if opus else
                     box(b'esds', b'\0' * 4))
    stsd = box(b'stsd', b'\0' * 7 + b'\1' + box(b'Opus' if opus else b'mp4a', audio + configuration))
    tables = b''.join(box(kind, b'\0' * 8) for kind in (b'stts', b'stsc', b'stsz', b'stco'))
    media = box(b'mdhd', mdhd) + box(b'hdlr', hdlr) + box(b'minf', box(b'stbl', stsd + tables + extra_tables))
    track = box(b'trak', box(b'tkhd', tkhd) + box(b'mdia', media) +
                box(b'edts', box(b'elst', edits_blob(entries) if elst is None else elst)))
    return box(b'mdat', b'\0' * 8192) + box(b'moov', box(b'mvhd', mvhd) + track * (2 if duplicate_track else 1))


def timing(start=0, final=1024):
    entries = ((100, 1024, 1, 0),)
    if start:
        entries = ((250, -1, 1, 0),) + entries
    edits = composed.Edits(7, 1000, 48000, entries, ((8, 8200),))
    packets = [{'pts': j * 1024, 'dts': j * 1024, 'duration': final if j == 5 else 1024,
                'pos': 8 + j * 16, 'size': 16} for j in range(6)]
    return edits, packets


class Presentation(unittest.TestCase):
    def test_full_or_short_terminal_samples_have_the_same_effective_map(self):
        for start in (0,):
            with self.subTest(start=start):
                source = {'frames': 4800, 'rate': 48000, 'channels': 1, 'start_ticks': start}
                result = composed.same_presentation(timing(start), source, timing(final=704),
                    {'samples': 4800, 'sample_rate': 48000, 'channels': 1, 'start_ticks': start})
                self.assertTrue(result['ok'])
                self.assertEqual(result['start_ticks'], start)
                self.assertEqual(result['packets_checked'], 6)

    def test_interior_durations_cannot_hide_behind_unchanged_endpoints(self):
        for start in (0, 12000):
            edits, packets = timing(start)
            packets[1]['duration'] += 1
            packets[2]['duration'] -= 1
            packets[2]['dts'] += 1
            packets[2]['pts'] += 1
            self.assertEqual(sum(p['duration'] for p in packets), 6144)
            self.assertEqual(packets[0]['pts'], 0)
            self.assertEqual(packets[-1]['pts'], 5120)
            with self.assertRaises(composed.Invalid):
                composed.sample_map(edits, packets, 4800)

    def test_start_binding_and_reference_priming_are_independent(self):
        for candidate, origin, reference, start in (
                (timing(), 12000, timing(), 0),
                (timing(12000), 12000, timing(12000), 12000),
                (timing(), 0, timing(), 1)):
            with self.assertRaises(composed.Invalid):
                composed.same_presentation(candidate,
                    {'frames': 4800, 'rate': 48000, 'channels': 1, 'start_ticks': origin}, reference,
                    {'samples': 4800, 'sample_rate': 48000, 'channels': 1, 'start_ticks': start})
        candidate = timing()
        reference = timing()
        reference = (dataclasses.replace(reference[0], entries=((100, 1025, 1, 0),)), reference[1])
        with self.assertRaises(composed.Invalid):
            composed.same_presentation(candidate,
                {'frames': 4800, 'rate': 48000, 'channels': 1, 'start_ticks': 0}, reference,
                {'samples': 4800, 'sample_rate': 48000, 'channels': 1, 'start_ticks': 0})

    def test_source_count_rate_channels_and_origin_cannot_be_chosen_by_the_capture(self):
        source = {'frames': 4800, 'rate': 48000, 'channels': 1, 'start_ticks': 0}
        for key, wrong in [('frames', 4799), ('rate', 44100), ('channels', 2), ('start_ticks', 1)]:
            with self.subTest(key=key), self.assertRaises(composed.Invalid):
                composed.same_presentation(timing(), dict(source, **{key: wrong}), timing(),
                    {'samples': 4800, 'sample_rate': 48000, 'channels': 1, 'start_ticks': 0})

    def test_normal_aac_baseline_does_not_own_duration_or_priming(self):
        properties = {'codec_name': 'aac', 'profile': 'LC', 'sample_fmt': 'fltp',
                      'sample_rate': '48000', 'channels': 2, 'channel_layout': 'stereo',
                      'time_base': '1/48000', 'start_pts': 0}
        calls = []

        def probe(command, env):
            calls.append(command)
            value = dict(properties, duration_ts=96000 if command[-1] == 'candidate' else 96648)
            return json.dumps({'streams': [value]}).encode(), b''

        with mock.patch.object(composed, 'bounded', side_effect=probe):
            composed.baseline_properties(SimpleNamespace(ffprobe='ffprobe'), 'candidate', 1, 'baseline', 1, {})
        self.assertEqual(len(calls), 2)
        self.assertTrue(all('duration_ts' not in command[command.index('-show_entries') + 1] for command in calls))
        for key, value in [('channels', 1), ('start_pts', 1), ('profile', 'HE-AAC')]:
            with mock.patch.object(composed, 'bounded', side_effect=[
                    (json.dumps({'streams': [properties]}).encode(), b''),
                    (json.dumps({'streams': [dict(properties, **{key: value})]}).encode(), b'')]):
                with self.assertRaises(composed.Invalid):
                    composed.baseline_properties(SimpleNamespace(ffprobe='ffprobe'), 'candidate', 1, 'baseline', 1, {})

    def test_fractional_grid_missing_fields_and_uncovered_samples_fail(self):
        edits, packets = timing()
        invalid_edits = dataclasses.replace(edits, movie_scale=1001)
        with self.assertRaises(composed.Invalid):
            composed.sample_map(invalid_edits, packets, 4800)
        for name in ('pts', 'dts', 'duration', 'pos', 'size'):
            changed = [dict(p) for p in packets]
            del changed[2][name]
            with self.assertRaises(composed.Invalid):
                composed.sample_map(edits, changed, 4800)
        with self.assertRaises(composed.Invalid):
            composed.sample_map(edits, packets[:-1], 4800)
        changed = [dict(p) for p in packets]
        changed[0]['pos'] = 0
        with self.assertRaises(composed.Invalid):
            composed.sample_map(edits, changed, 4800)


class EditReader(unittest.TestCase):
    def read(self, data, track_id=7):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'headers.mp4'
            path.write_bytes(data)
            return composed.read_edits(path, track_id)

    def test_track_id_is_not_the_ffprobe_stream_index(self):
        entries = ((100, 1024, 1, 0),)
        self.assertEqual(self.read(movie(entries)).track_id, 7)
        with self.assertRaises(composed.Invalid):
            self.read(movie(entries), 0)

    def test_version_one_and_nonzero_start(self):
        entries = ((250, -1, 1, 0), (100, 1024, 1, 0))
        self.assertEqual(self.read(movie(entries, elst=edits_blob(entries, 1))).entries, entries)

    def test_interior_edits_preserving_endpoints_are_rejected(self):
        media = ((25, 1024, 1, 0), (50, 2225, 1, 0), (25, 4624, 1, 0))
        for empty in ((), ((250, -1, 1, 0),)):
            with self.assertRaises(composed.Invalid):
                self.read(movie(empty + media))

    def test_unknown_versions_duplicates_trailing_and_truncated_edits_fail(self):
        entries = ((100, 1024, 1, 0),)
        valid = edits_blob(entries)
        for payload in (b'\2' + valid[1:], valid + b'\0', valid[:-1], valid[:4] + b'\0' * 4):
            with self.assertRaises(composed.Invalid):
                self.read(movie(entries, elst=payload))
        for data in (movie(entries, duplicate_track=True), movie(entries, extra_tables=box(b'ctts', b'\0' * 8)),
                     movie(entries) + box(b'moof', b''), movie(entries)[:-1]):
            with self.assertRaises(composed.Invalid):
                self.read(data)

    def test_mdat_is_skipped_instead_of_read_as_control_bytes(self):
        entries = ((100, 1024, 1, 0),)
        header = movie(entries)[8200:]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'sparse.mp4'
            with path.open('wb') as out:
                out.write(struct.pack('>I4s', 32 << 20, b'mdat'))
                out.seek((32 << 20) - 1)
                out.write(b'\0' + header)
            reader = composed.Headers(path)
            try:
                self.assertEqual(reader.selected(7).entries, entries)
                self.assertLess(reader.control, 2048)
            finally:
                reader.file.close()


class Registration(unittest.TestCase):
    def test_the_inherited_audio_policy_checker_keeps_its_registered_bytes(self):
        root = Path(__file__).resolve().parents[2]
        self.assertEqual(composed.sha256_file(root / 'scripts/oracle_audio.py'),
                         composed.registration()[0]['legacy']['audio_input_policy_sha256'])

    def test_policy_matches_the_first_registration(self):
        self.assertEqual(composed.check_policy()['version'], composed.VERSION)

    def test_duplicate_keys_and_nonfinite_values_are_refused(self):
        for data in ('{"x":1,"x":2}', '{"x":NaN}', '{"x":Infinity}'):
            with self.assertRaises(composed.Invalid):
                composed.json_document(data)

    def test_unversioned_and_current_evidence_never_mix(self):
        _, identity = composed.registration()
        composed.require_version({}, None)
        composed.require_version({'compat_check': identity}, identity)
        for doc, wanted in (({}, identity), ({'compat_check': identity}, None),
                            ({'compat_check': dict(identity, registration_sha256='0' * 64)}, identity)):
            with self.assertRaises(composed.Invalid):
                composed.require_version(doc, wanted)

    def test_new_selection_cannot_summarize_or_resume_a_historical_result(self):
        historical = {'complete': True, 'arms': {}, 'baseline': {'status': 'ok'}}
        checker.validate_result(historical, None)
        with self.assertRaises(composed.Invalid):
            checker.validate_result(historical, composed.registration()[1])
        relabeled = dict(historical, compat_check=composed.registration()[1])
        with self.assertRaises(composed.Invalid):
            checker.validate_result(relabeled, composed.registration()[1])

    def test_version_one_cannot_be_reused_or_relabeled_as_version_two(self):
        root = Path(__file__).resolve().parents[2]
        historical = composed.json_document((root / 'docs/design/composed-opus-aac-v1.json').read_bytes())
        self.assertEqual(historical['version'], 'composed-opus-aac-v1')
        old = {'version': historical['version'], 'registration_sha256':
               composed.sha256_file(root / 'docs/design/composed-opus-aac-v1.json')}
        for identity in (old, dict(old, version=composed.VERSION)):
            with self.assertRaises(composed.Invalid):
                composed.require_version({'compat_check': identity}, composed.registration()[1])


class Receipt(unittest.TestCase):
    def setUp(self):
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.capture = self.root / ('a' * 32)
        self.capture.mkdir()
        self.source, self.output, self.program = (self.root / name for name in ('source', 'output', 'program'))
        for path in (self.source, self.output, self.program):
            path.write_bytes(path.name.encode())
        self.pcm = self.capture / 'stdin.f32'
        self.pcm.write_bytes(struct.pack('<ff', 0.2, -0.3))
        self.tools = {'coreav': 'b' * 64, 'ffmpeg': composed.sha256_file(self.program)}
        self.command = ['-i', str(self.source), '-c:a', 'aac', str(self.output)]
        self.expected = composed.expected_encoder(48000, 1, None)
        self.receipt = {
            'schema': 'coreav.encoder-input/1', 'complete': True, **composed.registration()[1],
            'call': self.capture.name, 'command': self.command,
            'source_sha256': composed.sha256_file(self.source),
            'output_sha256': composed.sha256_file(self.output), 'candidate_sha256': self.tools['coreav'],
            'audio': {'stream_index': 2, 'codec': 'opus', 'encoder': 'aac', 'channels': 1,
                      'samples': 2, 'sample_rate': 48000},
            'input': {'format': 'f32le', 'sample_rate': 48000, 'channel_mask': 4, 'start_ticks': 0,
                      'encoder_sha256': self.tools['ffmpeg'], 'argv': self.expected,
                      'sandboxed': True, 'address_space': 2 << 30, 'open_files': 64,
                      'launcher_argv': ['__coreav_exec', '--address-space', str(2 << 30), '--cpu-seconds',
                                        '100', '--open-files', '64', '--', str(self.program), *self.expected],
                      'pcm': {'path': str(self.pcm), 'bytes': 8, 'sha256': composed.sha256_file(self.pcm)}}}

    def validate(self):
        (self.capture / 'receipt.json').write_text(json.dumps(self.receipt))
        return composed.validate_receipt(self.capture, self.source, self.output, self.command, 2, None, self.tools)

    def test_receipt_binds_successful_pcm_to_actual_source_stream_and_command(self):
        _, path, capture = self.validate()
        self.assertEqual(path, self.pcm)
        self.assertEqual(capture['stream_index'], 2)

    def test_wrong_pcm_and_wrong_invocation_cannot_reuse_a_receipt(self):
        self.pcm.write_bytes(struct.pack('<ff', 0.2, -0.3000001))
        with self.assertRaises(composed.Invalid):
            self.validate()
        self.pcm.write_bytes(struct.pack('<ff', 0.2, -0.3))
        self.receipt['call'] = 'c' * 32
        with self.assertRaises(composed.Invalid):
            self.validate()

    def test_wrong_stream_rate_count_layout_and_incomplete_capture_fail(self):
        original = json.dumps(self.receipt)
        for block, key, value in [('audio', 'stream_index', 1), ('audio', 'samples', 1),
                                  ('input', 'sample_rate', 44100), ('input', 'channel_mask', 3),
                                  ('input', 'open_files', 65), ('audio', 'stream_index', True)]:
            self.receipt = json.loads(original)
            self.receipt[block][key] = value
            with self.assertRaises(composed.Invalid):
                self.validate()
        self.receipt = json.loads(original)
        self.receipt['complete'] = False
        with self.assertRaises(composed.Invalid):
            self.validate()

    def test_actual_encoder_argument_mutation_does_not_choose_its_own_reference(self):
        for field in ('argv', 'launcher_argv'):
            with self.subTest(field=field):
                original = json.dumps(self.receipt)
                self.receipt['input'][field] = self.receipt['input'][field][:-3] + ['-b:a', '32000', '-f', 'matroska', 'pipe:1']
                with self.assertRaises(composed.Invalid):
                    self.validate()
                self.receipt = json.loads(original)


class Projection(unittest.TestCase):
    def test_the_mono_comparator_reference_is_two_identical_channels(self):
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory) / 'source', Path(directory) / 'target'
            source.write_bytes(struct.pack('<ff', 0.25, -0.25))
            composed.project_s16(source, target, 2, 1, stereo_reference=True)
            self.assertEqual(struct.unpack('<4h', target.read_bytes()), (8192, 8192, -8192, -8192))

    def test_clipping_and_halfway_values_follow_the_registered_projection(self):
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory) / 'source', Path(directory) / 'target'
            values = [-2.0, -1.0, -1.5 / 32768, -0.5 / 32768, 0.5 / 32768, 1.5 / 32768, 1.0, 2.0]
            source.write_bytes(struct.pack('<8f', *values))
            composed.project_s16(source, target, 8, 1)
            self.assertEqual(struct.unpack('<8h', target.read_bytes()), (-32768, -32768, -2, 0, 0, 2, 32767, 32767))

    def test_nonfinite_or_truncated_samples_fail(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / 'source'
            for index, data in enumerate((struct.pack('<f', float('nan')), b'\0' * 3)):
                source.write_bytes(data)
                with self.assertRaises(composed.Invalid):
                    composed.project_s16(source, Path(directory) / f'target-{index}', 1, 1)


class Mapping(unittest.TestCase):
    def test_the_baseline_mapping_not_the_candidate_selects_the_audio(self):
        streams = [{'index': 0, 'codec_type': 'video'}, {'index': 1, 'codec_type': 'audio'},
                   {'index': 2, 'codec_type': 'audio'}]
        stderr = b'header\nStream mapping:\n  Stream #0:0 -> #0:0 (vp9 -> h264)\n  Stream #0:2 -> #0:1 (opus -> aac)\nPress [q]\n'
        self.assertEqual(composed.source_mapping(stderr, [1], streams), {1: 2})
        for changed in (b'', stderr + stderr, stderr.replace(b'#0:2', b'#0:0'),
                        stderr.replace(b'Press [q]', b'  Stream #0:1 -> #0:1 (opus -> aac)')):
            with self.assertRaises(composed.Invalid):
                composed.source_mapping(changed, [1], streams)

    def test_unedited_packet_metadata_is_bounded_and_bound_to_track_id(self):
        edits, packets = timing()
        stream = {'index': 1, 'id': '0x7', 'codec_name': 'aac', 'sample_rate': '48000',
                  'channels': 1, 'time_base': '1/48000', 'extradata_hash': 'SHA256:' + 'a' * 64,
                  'extradata_size': 5}
        for packet in packets:
            packet['data_hash'] = 'SHA256:' + 'b' * 64
            packet['pos'], packet['size'] = str(packet['pos']), str(packet['size'])
        with mock.patch.object(composed, 'bounded', return_value=(json.dumps({'streams': [stream], 'packets': packets}).encode(), b'')) as command, \
                mock.patch.object(composed, 'read_edits', return_value=edits) as read:
            _, hashes = composed.read_aac_timing('ffprobe', 'file', 1, 4800, {}, artifact='candidate')
            self.assertEqual(hashes['channels'], 1)
            self.assertIn('-ignore_editlist', command.call_args.args[0])
            read.assert_called_once_with('file', 7)
            for artifact in ('candidate', 'AAC baseline', 'reference readback'):
                with self.assertRaises(composed.Invalid) as failure:
                    composed.read_aac_timing('ffprobe', 'file', 1, 4799, {}, artifact=artifact)
                reason, raw = str(failure.exception).split('; timing=', 1)
                self.assertEqual(reason, 'edit does not describe the exact captured sample grid')
                self.assertEqual(json.loads(raw), {
                    'artifact': artifact, 'track_id': 7, 'edit_media_time': 1024,
                    'edit_segment_duration': 100, 'movie_timescale': 1000, 'media_timescale': 48000,
                    'priming_samples': 1024, 'sample_count': {'numerator': 4800, 'denominator': 1},
                    'packet_duration_samples': 6144, 'packet_count': 6, 'captured_sample_count': 4799})


class SourcePresentation(unittest.TestCase):
    def setUp(self):
        self.edits = composed.Edits(7, 48000, 48000, ((960, 312, 1, 0),), ((8, 8200),), 1, 312)
        self.packets = [{'pts': 0, 'dts': 0, 'duration': 960, 'pos': 8, 'size': 16},
                        {'pts': 960, 'dts': 960, 'duration': 312, 'pos': 24, 'size': 16}]

    def test_source_window_uses_declared_edits_and_packet_work_not_candidate_length(self):
        self.assertEqual(source_model.source_window(self.edits, self.packets, lambda *_: b'\x98\0'),
                         (0, 960, 0, 1608))
        with self.assertRaises(composed.Invalid):
            source_model.source_window(dataclasses.replace(self.edits, entries=((2000, 312, 1, 0),)),
                                       self.packets, lambda *_: b'\x98\0')

    def test_packet_clock_payload_bounds_and_codec_work_are_required(self):
        for key, value in [('pts', 959), ('dts', 959), ('duration', 0), ('pos', 8200)]:
            packets = json.loads(json.dumps(self.packets))
            packets[1][key] = value
            with self.assertRaises(composed.Invalid):
                source_model.source_window(self.edits, packets, lambda *_: b'\x98\0')
        for header in (b'', b'\x9b', b'\x9b\0', b'\x9b\x3f'):
            with self.assertRaises(composed.Invalid):
                source_model.packet_samples(header)

    def test_full_decode_must_match_source_work_before_applying_source_edits(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / 'source.mp4'
            data = bytearray(movie(((20, 312, 1, 0),), opus=True))
            data[8] = data[24] = 0x98
            path.write_bytes(data)
            document = {'streams': [{'index': 0, 'id': '0x7', 'codec_name': 'opus',
                         'channels': 1, 'sample_rate': '48000', 'time_base': '1/48000'}], 'packets': self.packets}
            args = SimpleNamespace(ffmpeg='ffmpeg', ffprobe='ffprobe')
            for count in (1607, 1608, 1609):
                def run(command, env, **kwargs):
                    if command[0] == 'ffprobe':
                        return json.dumps(document).encode(), b''
                    self.assertIn('-ignore_editlist', command)
                    self.assertEqual(kwargs['limit'], 1608 * 4)
                    kwargs['output'].write(struct.pack('<' + 'f' * count, *range(count)))
                    return b'', b''
                with mock.patch.object(composed, 'bounded', side_effect=run):
                    if count != 1608:
                        with self.assertRaises(composed.Invalid):
                            source_model.source_reference(args, root, path, 0, 1, {})
                    else:
                        target, presentation = source_model.source_reference(args, root, path, 0, 1, {})
                        self.assertEqual(presentation, {'frames': 960, 'rate': 48000, 'channels': 1, 'start_ticks': 0})
                        self.assertEqual(target.read_bytes(), struct.pack('<' + 'f' * 960, *range(960)))
                        target.unlink()

    def test_matroska_reference_binds_the_decoded_count_and_zero_origin(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / 'source.webm'
            path.write_bytes(bytes.fromhex('1a45dfa3'))
            args = SimpleNamespace(ffmpeg='ffmpeg', ffprobe='ffprobe')
            for origin, count in ((0, 4), (1, 4), (0, 3)):
                target = root / 'libopus.f32'
                target.unlink(missing_ok=True)

                def run(command, env, **kwargs):
                    if command[0] == 'ffmpeg':
                        kwargs['output'].write(struct.pack('<4f', 0, 1, 2, 3))
                        return b'', b''
                    return json.dumps({'streams': [{'index': 2, 'codec_name': 'opus',
                        'channels': 1, 'sample_rate': '48000'}],
                        'frames': [{'pts': origin, 'nb_samples': count}]}).encode(), b''

                with mock.patch.object(composed, 'bounded', side_effect=run):
                    if origin == 0 and count == 4:
                        _, presentation = source_model.source_reference(args, root, path, 2, 1, {})
                        self.assertEqual(presentation, {'frames': 4, 'rate': 48000, 'channels': 1, 'start_ticks': 0})
                    else:
                        with self.assertRaises(composed.Invalid):
                            source_model.source_reference(args, root, path, 2, 1, {})


class BoundedCommands(unittest.TestCase):
    def test_metadata_output_and_timeout_fail_closed(self):
        with self.assertRaises(composed.Invalid):
            composed.bounded([sys.executable, '-c', 'print("x" * 1000)'], {}, limit=10)
        with self.assertRaises(composed.Invalid):
            composed.bounded([sys.executable, '-c', 'import time; time.sleep(10)'], {}, timeout=0.05)


if __name__ == '__main__':
    unittest.main()
