"""Matroska exclusions bind generated track bytes to both packet listings."""
import copy
import dataclasses
import functools
import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))
import corpus_kd as kd

SPEC = importlib.util.spec_from_file_location('corpus_mkv_fixtures', kd.ROOT / 'tools/oracle/tests/test_kd_mkv.py')
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)
fixtures.ac3_frame = functools.lru_cache()(fixtures.ac3_frame)
CASES = (kd.MKV_DOWNMIX, kd.AV1_DEFAULT, kd.AV1_CHROMA)


class Matroska(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.media = self.root / 'generated.mkv'
        self.reference_packets = self.root / 'reference.packets'
        self.candidate_packets = self.root / 'candidate.packets'
        self.registry = kd.production_registry()

    def entry(self, number, kind, codec, private=None):
        e = fixtures.ebml
        fields = e(0xD7, bytes([number])) + e(0x73C5, bytes([number])) + e(0x83, bytes([kind]))
        if codec is not None:
            fields += e(0x86, codec)
        if private is not None:
            fields += e(0x63A2, private) + e(0xE0, e(0xB0, b'\x40') + e(0xBA, b'\x40'))
        elif codec == b'A_AC3':
            fields += e(0xE1, e(0xB5, b'\x46\xbb\x80\x00') + e(0x9F, b'\x06'))
        return e(0xAE, fields)

    def write_media(self, entries, blocks):
        e = fixtures.ebml
        payloads, elements = [], []
        for i, (number, frames, lacing, group) in enumerate(blocks):
            lace = b''
            if lacing:
                lace = bytes([len(frames) - 1])
                if lacing == 1:
                    lace += bytes(len(frame) for frame in frames[:-1])
                elif lacing == 3:
                    size = len(frames[0])
                    lace += bytes([0x80 | size]) if size < 127 else bytes([0x40 | size >> 8, size & 255])
                    lace += b'\xbf' * (len(frames) - 2)
            payload = bytes([0x80 | number, 0, 32 * i, 0x80 | lacing << 1]) + lace + b''.join(frames)
            block = e(0xA1 if group else 0xA3, payload)
            elements.append(e(0xA0, block) if group else block)
            payloads.append((payload, frames, number))
        data = (e(0x1A45DFA3, e(0x4282, b'matroska') + e(0x4287, b'\x04') + e(0x4285, b'\x02'))
                + e(0x18538067, e(0x1549A966, e(0x2AD7B1, b'\x0f\x42\x40'))
                    + e(0x1654AE6B, b''.join(entries)) + e(0x1F43B675, e(0xE7, b'\x00') + b''.join(elements))))
        self.media.write_bytes(data)
        return [(data.index(payload), len(frame), number) for payload, frames, number in payloads for frame in frames]

    def make(self, case, *, before=False, count=1, lacing=0, group=False,
             empty=False, zero=False, corrupt=False, codec=None):
        entries, reference, candidate, blocks, numbers = [], [], [], [], []
        if before:
            entries.append(self.entry(7, 2, b'A_OPUS'))
            reference.append({'index': 0, 'codec_name': 'opus'})
            candidate.append(copy.deepcopy(reference[0]))
            numbers.append(7)
        for ordinal in range(count):
            index, number = len(reference), ordinal + 1
            numbers.append(number)
            common = {'index': index}
            if case == kd.MKV_DOWNMIX:
                entries.append(self.entry(number, 2, codec or b'A_AC3'))
                frame = fixtures.ac3_frame(mode=ordinal + 1, crc=not corrupt)
                keys = kd.oracle_kd_mkv.DOWNMIX_KEYS
                theirs = dict(zip(keys, kd.oracle_kd_mkv.NO_DOWNMIX, strict=True))
                ours = dict(zip(keys, (str(ordinal + 1),) + ('0.707107',) * 4, strict=True))
                common.update(codec_name='ac3', codec_type='audio')
            else:
                position = int(case == kd.AV1_CHROMA)
                private = fixtures.av1_private(level=5 + ordinal, position=position,
                                               colour=None if position else (1, 1, 1))
                entries.append(self.entry(number, 1, codec or b'V_AV1', private))
                short = f'av01.0.{5 + ordinal:02}M.08'
                theirs = {'mime_codec_string': short if position else short + '.0.110.01.01.01.0'}
                ours = {'mime_codec_string': short + '.0.111.01.01.01.0' if position else short}
                frame = b'\x7a\x01\x00'
            reference.append(dict(common, **theirs))
            candidate.append(dict(common, **ours))
            if not empty:
                frames = [b'' if zero else frame] * (3 if lacing else 1)
                blocks.append((number, frames, lacing, group))
        spans = self.write_media(entries, blocks)
        rows = [{'stream_index': str(numbers.index(number)), 'pos': str(offset), 'size': str(size), 'pts': '0'}
                for offset, size, number in spans]
        self.write_packets(rows)
        return kd.Capture({'streams': reference}, {'streams': candidate}, self.reference_packets, self.candidate_packets)

    def write_packets(self, reference, candidate=None):
        for path, rows in ((self.reference_packets, reference),
                           (self.candidate_packets, reference if candidate is None else candidate)):
            path.write_text(''.join('packet|' + '|'.join(key + '=' + value for key, value in row.items()) + '\n'
                                    for row in rows))

    def compare(self, capture):
        return kd.compare(capture, self.media, self.registry, 'matroska')

    def assert_covered(self, capture, case):
        result = self.compare(capture)
        fields = kd.oracle_kd_mkv.DOWNMIX_KEYS if case == kd.MKV_DOWNMIX else ('mime_codec_string',)
        self.assertEqual(result['excluded'], {'streams.' + field: [case] for field in fields})
        for field in fields:
            self.assertFalse(result['keys']['streams.' + field])
        self.assertTrue(result['keys'].get('packets.pos', True))

    def test_all_keys_bind_exact_values_positions_and_track_packet_spans(self):
        for case in CASES:
            for before in (False, True):
                with self.subTest(case=case, before=before):
                    self.assert_covered(self.make(case, before=before), case)

    def test_simple_blocks_block_groups_and_all_laces_bind_each_frame(self):
        for case in CASES:
            for lacing, group in ((0, True), (1, False), (2, True), (3, False)):
                with self.subTest(case=case, lacing=lacing, group=group):
                    self.assert_covered(self.make(case, lacing=lacing, group=group), case)

    def test_each_missing_null_or_changed_target_value_stays_uncovered(self):
        for case in CASES:
            fields = kd.oracle_kd_mkv.DOWNMIX_KEYS if case == kd.MKV_DOWNMIX else ('mime_codec_string',)
            for side in ('reference', 'candidate'):
                for field in fields:
                    for value in ('absent', None, '9', '0.500000'):
                        with self.subTest(case=case, side=side, field=field, value=value):
                            capture = self.make(case)
                            stream = getattr(capture, side)['streams'][0]
                            if value == 'absent':
                                del stream[field]
                            else:
                                stream[field] = value
                            self.assertNotIn('streams.' + field, self.compare(capture)['excluded'])

    def test_section_or_packet_failure_never_gets_coverage(self):
        for case in CASES:
            for code in ('section_code', 'packet_code'):
                with self.subTest(case=case, code=code):
                    capture = self.make(case)
                    setattr(capture, code, 1)
                    self.assertEqual(self.compare(capture)['excluded'], {})

    def test_no_frames_zero_frames_bad_crc_or_another_codec_stay_uncovered(self):
        for case in CASES:
            for options in ({'empty': True}, {'zero': True}, {'codec': b'A_EAC3'}):
                with self.subTest(case=case, options=options):
                    self.assertEqual(self.compare(self.make(case, **options))['excluded'], {})
        self.assertEqual(self.compare(self.make(kd.MKV_DOWNMIX, corrupt=True))['excluded'], {})

    def test_unreadable_or_incomplete_bytes_refuse_evidence(self):
        for case in CASES:
            for content in (b'not Matroska', None):
                with self.subTest(case=case, content=content):
                    capture = self.make(case)
                    data = self.media.read_bytes()
                    self.media.write_bytes(data[:-1] if content is None else content)
                    self.assertEqual(self.compare(capture)['excluded'], {})

    def test_duplicate_track_numbers_are_explicitly_refused(self):
        for case in CASES:
            capture = self.make(case, before=True)
            data = self.media.read_bytes()
            self.media.write_bytes(data.replace(fixtures.ebml(0xD7, b'\x07'), fixtures.ebml(0xD7, b'\x01'), 1))
            self.assertEqual(self.compare(capture)['excluded'], {})

    def test_stream_indices_are_required_independently_on_both_sides(self):
        for case in CASES:
            for side in ('reference', 'candidate'):
                for index in (None, True, 1, '0'):
                    with self.subTest(case=case, side=side, index=index):
                        capture = self.make(case)
                        getattr(capture, side)['streams'][0]['index'] = index
                        self.assertEqual(self.compare(capture)['excluded'], {})
                        getattr(capture, side)['streams'][0]['index'] = 0
                        self.assert_covered(capture, case)

    def test_an_omitted_track_does_not_turn_a_stream_index_into_a_track_position(self):
        for case in CASES:
            capture = self.make(case, before=True)
            for side in ('reference', 'candidate'):
                getattr(capture, side)['streams'] = [getattr(capture, side)['streams'][1]]
                getattr(capture, side)['streams'][0]['index'] = 0
            for path in (self.reference_packets, self.candidate_packets):
                path.write_text(path.read_text().replace('stream_index=1', 'stream_index=0'))
            self.assertEqual(self.compare(capture)['excluded'], {})

    def test_shifted_or_swapped_stream_arrays_are_never_searched(self):
        for case in CASES:
            for relabel in (False, True):
                with self.subTest(case=case, relabel=relabel):
                    capture = self.make(case, count=2)
                    capture.candidate['streams'].reverse()
                    if relabel:
                        for i, stream in enumerate(capture.candidate['streams']):
                            stream['index'] = i
                        rows = [dict(row, stream_index=str(1 - int(row['stream_index'])))
                                for row in kd.packet_rows(self.candidate_packets)]
                        self.write_packets(list(kd.packet_rows(self.reference_packets)), rows)
                    self.assertEqual(self.compare(capture)['excluded'], {})
                    self.assertEqual(self.compare(capture)['validated'], {})

    def test_each_listing_must_match_every_span_in_order(self):
        for case in CASES:
            for side in ('reference', 'candidate'):
                for action in ('position', 'size', 'stream', 'drop', 'duplicate', 'swap'):
                    with self.subTest(case=case, side=side, action=action):
                        capture = self.make(case, count=2)
                        reference = list(kd.packet_rows(self.reference_packets))
                        candidate = copy.deepcopy(reference)
                        rows = reference if side == 'reference' else candidate
                        if action in ('position', 'size', 'stream'):
                            field = {'position': 'pos', 'size': 'size', 'stream': 'stream_index'}[action]
                            rows[0][field] = str(int(rows[0][field]) + 1)
                        elif action == 'drop':
                            del rows[0]
                        elif action == 'duplicate':
                            rows.insert(0, copy.deepcopy(rows[0]))
                        else:
                            rows[0]['pos'], rows[1]['pos'] = rows[1]['pos'], rows[0]['pos']
                        self.write_packets(reference, candidate)
                        result = self.compare(capture)
                        self.assertEqual(result['validated'].get('streams.dmix_mode' if case == kd.MKV_DOWNMIX
                                                               else 'streams.mime_codec_string', 0),
                                         0 if action in ('stream', 'swap') else 1)
                        self.assertEqual(result['excluded'], {})

    def test_a_later_uncovered_occurrence_keeps_the_file_key_row(self):
        for case in CASES:
            capture = self.make(case, count=2)
            rows = list(kd.packet_rows(self.candidate_packets))
            rows[1]['pos'] = '1'
            self.write_packets(list(kd.packet_rows(self.reference_packets)), rows)
            result = self.compare(capture)
            self.assertEqual(result['excluded'], {})
            for key in ('streams.' + field for field in capture.reference['streams'][0] if field != 'index'):
                if key in result['validated']:
                    self.assertEqual(result['differences'][key], 2)
                    self.assertEqual(result['validated'][key], 1)

    def test_direct_agreement_and_program_nested_copies_are_not_exclusions(self):
        for case in CASES:
            capture = self.make(case)
            capture.reference['programs'] = [{'streams': copy.deepcopy(capture.reference['streams'])}]
            capture.candidate['programs'] = [{'streams': copy.deepcopy(capture.candidate['streams'])}]
            result = self.compare(capture)
            self.assertTrue(result['excluded'])
            self.assertFalse(any(key.startswith('programs.') for key in result['excluded']))
            capture.candidate = copy.deepcopy(capture.reference)
            self.assertEqual(self.compare(capture)['excluded'], {})

    def test_callbacks_cannot_certify_another_records_key(self):
        capture = self.make(kd.AV1_DEFAULT)
        facts = self.registry.evidence(self.media, 'matroska')['matroska']
        occurrence = next(o for o in kd.occurrences(capture, summarize=True) if o.key == 'streams.mime_codec_string')
        self.assertIsNone(kd.mkv_downmix_expected((facts[1], (), ()), occurrence))
        capture = self.make(kd.MKV_DOWNMIX)
        facts = self.registry.evidence(self.media, 'matroska')['matroska']
        occurrence = next(o for o in kd.occurrences(capture, summarize=True) if o.key == 'streams.dmix_mode')
        self.assertIsNone(kd.mkv_default_expected(((), facts[0], ()), occurrence))

    def test_dispatch_only_reads_the_selected_container_and_propagates_bugs(self):
        capture = self.make(kd.MKV_DOWNMIX)
        forbidden = mock.Mock(side_effect=AssertionError('wrong container reader'))
        with mock.patch.dict(self.registry.readers, {'mpegts': forbidden}):
            self.assert_covered(capture, kd.MKV_DOWNMIX)
        forbidden.assert_not_called()
        selected = mock.Mock(return_value=object())
        with mock.patch.dict(self.registry.readers, {'mpegts': selected, 'matroska': forbidden}):
            self.assertEqual(self.registry.evidence(self.media, 'mpegts'), {'mpegts': selected.return_value})
        selected.assert_called_once_with(self.media)
        forbidden.assert_not_called()
        with mock.patch.dict(self.registry.readers, {'matroska': forbidden}):
            self.assertEqual(self.registry.evidence(self.media, 'isobmff'), {})
        forbidden.assert_not_called()
        for container in ('mpegts', 'matroska'):
            with mock.patch.dict(self.registry.readers, {container: mock.Mock(side_effect=RuntimeError('reader bug'))}):
                with self.assertRaisesRegex(RuntimeError, 'reader bug'):
                    self.registry.evidence(self.media, container)

    def test_a_failed_av1_walk_does_not_supply_or_remove_another_tracks_evidence(self):
        for case in (kd.AV1_DEFAULT, kd.AV1_CHROMA):
            capture = self.make(case, count=2)
            original = kd.oracle_kd_mkv.policy_ref.matroska_block_frames
            def walk(path, number):
                if number == 2:
                    raise ValueError('unreadable track blocks')
                return original(path, number)
            with mock.patch.object(kd.oracle_kd_mkv.policy_ref, 'matroska_block_frames', side_effect=walk):
                result = self.compare(capture)
            self.assertEqual(result['excluded'], {})
            self.assertEqual(result['validated']['streams.mime_codec_string'], 1)
            with mock.patch.object(kd.oracle_kd_mkv.policy_ref, 'matroska_block_frames',
                                   side_effect=RuntimeError('walk bug')):
                with self.assertRaisesRegex(RuntimeError, 'walk bug'):
                    self.compare(capture)

    def test_all_used_cases_survive_the_standard_mutation_audit(self):
        for case in CASES:
            for options in ({}, {'before': True}, {'lacing': 1}):
                with self.subTest(case=case, options=options):
                    capture = self.make(case, **options)
                    result = kd.audit(capture, self.media, self.registry, 'matroska')
                    self.assertTrue(result['passed'], result)

    def test_registry_freezes_container_scopes_and_reader_identities(self):
        records = [record for record in self.registry.records() if record['container'] == 'matroska']
        self.assertEqual({record['id'] for record in records}, set(CASES))
        for record in records:
            self.assertFalse(record['requires_pid'])
            self.assertFalse(record['permits_absence'])
            self.assertTrue(record['binds_packets'])
            self.assertLessEqual(set(record['keys']), kd.authorized_keys(record['record']))
        for rule in (rule for rule in self.registry.rules if rule.binds_packets):
            with self.assertRaises(ValueError):
                kd.Registry((dataclasses.replace(rule, container='isobmff'),), {})
        root = self.root
        (root / 'scripts').mkdir()
        (root / 'scripts/oracle-ffmpeg.env').write_bytes(b'pin')
        for path in kd.READER_PATHS:
            (root / path).write_bytes(b'original reader')
        def source(root, *args):
            if args[0] == 'merge-base':
                return b''
            if args[-1].endswith('scripts/oracle-ffmpeg.env'):
                return b'pin'
            for document in {rule.document for rule in self.registry.rules}:
                if args[-1].endswith(document):
                    return json.dumps({'entries': [rule.record for rule in self.registry.rules
                                                  if rule.document == document]}).encode()
            return b'old plan' if '^:' in args[-1] else kd.AMENDMENT.encode()
        with mock.patch.object(kd, 'git', side_effect=source):
            frozen = kd.inventory(root, 'a' * 40, self.registry, '0' * 64)
            self.assertIn('reader_sha256', frozen)
            self.assertEqual(set(frozen['reader_sha256']), set(kd.READER_PATHS))
            (root / kd.READER_PATHS[-1]).write_bytes(b'changed reader')
            self.assertNotEqual(frozen, kd.inventory(root, 'a' * 40, self.registry, '0' * 64))


if __name__ == '__main__':
    unittest.main()
