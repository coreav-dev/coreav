"""Exact TS exclusions using the landed records and generated media bytes."""
import copy
import importlib.util
import pathlib
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))
import corpus_kd as kd

SPEC = importlib.util.spec_from_file_location(
    'corpus_ts_fixtures', kd.ROOT / 'tools/oracle/tests/test_kd_ts.py')
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)


class TransportStreams(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = pathlib.Path(temporary.name)
        self.media = self.root / 'generated.ts'
        self.reference_packets = self.root / 'reference.packets'
        self.candidate_packets = self.root / 'candidate.packets'
        packet = 'packet|stream_index=0|pts=90000|size=16\n'
        self.reference_packets.write_text(packet)
        self.candidate_packets.write_text(packet)
        self.registry = kd.production_registry()

    def capture(self, reference, candidate):
        return kd.Capture({'streams': copy.deepcopy(reference)}, {'streams': copy.deepcopy(candidate)},
                          self.reference_packets, self.candidate_packets)

    def vbi(self, descriptor=True, later=0x99):
        mux = fixtures.Mux()
        tags = b'\x45\x06\x04\x01\xf0\x05\x01\xf7' if descriptor else b''
        mux.tables([(0x06, 0x101, tags)])
        for i in range(3):
            mux.pes(0x101, 90000 + 3600 * i, bytes([0x99 if not i else later]) + bytes(138))
        self.media.write_bytes(mux.out)
        common = {'index': 0, 'id': '0x101', 'time_base': '1/90000'}
        return self.capture([dict(common, codec_type='audio', codec_name='mp3', sample_rate='0', channels=0)],
                            [dict(common, codec_type='data', codec_name='bin_data')])

    def wrap(self, programs=2, first=5_479_446_912):
        mux = fixtures.Mux()
        mux.programs({number: (0x1000 + number, [(0x02, 0x100 + number)])
                      for number in range(1, programs + 1)})
        for i in range(3):
            for number in range(1, programs + 1):
                mux.pes(0x100 + number, first + 3600 * i, bytes(100))
        self.media.write_bytes(mux.out)
        common = {'index': 0, 'id': '0x101', 'time_base': '1/90000'}
        reference = dict(common, start_pts=first + kd.oracle_kd_ts.WRAP,
                         start_time=kd.oracle_kd_ts.seconds(first + kd.oracle_kd_ts.WRAP))
        candidate = dict(common, start_pts=first, start_time=kd.oracle_kd_ts.seconds(first))
        return self.capture([reference], [candidate])

    def downmix(self, corrupt=False):
        frame = fixtures.ac3_annex_d()
        stated = kd.oracle_kd_ts.annex_d_downmix(frame)
        if corrupt:
            frame = frame[:20] + bytes([frame[20] ^ 1]) + frame[21:]
        mux = fixtures.Mux()
        mux.tables([(0x06, 0x101, b'\x6a\x01\x00')])
        mux.pes(0x101, 90000, frame)
        self.media.write_bytes(mux.out)
        common = {'index': 0, 'id': '0x101', 'codec_name': 'ac3'}
        absent = {key: '0' if key == 'dmix_mode' else '0.000000' for key in stated}
        return self.capture([dict(common, **absent)], [dict(common, **stated)])

    def compare(self, capture):
        return kd.compare(capture, self.media, self.registry)

    def test_registry_scopes_are_finite_and_match_the_landed_records(self):
        records = self.registry.records()
        self.assertEqual([record['id'] for record in records],
                         ['KD-TS-VBI-AS-AUDIO', 'KD-TS-PROGRAM-WRAP', 'KD-TS-ANNEX-D-DOWNMIX'])
        for record in records:
            self.assertTrue(record['requires_pid'])
            self.assertFalse(record['permits_absence'])
            self.assertLessEqual(set(record['keys']), kd.authorized_keys(record['record']))
        self.assertEqual(records[0]['keys'], ['streams.codec_name'])

    def test_vbi_does_not_hide_type_packets_or_missing_audio_values(self):
        capture = self.vbi()
        self.candidate_packets.write_text('packet|stream_index=0|pts=90001|size=16\n')
        result = self.compare(capture)
        self.assertEqual(result['excluded'], {'streams.codec_name': ['KD-TS-VBI-AS-AUDIO']})
        for key in ('streams.codec_name', 'streams.codec_type', 'streams.sample_rate',
                    'streams.channels', 'packets.pts'):
            self.assertFalse(result['keys'][key])

    def test_vbi_requires_the_descriptor_and_every_payload_identifier(self):
        for descriptor, later in ((False, 0x99), (True, 0xff)):
            with self.subTest(descriptor=descriptor, later=later):
                self.assertEqual(self.compare(self.vbi(descriptor, later))['excluded'], {})

    def test_vbi_requires_the_exact_candidate_classification(self):
        capture = self.vbi()
        capture.candidate['streams'][0]['codec_name'] = 'h264'
        self.assertEqual(self.compare(capture)['excluded'], {})

    def test_program_clock_proves_both_exact_start_values(self):
        result = self.compare(self.wrap())
        self.assertEqual(result['excluded'], {
            'streams.start_pts': ['KD-TS-PROGRAM-WRAP'],
            'streams.start_time': ['KD-TS-PROGRAM-WRAP']})
        self.assertFalse(result['keys']['streams.start_pts'])

    def test_wrap_requires_multiple_programs_and_an_interior_clock(self):
        for programs, first in ((1, 5_479_446_912), (2, kd.oracle_kd_ts.WRAP - 1000)):
            with self.subTest(programs=programs, first=first):
                self.assertEqual(self.compare(self.wrap(programs, first))['excluded'], {})

    def test_wrap_refuses_any_missing_null_or_changed_required_value(self):
        for side in ('reference', 'candidate'):
            for field in ('start_pts', 'start_time', 'time_base'):
                for value in ('absent', None, 'wrong'):
                    with self.subTest(side=side, field=field, value=value):
                        capture = self.wrap()
                        stream = getattr(capture, side)['streams'][0]
                        if value == 'absent':
                            del stream[field]
                        else:
                            stream[field] = value
                        self.assertEqual(self.compare(capture)['excluded'], {})

    def test_downmix_uses_all_five_exact_byte_derived_values(self):
        result = self.compare(self.downmix())
        self.assertEqual(result['excluded'], {'streams.' + key: ['KD-TS-ANNEX-D-DOWNMIX']
                                             for key in kd.oracle_kd_ts.DOWNMIX_KEYS})

    def test_downmix_refuses_corrupt_crc_and_changed_or_missing_values(self):
        self.assertEqual(self.compare(self.downmix(corrupt=True))['excluded'], {})
        for side in ('reference', 'candidate'):
            for key in kd.oracle_kd_ts.DOWNMIX_KEYS:
                for value in ('absent', None, '9'):
                    with self.subTest(side=side, key=key, value=value):
                        capture = self.downmix()
                        stream = getattr(capture, side)['streams'][0]
                        if value == 'absent':
                            del stream[key]
                        else:
                            stream[key] = value
                        self.assertEqual(self.compare(capture)['excluded'], {})

    def test_a_later_uncovered_occurrence_keeps_each_key_row(self):
        for make in (self.vbi, self.wrap, self.downmix):
            with self.subTest(case=make.__name__):
                capture = make()
                before = self.compare(capture)['excluded']
                reference = copy.deepcopy(capture.reference['streams'][0])
                candidate = copy.deepcopy(capture.candidate['streams'][0])
                reference['index'] = candidate['index'] = 1
                candidate['id'] = '0x102'
                capture.reference['streams'].append(reference)
                capture.candidate['streams'].append(candidate)
                result = self.compare(capture)
                self.assertTrue(before)
                self.assertEqual(result['excluded'], {})
                for key in before:
                    self.assertEqual(result['differences'][key], 2)
                    self.assertEqual(result['validated'][key], 1)

    def test_no_pid_search_or_reordering_is_used(self):
        for make in (self.vbi, self.wrap, self.downmix):
            with self.subTest(case=make.__name__):
                capture = make()
                reference = copy.deepcopy(capture.reference['streams'][0])
                candidate = copy.deepcopy(capture.candidate['streams'][0])
                reference.update(index=1, id='0x102')
                candidate.update(index=1, id='0x102')
                capture.reference['streams'].append(reference)
                capture.candidate['streams'].append(candidate)
                capture.candidate['streams'].reverse()
                result = self.compare(capture)
                self.assertEqual(result['excluded'], {})
                self.assertEqual(result['validated'], {})

    def test_program_nested_copies_are_not_in_the_registered_scope(self):
        capture = self.wrap()
        capture.reference['programs'] = [{'streams': copy.deepcopy(capture.reference['streams'])}]
        capture.candidate['programs'] = [{'streams': copy.deepcopy(capture.candidate['streams'])}]
        result = self.compare(capture)
        for field in ('start_pts', 'start_time'):
            self.assertIn('streams.' + field, result['excluded'])
            self.assertFalse(result['keys']['programs.streams.' + field])
            self.assertNotIn('programs.streams.' + field, result['excluded'])

    def test_direct_agreement_and_unreadable_evidence_are_not_exclusions(self):
        for make in (self.vbi, self.wrap, self.downmix):
            with self.subTest(case=make.__name__):
                capture = make()
                capture.candidate = copy.deepcopy(capture.reference)
                self.assertEqual(self.compare(capture)['excluded'], {})
                capture = make()
                self.media.write_bytes(b'not transport stream bytes')
                self.assertEqual(self.compare(capture)['excluded'], {})

    def test_every_used_exclusion_survives_candidate_mutation_audit(self):
        for make in (self.vbi, self.wrap, self.downmix):
            with self.subTest(case=make.__name__):
                capture = make()
                result = kd.audit(capture, self.media, self.registry, kd.mutations(capture))
                self.assertTrue(result['passed'], result)


if __name__ == '__main__':
    unittest.main()
