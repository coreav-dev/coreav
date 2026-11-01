"""ISO exclusions bind landed byte proofs to the exact final packet."""
import collections
import copy
import dataclasses
import importlib.util
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))
import corpus_kd as kd

SPEC = importlib.util.spec_from_file_location('corpus_iso_fixtures', kd.ROOT / 'tools/oracle/tests/test_kd_empty_edit.py')
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)


class Iso(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.media = self.root / 'generated.mp4'
        self.reference_packets = self.root / 'reference.packets'
        self.candidate_packets = self.root / 'candidate.packets'
        self.registry = kd.production_registry()

    def make(self, absence=False, frame960=False):
        if absence:
            options = dict(edits=((3, -1, 1 << 16), (200, 0, 1 << 16)),
                           stts=((6, 960 if frame960 else 1024),),
                           asc=bytes([fixtures.LC_48K[0], fixtures.LC_48K[1] | (4 if frame960 else 0)]))
            oracle, coreav = '144', None
        else:
            options = dict(edits=((30, -1, 1 << 16), (100, 1024, 1 << 16)))
            oracle, coreav = '740', '324'
        data, last = fixtures.movie(**options)
        self.media.write_bytes(data)
        count = sum(n for n, _ in options.get('stts', ((5, 1024), (1, 700))))
        self.reference = [{'stream_index': '0', 'pos': str(last - 10 * (count - 1 - i)),
                           'size': '10', 'pts': str(i * 1024)} for i in range(count)]
        self.candidate = copy.deepcopy(self.reference)
        self.reference[-1]['side_data_list.discard_padding'] = oracle
        if coreav is not None:
            self.candidate[-1]['side_data_list.discard_padding'] = coreav
        self.write_packets()
        sections = {'streams': [{'index': 0, 'id': '0x1', 'codec_type': 'audio', 'codec_name': 'aac'}]}
        return kd.Capture(sections, copy.deepcopy(sections), self.reference_packets, self.candidate_packets)

    def write_packets(self):
        for path, rows in ((self.reference_packets, self.reference), (self.candidate_packets, self.candidate)):
            path.write_text(''.join('packet|' + '|'.join(key + '=' + value for key, value in row.items()) + '\n'
                                    for row in rows))

    def compare(self, capture):
        return kd.compare(capture, self.media, self.registry, 'isobmff')

    def assert_covered(self, capture):
        result = self.compare(capture)
        self.assertFalse(result['keys'][kd.ABSENCE_KEY])
        self.assertEqual(result['excluded'], {kd.ABSENCE_KEY: [kd.ABSENCE_KD]})

    def test_both_present_and_whole_frames_of_960_or_1024_bind_generated_bytes(self):
        for absence, flag in ((False, False), (True, False), (True, True)):
            with self.subTest(absence=absence, flag=flag):
                capture = self.make(absence, flag)
                self.assert_covered(capture)
                self.assertTrue(kd.audit(capture, self.media, self.registry, 'isobmff')['passed'])

    def test_44100_tick_rounding_binds_only_the_exact_864_and_26_pair(self):
        asc = bytes([2 << 3 | 4 >> 1, (4 & 1) << 7 | 1 << 3]) + fixtures.EXT
        data, last = fixtures.movie(edits=((19, -1, 1 << 16), (9984, 0, 1 << 16)),
                                    stts=((429, 1024), (1, 998)), asc=asc, timescale=44100)
        capture = self.make()
        self.media.write_bytes(data)
        self.reference = [{'stream_index': '0', 'pos': str(last), 'size': '10',
                           'side_data_list.discard_padding': '864'}]
        self.candidate = [dict(self.reference[0], **{'side_data_list.discard_padding': '26'})]
        self.write_packets()
        self.assert_covered(capture)
        self.candidate[0]['side_data_list.discard_padding'] = '27'
        self.write_packets()
        self.assertEqual(self.compare(capture)['excluded'], {})

    def test_helper_absence_transfers_by_keyword_even_if_field_order_changes(self):
        capture = self.make(True)
        original = kd.iso_facts(self.media)
        reordered = collections.namedtuple('ReorderedProof', reversed(kd.ProvenAbsence.__dataclass_fields__))
        with mock.patch.object(kd.oracle_kd, 'ProvenAbsence', reordered):
            transferred = kd.iso_facts(self.media)
            self.assertEqual(transferred, original)
            self.assertIs(type(transferred[0]), kd.ProvenAbsence)
            self.assert_covered(capture)

    def test_each_shape_refuses_the_other_shapes_candidate(self):
        for absence in (False, True):
            capture = self.make(absence)
            if absence:
                self.candidate[-1]['side_data_list.discard_padding'] = '324'
            else:
                del self.candidate[-1]['side_data_list.discard_padding']
            self.write_packets()
            self.assertEqual(self.compare(capture)['excluded'], {})

    def test_missing_reference_and_null_or_wrong_candidate_never_qualify(self):
        for absence in (False, True):
            capture = self.make(absence)
            occurrence = next(item for item in kd.occurrences(capture, summarize=True)
                              if item.key == kd.ABSENCE_KEY and item.different)
            evidence = self.registry.evidence(self.media, 'isobmff')
            for changed in (dataclasses.replace(occurrence, reference=None),
                            dataclasses.replace(occurrence, present=True, candidate=None),
                            dataclasses.replace(occurrence, present=True, candidate='325'),
                            dataclasses.replace(occurrence, reference='741')):
                self.assertEqual(self.registry.covers(changed, evidence), frozenset())

    def test_both_stream_ids_and_indices_are_required_without_searching(self):
        for absence in (False, True):
            for side in ('reference', 'candidate'):
                for field, value in (('id', '0x2'), ('index', 1), ('index', '0'), ('index', True)):
                    with self.subTest(absence=absence, side=side, field=field, value=value):
                        capture = self.make(absence)
                        stream = getattr(capture, side)['streams'][0]
                        old = stream[field]
                        stream[field] = value
                        self.assertEqual(self.compare(capture)['excluded'], {})
                        stream[field] = old
                        self.assert_covered(capture)

    def test_a_duplicate_track_id_on_either_side_is_not_selected(self):
        for absence in (False, True):
            for side in ('reference', 'candidate'):
                capture = self.make(absence)
                streams = getattr(capture, side)['streams']
                streams.append({'index': 1, 'id': '0x1'})
                self.assertEqual(self.compare(capture)['excluded'], {})
                streams.pop()
                self.assert_covered(capture)

    def test_packet_offset_size_and_stream_index_bind_both_listings(self):
        for absence in (False, True):
            for side in ('reference', 'candidate'):
                for field in ('pos', 'size', 'stream_index'):
                    with self.subTest(absence=absence, side=side, field=field):
                        capture = self.make(absence)
                        row = getattr(self, side)[-1]
                        old = row[field]
                        row[field] = str(int(old) + 1)
                        self.write_packets()
                        self.assertEqual(self.compare(capture)['excluded'], {})
                        row[field] = old
                        self.write_packets()
                        self.assert_covered(capture)

    def test_packet_must_be_last_on_both_sides_and_counts_cannot_hide_loss(self):
        for absence in (False, True):
            for side in ('reference', 'candidate'):
                for action in ('later', 'duplicate', 'drop'):
                    with self.subTest(absence=absence, side=side, action=action):
                        capture = self.make(absence)
                        rows = getattr(self, side)
                        if action == 'drop':
                            rows.pop(0)
                        else:
                            extra = dict(rows[-1])
                            if action == 'later':
                                extra['pos'] = str(int(extra['pos']) + 10)
                            rows.append(extra)
                        self.write_packets()
                        self.assertEqual(self.compare(capture)['excluded'], {})

    def test_every_other_packet_agrees_for_absence_but_not_for_a_present_pair(self):
        for absence in (False, True):
            capture = self.make(absence)
            self.candidate[0]['pts'] = '1'
            self.write_packets()
            result = self.compare(capture)
            self.assertFalse(result['keys']['packets.pts'])
            self.assertEqual(result['excluded'], {} if absence else {kd.ABSENCE_KEY: [kd.ABSENCE_KD]})

    def test_another_uncovered_occurrence_keeps_the_whole_key_row_visible(self):
        capture = self.make()
        self.reference[0]['side_data_list.discard_padding'] = '5'
        self.candidate[0]['side_data_list.discard_padding'] = '6'
        self.write_packets()
        result = self.compare(capture)
        self.assertEqual(result['validated'][kd.ABSENCE_KEY], 1)
        self.assertEqual(result['differences'][kd.ABSENCE_KEY], 2)
        self.assertEqual(result['excluded'], {})

    def test_only_the_padding_leaf_is_covered_not_sibling_keys_or_duration(self):
        capture = self.make(True)
        self.reference[-1].update({'side_data_list.side_data_type': 'Skip Samples',
                                   'side_data_list.skip_samples': '0', 'duration': '1024'})
        self.candidate[-1]['duration'] = '880'
        self.write_packets()
        self.assert_covered(capture)
        result = self.compare(capture)
        for key in ('packets.side_data_list.side_data_type', 'packets.side_data_list.skip_samples', 'packets.duration'):
            self.assertFalse(result['keys'][key])
            self.assertNotIn(key, result['excluded'])
        item = next(item for item in kd.occurrences(capture, summarize=True) if item.key == kd.ABSENCE_KEY)
        for changed in (dataclasses.replace(item, key='packets.duration'),
                        dataclasses.replace(item, path='streams[0].side_data_list.discard_padding')):
            self.assertIsNone(kd.iso_expected(kd.iso_facts(self.media), changed))

    def test_failed_section_or_packet_capture_never_qualifies(self):
        for absence in (False, True):
            for field in ('section_code', 'packet_code'):
                capture = self.make(absence)
                setattr(capture, field, 2)
                self.assertEqual(self.compare(capture)['excluded'], {})

    def test_unextended_short_samples_diagnostic_oracle_absence_and_garbage_are_uncovered(self):
        capture = self.make()
        for options in ({'asc': fixtures.LC_48K}, {'edits': ((50, -1, 1 << 16), (100, 1024, 1 << 16))}):
            data, last = fixtures.movie(**({'edits': ((30, -1, 1 << 16), (100, 1024, 1 << 16))} | options))
            self.media.write_bytes(data)
            for rows in (self.reference, self.candidate):
                for i, row in enumerate(rows):
                    row['pos'] = str(last - 10 * (len(rows) - 1 - i))
            self.write_packets()
            self.assertEqual(self.compare(capture)['excluded'], {})
        for data in (b'not a movie', b''):
            self.media.write_bytes(data)
            self.assertEqual(self.compare(capture)['excluded'], {})

    def test_deleting_324_padding_from_the_real_700_tick_straddle_is_caught(self):
        capture = self.make()
        def delete(i, row):
            return ({key: value for key, value in row.items() if key != 'side_data_list.discard_padding'},)
        audit = kd.audit(capture, self.media, self.registry, 'isobmff', [kd.Attack('required padding deleted', packets=delete)])
        self.assertTrue(audit['passed'])
        self.assertEqual(audit['attacks'][0]['verdict'], 'caught')
        del self.candidate[-1]['side_data_list.discard_padding']
        self.write_packets()
        self.assertEqual(self.compare(capture)['excluded'], {})

    def test_ambiguous_byte_evidence_never_picks_a_certificate(self):
        for absence in (False, True):
            capture = self.make(absence)
            facts = kd.iso_facts(self.media)
            with mock.patch.dict(self.registry.readers, {'isobmff': lambda path: facts + facts}):
                self.assertEqual(self.compare(capture)['excluded'], {})

    def test_dispatch_and_empty_evidence_do_not_read_or_summarize_other_containers(self):
        capture = self.make(True)
        forbidden = mock.Mock(side_effect=AssertionError('wrong container reader'))
        with mock.patch.dict(self.registry.readers, {'mpegts': forbidden, 'matroska': forbidden}):
            self.assert_covered(capture)
        forbidden.assert_not_called()
        selected = mock.Mock(return_value=object())
        with mock.patch.dict(self.registry.readers, {'isobmff': forbidden, 'mpegts': selected, 'matroska': selected}):
            for container in ('mpegts', 'matroska'):
                self.assertEqual(self.registry.evidence(self.media, container), {container: selected.return_value})
        forbidden.assert_not_called()
        self.media.write_bytes(b'not a movie')
        with mock.patch.object(kd, 'packet_summary', side_effect=AssertionError('no byte evidence')):
            self.assertEqual(self.compare(capture)['excluded'], {})

    def test_landed_parse_refusals_stay_uncovered_but_predicate_bugs_abort(self):
        capture = self.make(True)
        with mock.patch.object(kd.oracle_kd, 'Facts', side_effect=IndexError('unreadable table')):
            self.assertEqual(self.compare(capture)['excluded'], {})
        with mock.patch.object(kd.oracle_kd, 'empty_edit_padding', side_effect=RuntimeError('predicate bug')):
            with self.assertRaisesRegex(RuntimeError, 'predicate bug'):
                self.compare(capture)

    def test_production_record_has_one_frozen_key_and_its_reader_digest(self):
        rule, = [rule for rule in self.registry.rules if rule.identifier == kd.ABSENCE_KD]
        self.assertEqual(rule.keys, (kd.ABSENCE_KEY,))
        self.assertEqual(rule.container, 'isobmff')
        self.assertTrue(rule.permits_absence)
        self.assertEqual(rule.record['corpus_absence'], kd.ABSENCE_CASE)
        self.assertIn('scripts/oracle_kd.py', self.registry.reader_paths)

    def test_audit_detects_an_adapter_that_echoes_the_observed_wrong_value(self):
        capture = self.make()
        rule, = [rule for rule in self.registry.rules if rule.identifier == kd.ABSENCE_KD]
        broad = dataclasses.replace(rule, expected=lambda facts, item: kd.Pair(item.reference, item.candidate))
        registry = kd.Registry((broad,), {'isobmff': kd.iso_facts})
        audit = kd.audit(capture, self.media, registry, 'isobmff')
        self.assertFalse(audit['passed'])
        self.assertTrue(any(item['verdict'] == 'covered' for item in audit['attacks']))


if __name__ == '__main__':
    unittest.main()
