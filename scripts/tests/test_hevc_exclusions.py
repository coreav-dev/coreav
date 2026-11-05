import copy
import json
import pathlib
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import hevc_conformance as harness


class KnownExclusions(unittest.TestCase):
    def test_parser_documentation_requires_picture_index_on_every_vcl(self):
        text = (harness.ROOT / 'bench/hevc-conformance/README.md').read_text()
        contract = next(paragraph for paragraph in text.split('\n\n') if paragraph.startswith('Every VCL record'))
        self.assertIn('`picture_index`', contract.split('A first slice')[0])
        self.assertIn("All slices belonging to one coded picture carry that picture's index", contract)

    def test_preparation_document_reports_verified_output_without_rebinding_the_old_receipt(self):
        document = harness.ROOT / 'bench/hevc-conformance/oracle-preparation.md'
        text = document.read_text()
        row = next(line for line in text.splitlines() if line.startswith('| VPSSPSPPS_A |'))
        self.assertIn('Verified', row)
        self.assertIn('028ca00a6831b64faa226614252edd0290ce642d', row)
        self.assertNotIn('pending', row)
        self.assertIn(harness.hevc_oracle_input.PREPARED_SHA256, text)
        historical_sha = 'bee17edd278a1a75644fce4fab67aa6378182c7aa5764e2f09b412237347ab2d'
        self.assertIn(historical_sha, text)
        self.assertNotEqual(harness.digest(document), historical_sha)
        self.assertIn('Future preparation receipts recompute', text)

    def test_historical_trials_retain_full_mismatch_receipts_and_bind_normalized_paths(self):
        for test, item in harness.known_exclusions().items():
            history = item['historical_trial']
            self.assertEqual(history['kind'], 'historical')
            self.assertEqual(history['trial_head'], item['trial_head'])
            self.assertEqual(history['original_input_sha256'], item['bitstream_sha256'])
            self.assertEqual(history['oracle']['ffmpeg'],
                             '0d709a0bed1a43f24cd2c420878c63afea4580671b6949ad19ed124572f3bc3d')
            attempt, = history['reference_attempts']
            self.assertEqual(attempt['expected_md5'], item['published_md5'])
            self.assertEqual(attempt['observed_md5'], item['observed_oracle_md5'])
            self.assertEqual(attempt['layout_registration']['sha256'], item['layout_registration_sha256_at_trial'])
            self.assertEqual(attempt['command'], ['{oracle_ffmpeg}', '-v', 'repeat+error', '-noautorotate',
                '-flags', 'unaligned', '-f', 'hevc', '-c:v', 'hevc', '-threads:a', '1', '-i',
                '{original_bitstream}', '-map', '0:v:0', '-fps_mode', 'passthrough', '-noautoscale',
                '-c:v', 'rawvideo', '-pix_fmt', '+yuv420p', '-progress', 'pipe:2', '-f', 'md5', '-'])
            self.assertEqual(attempt['exit_code'], 0)
            self.assertTrue(attempt['progress_complete'])
            self.assertTrue(attempt['count_matches_frame_metadata'])
            self.assertEqual(attempt['frame_metadata_count'], 500)
            self.assertEqual(attempt['output_pictures'], 500)
            self.assertEqual(attempt['progress'][-1]['frame'], '500')
            self.assertEqual(attempt['progress'][-1]['progress'], 'end')
            self.assertEqual(attempt['status'], 'md5_mismatch')
            self.assertEqual(len(attempt['progress']), 3 if test == 'SAODBLK_A' else 4)

    def test_changed_historical_fields_or_conflicting_exclusion_metadata_are_rejected(self):
        registered = dict(version=1, exclusions=harness.known_exclusions())
        paths = [(['kind'], 'fresh'), (['trial_head'], '0' * 40),
                 (['original_input_sha256'], '0' * 64), (['oracle', 'ffmpeg'], '0' * 64),
                 (['reference_attempts', 0, 'command', 1], '-changed'),
                 (['reference_attempts', 0, 'exit_code'], 1),
                 (['reference_attempts', 0, 'progress_complete'], False),
                 (['reference_attempts', 0, 'output_pictures'], 499),
                 (['reference_attempts', 0, 'expected_md5'], '0' * 32),
                 (['reference_attempts', 0, 'observed_md5'], '0' * 32),
                 (['reference_attempts', 0, 'layout_registration', 'sha256'], '0' * 64),
                 (['reference_attempts', 0, 'progress', 0, 'fps'], 'changed')]
        for path, value in paths:
            data = copy.deepcopy(registered)
            target = data['exclusions']['SAODBLK_A']['historical_trial']
            for key in path[:-1]:
                target = target[key]
            target[path[-1]] = value
            with mock.patch.object(harness, 'read_json', return_value=data), \
                 self.subTest(path=path), self.assertRaisesRegex(ValueError, 'historical exclusion trial'):
                harness.known_exclusions()
        for field, value in (('published_md5', '0' * 32), ('observed_oracle_md5', '0' * 32),
                             ('trial_head', '0' * 40), ('layout_registration_sha256_at_trial', '0' * 64)):
            data = copy.deepcopy(registered)
            data['exclusions']['SAODBLK_A'][field] = value
            with mock.patch.object(harness, 'read_json', return_value=data), \
                 self.subTest(field=field), self.assertRaisesRegex(ValueError, 'historical exclusion trial'):
                harness.known_exclusions()

    def test_current_exclusion_run_copies_history_without_invoking_any_media_oracle(self):
        exclusions = harness.known_exclusions()
        item = exclusions['SAODBLK_A']
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source_dir = root / 'data' / 'SAODBLK_A'
            source_dir.mkdir(parents=True)
            source = source_dir / '000.bin'
            source.write_bytes(b'unit-test')
            inventory = dict(archive_sha256=item['archive_sha256'], descriptions=[],
                references=[dict(layout='whole_output_yuv_file', md5=item['published_md5'])],
                members=[dict(name='a.bin', path='000.bin', role='bitstream', bytes=9,
                              sha256=item['bitstream_sha256'])])
            case = dict(test='SAODBLK_A', archive='a.zip', archive_sha256=item['archive_sha256'],
                        extracted=harness.extracted_data(inventory))
            data = dict(archives=[case])
            acquisition = dict(complete=True, registration_identity_sha256=harness.registration_identity(data),
                archives=[dict(archive='a.zip', archive_sha256=item['archive_sha256'], inventory_sha256='d' * 64)])
            args = SimpleNamespace(registration=root / 'registration.json', data=root / 'data',
                                   oracle_bin=root / 'oracle', parser=root / 'parser', out=root / 'out')

            def identity(path):
                if path == source:
                    return item['bitstream_sha256']
                if path == args.data / 'a.zip':
                    return item['archive_sha256']
                return 'd' * 64

            with mock.patch.object(harness, 'registration', return_value=data), \
                 mock.patch.object(harness, 'known_exclusions', return_value=exclusions), \
                 mock.patch.object(harness, 'read_json', side_effect=[acquisition, inventory]), \
                 mock.patch.object(harness, 'digest', side_effect=identity), \
                 mock.patch.object(harness.test_oracle, 'verify_bin', return_value=[]), \
                 mock.patch.object(harness, 'child', return_value=(0, root / 'native', None)) as child, \
                 mock.patch.object(harness, 'native_rows', return_value=(dict(complete=True), [], [])), \
                 mock.patch.object(harness, 'probe') as probe, \
                 mock.patch.object(harness, 'confirm_reference') as confirm:
                with self.assertRaisesRegex(ValueError, 'G1 blocked'):
                    harness.run(args)
            child.assert_called_once()
            self.assertEqual(child.call_args.args[0][0], args.parser)
            probe.assert_not_called()
            confirm.assert_not_called()
            result = json.loads((args.out / 'census.json').read_text())
            record = result['archives'][0]['streams'][0]
            self.assertEqual(record['status'], 'excluded')
            self.assertEqual(record['exclusion']['historical_trial'], item['historical_trial'])
            self.assertNotIn('reference_attempts', record)
            self.assertNotIn('reference_count', record)
            self.assertEqual(result['reference_count_methods'], {'maintainer_exclusion': 1})

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
