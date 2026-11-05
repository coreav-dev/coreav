import pathlib
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import hevc_conformance as harness
import hevc_oracle_input as preparation


def unit(kind, payload=b'\x80', prefix=b'\0\0\0\1'):
    return prefix + bytes([kind << 1, 1]) + payload


class OracleInputPreparation(unittest.TestCase):
    def test_stable_order_keeps_every_unit_byte_duplicates_and_vcl_suffix(self):
        units = [unit(kind, bytes([value, 0x80]), b'\0\0\1' if value in (3, 5) else b'\0\0\0\1')
                 for kind, value in ((34, 1), (32, 2), (33, 3), (34, 4), (32, 5), (33, 6))]
        suffix = unit(19) + unit(40) + b'\0\0'
        source = b'\0\0' + b''.join(units) + suffix
        result, proof = preparation.reorder(source, harness)
        self.assertEqual(result, b'\0\0' + b''.join(units[i] for i in (1, 4, 2, 5, 0, 3)) + suffix)
        self.assertEqual(proof['original_nal_indices_in_prepared_order'], [1, 4, 2, 5, 0, 3, 6, 7])
        self.assertEqual(proof['prefix_counts'], {'32': 2, '33': 2, '34': 2})
        self.assertEqual(proof['bytes'], len(source))
        self.assertNotEqual(proof['original_sha256'], proof['prepared_sha256'])
        self.assertTrue(proof['suffix_unchanged'])
        self.assertTrue(proof['every_nal_byte_preserved'])
        self.assertTrue(proof['within_type_order_preserved'])

    def test_nonparameter_prefix_late_parameter_missing_prefix_or_slice_stays_blocked(self):
        for source in (unit(35) + unit(32) + unit(19), unit(32) + unit(19) + unit(33),
                       unit(19), unit(32) + unit(33) + unit(34)):
            with self.subTest(source=source), self.assertRaises(ValueError):
                preparation.reorder(source, harness)

    def test_closed_stream_identity_and_description_are_checked_before_media(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            for case in (dict(test='another'), dict(test='VPSSPSPPS_A'),
                         dict(test='VPSSPSPPS_A', published_output_count=dict(value=5))):
                with self.subTest(case=case), self.assertRaisesRegex(ValueError, 'registered only'):
                    preparation.prepare(case, root / 'absent.bit', root, harness)
            self.assertEqual(list(root.iterdir()), [])

    def test_other_bytes_cannot_use_the_registered_recipe(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            path = root / 'source.bit'
            path.write_bytes(b'x' * 106928)
            case = dict(test='VPSSPSPPS_A', published_output_count=dict(value=6,
                        source='VPSSPSPPS_A_MainConcept_1.txt', source_sha256=preparation.DESCRIPTION_SHA256))
            with self.assertRaisesRegex(ValueError, 'original stream identity differs'):
                preparation.prepare(case, path, root, harness)
            self.assertEqual(list(root.iterdir()), [path])

    def test_prepared_diagnostics_retain_both_actual_input_identities(self):
        proof = dict(original_sha256='a' * 64, prepared_sha256='b' * 64)
        item = harness.stderr_line('[hevc @ 0x123] diagnostic\n', 'VPSSPSPPS_A', 'frames',
                                   exit_code=0, preparation=proof)
        self.assertEqual(item['original_input_sha256'], 'a' * 64)
        self.assertEqual(item['oracle_input_sha256'], 'b' * 64)
        self.assertEqual(item['classification'], 'unclassified')

    def test_another_stream_cannot_claim_a_preparation_proof(self):
        with self.assertRaisesRegex(ValueError, 'registered one-stream'):
            harness.classify_diagnostics(dict(test='RPS_C'), dict(sha256='a' * 64),
                dict(references=[]), {}, {}, dict(oracle_preparation={}), [], {})


if __name__ == '__main__':
    unittest.main()
