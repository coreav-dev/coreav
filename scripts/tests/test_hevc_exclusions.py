import copy
import pathlib
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import hevc_conformance as harness


class KnownExclusions(unittest.TestCase):
    def test_only_the_two_exact_named_streams_have_exclusions(self):
        exclusions = harness.known_exclusions()
        self.assertEqual(set(exclusions), {'SAODBLK_A', 'SAODBLK_B'})
        for test, item in exclusions.items():
            self.assertEqual(item['bitstream_sha256'], harness.EXCLUSION_IDENTITIES[test])
            self.assertEqual(item['decision'], 'excluded by maintainer decision, 2026-11-04')
            self.assertNotEqual(item['published_md5'], item['observed_oracle_md5'])
            self.assertTrue(item['investigation'])

    def test_another_stream_changed_bytes_missing_exclusion_or_decision_is_rejected(self):
        registered = dict(version=1, exclusions=harness.known_exclusions())
        changed = []
        data = copy.deepcopy(registered)
        data['exclusions']['OTHER'] = copy.deepcopy(data['exclusions']['SAODBLK_A'])
        changed.append(data)
        data = copy.deepcopy(registered)
        del data['exclusions']['SAODBLK_B']
        changed.append(data)
        for field, value in (('bitstream_sha256', '0' * 64), ('decision', 'excluded'),
                             ('investigation', ''), ('layout_registration_sha256_at_trial', '')):
            data = copy.deepcopy(registered)
            data['exclusions']['SAODBLK_A'][field] = value
            changed.append(data)
        for data in changed:
            with mock.patch.object(harness, 'read_json', return_value=data), \
                    self.subTest(data=data), self.assertRaises(ValueError):
                harness.known_exclusions()

    def test_excluded_streams_stay_in_accounting_without_passing_hash_or_count_credit(self):
        exclusions = harness.known_exclusions()
        records = [dict(test=test, status='accounted', streams=[dict(name=test + '.bin',
                        sha256=item['bitstream_sha256'], status='excluded', exclusion=item,
                        reason=item['decision'])]) for test, item in exclusions.items()]
        result = harness.census_result(records, 'r', 'p', {})
        self.assertEqual(len(result['archives']), 2)
        self.assertEqual(result['distinct_bitstreams'], 2)
        self.assertEqual(result['reference_count_methods'], {'maintainer_exclusion': 2})
        self.assertFalse(result['complete'])
        self.assertEqual(result['intra_candidates'], [])
        for row in result['reference_count_sources']:
            self.assertEqual(row['status'], 'excluded')
            self.assertIsNone(row['reference_count'])
            self.assertEqual(row['exclusion'], exclusions[row['test']])


if __name__ == '__main__':
    unittest.main()
