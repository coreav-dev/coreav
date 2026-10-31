"""Only a frozen empty-edit padding record may certify CoreAV's absence."""
import copy
import dataclasses
import json
import pathlib
import sys
import tempfile
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))
import corpus_kd as kd


class Absence(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        self.media = self.root / 'bytes'
        self.media.write_bytes(b'\x14')
        self.reference, self.candidate = self.root / 'reference', self.root / 'candidate'
        self.first = 'packet|stream_index=0|pos=100|size=8|pts=0|side_data_list.discard_padding=0\n'
        self.last = 'packet|stream_index=0|pos=108|size=8|pts=100'
        self.reference.write_text(self.first + self.last + '|side_data_list.discard_padding=20\n')
        self.candidate.write_text(self.first + self.last + '\n')
        sections = {'streams': [{'index': 0, 'id': '0x1'}]}
        self.capture = kd.Capture(sections, copy.deepcopy(sections), self.reference, self.candidate)
        self.proof = kd.ProvenAbsence(20, 1, 108, 8, 200, 200)
        record = {'id': kd.ABSENCE_KD, 'basis': 'ISO/IEC 14496-12 8.6.6',
                  'level': ['packets'], 'fields': ['side_data_list.discard_padding'],
                  'corpus_absence': kd.ABSENCE_CASE}
        self.rule = kd.Rule(kd.ABSENCE_KD, 'tools/compat-corpus/ffmpeg9.json', record,
                            (kd.ABSENCE_KEY,), lambda facts, occurrence: self.proof if facts == b'\x14' else None)
        self.registry = kd.Registry((self.rule,), lambda path: path.read_bytes())

    def compare(self, registry=None, capture=None):
        return kd.compare(capture or self.capture, self.media, registry or self.registry)

    def test_positive_byte_certificate_excludes_only_the_padding_row_and_keeps_raw_result(self):
        result = self.compare()
        self.assertFalse(result['keys'][kd.ABSENCE_KEY])
        self.assertEqual(result['excluded'], {kd.ABSENCE_KEY: [kd.ABSENCE_KD]})
        self.assertTrue(result['keys']['packets.pts'])
        self.assertTrue(kd.audit(self.capture, self.media, self.registry)['passed'])

    def test_unproven_absence_and_an_ordinary_none_pair_remain_uncovered(self):
        for expected in (lambda facts, occurrence: None, lambda facts, occurrence: kd.Pair('20', None)):
            with self.subTest(expected=expected):
                registry = kd.Registry((dataclasses.replace(self.rule, expected=expected),), lambda p: p.read_bytes())
                self.assertEqual(self.compare(registry)['excluded'], {})
        self.media.write_bytes(b'\x15')
        self.assertEqual(self.compare()['excluded'], {})

    def test_other_kds_sources_keys_and_unregistered_case_markers_cannot_enable_absence(self):
        variants = [dataclasses.replace(self.rule, identifier='KD-OTHER', record={**self.rule.record, 'id': 'KD-OTHER'}),
                    dataclasses.replace(self.rule, document='docs/known-divergences.json'),
                    dataclasses.replace(self.rule, record={k: v for k, v in self.rule.record.items() if k != 'corpus_absence'}),
                    dataclasses.replace(self.rule, record={**self.rule.record, 'corpus_absence': 'another case'}),
                    dataclasses.replace(self.rule, keys=('packets.pts',), record={**self.rule.record, 'fields': ['pts']})]
        for rule in variants:
            with self.subTest(rule=rule):
                registry = kd.Registry((rule,), lambda p: p.read_bytes())
                self.assertEqual(self.compare(registry)['excluded'], {})

    def test_absence_authorization_cannot_be_added_after_the_plan_cutoff(self):
        (self.root / 'scripts').mkdir()
        (self.root / 'scripts/oracle-ffmpeg.env').write_bytes(b'pin')
        frozen_record = {key: value for key, value in self.rule.record.items() if key != 'corpus_absence'}
        def source(root, *args):
            if args[0] == 'merge-base':
                return b''
            if args[-1].endswith(self.rule.document):
                return json.dumps({'entries': [frozen_record]}).encode()
            if args[-1].endswith('scripts/oracle-ffmpeg.env'):
                return b'pin'
            return b'old plan' if '^:' in args[-1] else kd.AMENDMENT.encode()
        with mock.patch.object(kd, 'git', side_effect=source), self.assertRaises(ValueError):
            kd.inventory(self.root, 'a' * 40, self.registry, '0' * 64)

    def test_a_sample_past_the_edit_and_unbound_expected_values_are_not_certificates(self):
        for field, value in (('sample_end', 201), ('oracle', 21), ('track_id', 2), ('offset', 109),
                             ('size', 9), ('edit_end', 199), ('sample_end', -1), ('oracle', True)):
            with self.subTest(field=field, value=value):
                self.proof = dataclasses.replace(kd.ProvenAbsence(20, 1, 108, 8, 200, 200), **{field: value})
                self.assertEqual(self.compare()['excluded'], {})

    def test_every_other_packet_must_agree_even_on_a_different_key(self):
        self.candidate.write_text(self.first.replace('pts=0', 'pts=1') + self.last + '\n')
        result = self.compare()
        self.assertEqual(result['excluded'], {})
        self.assertFalse(result['keys']['packets.pts'])

    def test_packet_loss_duplication_or_a_later_packet_prevents_absence_coverage(self):
        for candidate in (self.first, self.first + self.last + '\n' + self.last + '\n',
                          self.first + self.last + '\npacket|stream_index=0|pos=116|size=8|pts=200\n'):
            with self.subTest(candidate=candidate):
                self.candidate.write_text(candidate)
                self.assertEqual(self.compare()['excluded'], {})

    def test_both_stream_identities_and_the_packet_size_and_offset_are_bound(self):
        for side in ('reference', 'candidate'):
            capture = copy.deepcopy(self.capture)
            getattr(capture, side)['streams'][0]['id'] = '0x2'
            self.assertEqual(self.compare(capture=capture)['excluded'], {})
        for field, value in (('pos=108', 'pos=109'), ('size=8', 'size=9'), ('stream_index=0', 'stream_index=1')):
            with self.subTest(field=field):
                self.candidate.write_text(self.first + self.last.replace(field, value) + '\n')
                self.assertEqual(self.compare()['excluded'], {})

    def test_absence_of_reference_or_explicit_null_still_never_qualifies(self):
        item = next(o for o in kd.occurrences(self.capture, permit_absence=True) if o.key == kd.ABSENCE_KEY and o.different)
        for changed in (dataclasses.replace(item, reference=None), dataclasses.replace(item, present=True, candidate=None)):
            self.assertEqual(self.registry.covers(changed, b'\x14'), frozenset())

    def test_failed_capture_never_qualifies_even_with_a_certificate(self):
        self.capture.packet_code = 2
        self.assertEqual(self.compare()['excluded'], {})

    def test_missing_fields_injected_without_byte_proof_are_caught(self):
        self.candidate.write_text(self.reference.read_text())
        self.media.write_bytes(b'\x15')
        def remove(i, fields):
            return ({key: value for key, value in fields.items() if key != 'side_data_list.discard_padding'},) if i == 1 else (fields,)
        audit = kd.audit(self.capture, self.media, self.registry, [kd.Attack('unproven missing field', packets=remove)])
        self.assertTrue(audit['passed'])
        self.assertEqual(audit['attacks'][0]['verdict'], 'caught')

    def test_bug_shaped_value_and_identity_mutations_stay_visible(self):
        attacks = []
        for key, value in (('pts', '101'), ('pos', '109'), ('size', '9'), ('stream_index', '1'),
                           ('side_data_list.discard_padding', '1')):
            def mutate(i, fields, key=key, value=value):
                return ({**fields, key: value},) if i == 1 else (fields,)
            attacks.append(kd.Attack(key + ' corrupt', packets=mutate))
        result = kd.audit(self.capture, self.media, self.registry, attacks)
        self.assertTrue(result['passed'])
        self.assertTrue(all(a['verdict'] == 'caught' for a in result['attacks']))

    def test_excluded_absence_is_published_separately(self):
        row = {'alias': 'f000001', 'container': 'isobmff', 'holdout': False, 'status': 'compared',
               'audit': kd.audit(self.capture, self.media, self.registry), **self.compare()}
        report = kd.report([row])
        self.assertEqual(report['excluded_rows'], [{'alias': 'f000001', 'container': 'isobmff', 'holdout': False,
                                                  'key': kd.ABSENCE_KEY, 'kds': [kd.ABSENCE_KD]}])
        self.assertEqual(report['groups']['all'][kd.ABSENCE_KEY]['direct'], 0)


if __name__ == '__main__':
    unittest.main()
