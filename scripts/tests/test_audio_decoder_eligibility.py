import copy
import importlib.util
import json
from pathlib import Path
import re
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / 'audio_decoder_eligibility.py'
SPEC = importlib.util.spec_from_file_location('audio_decoder_eligibility', SCRIPT)
eligibility = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(eligibility)


class DecoderEligibility(unittest.TestCase):
    def setUp(self):
        self.record = json.loads(eligibility.RECORD.read_text())

    def test_checked_in_record_and_empty_registry_are_valid(self):
        eligibility.validate(self.record)
        self.record['decoders'] = []
        eligibility.validate(self.record)

    def test_missing_unknown_duplicate_and_false_admission_are_rejected(self):
        def check(change):
            record = copy.deepcopy(self.record)
            change(record)
            with self.assertRaises(ValueError):
                eligibility.validate(record)

        check(lambda r: r.pop('schema'))
        check(lambda r: r.update(schema='future'))
        check(lambda r: r.update(oracle_version='unregistered'))
        check(lambda r: r['decoders'].append(copy.deepcopy(r['decoders'][0])))
        for field, value in [('codec', 'future'), ('kind', 'tolerance'), ('kind', 'exact'),
                             ('decoder_eligible', False), ('decoder_eligible', 1),
                             ('evidence', '../outside.md#evidence'),
                             ('evidence', 'docs/design/lossy-pcm-eligibility.md#missing-section')]:
            with self.subTest(field=field, value=value):
                check(lambda r: r['decoders'][0].update({field: value}))
        for field, value in [('container', 'future'), ('eligible', 1),
                             ('requirements', []), ('requirements', ['future']),
                             ('requirements', ['supported_mp4_timeline']),
                             ('requirements', ['mpeg1_layer3'])]:
            with self.subTest(field=field, value=value):
                check(lambda r: r['decoders'][0]['presentations'][0].update({field: value}))
        check(lambda r: r['decoders'][0]['presentations'].append(
            copy.deepcopy(r['decoders'][0]['presentations'][0])))

    def test_evidence_anchors_ignore_fences_and_number_duplicate_headings(self):
        self.assertEqual(eligibility.heading_anchors(
            '# AAC-LC\n```\n## Not a heading\n```\n## AAC-LC\n'), {'aac-lc', 'aac-lc-1'})

    def test_fences_close_only_with_the_same_character_and_sufficient_length(self):
        for opening, interior, closing in [('````', '```', '`````'),
                                            ('~~~', '```', '~~~'),
                                            ('```', '~~~', '```')]:
            with self.subTest(opening=opening, interior=interior):
                markdown = f'{opening}text\n{interior}\n## Hidden\n{closing}\n## Visible\n'
                self.assertEqual(eligibility.heading_anchors(markdown), {'visible'})
        self.assertEqual(eligibility.heading_anchors(
            '  ````\n````not a closer\n## Hidden\n  ````\n  ## Visible\n'), {'visible'})

    def test_list_fences_and_indented_code_cannot_supply_evidence_headings(self):
        self.assertEqual(eligibility.heading_anchors(
            '- ~~~\n  ## Example\n  ~~~\n\n## Visible\n'), {'visible'})
        self.assertEqual(eligibility.heading_anchors(
            '    ## Example\n\n## Visible\n'), {'visible'})
        self.assertEqual(eligibility.heading_anchors(
            'Visible [label](https://example.com)\n===\n'), {'visible-label'})

    def test_opus_presentation_cannot_be_enabled_by_its_vector_pass(self):
        opus = next(row for row in self.record['decoders'] if row['codec'] == 'opus')
        self.assertTrue(opus['decoder_eligible'])
        self.assertFalse(opus['presentations'][1]['eligible'])
        opus['presentations'][0].update(eligible=True, requirements=['supported_mp4_timeline'])
        with self.assertRaisesRegex(ValueError, 'profile predicate'):
            eligibility.validate(self.record)

    def test_aac_project_comparator_cannot_grant_iso_eligibility(self):
        aac = next(row for row in self.record['decoders'] if row['codec'] == 'aac')
        aac['kind'] = 'conformance'
        aac.pop('tolerance')
        aac['decoder_eligible'] = True
        with self.assertRaisesRegex(ValueError, 'ISO conformance procedure'):
            eligibility.validate(self.record)

    def test_aac_tolerance_cannot_grant_exactness_or_relax_its_scope(self):
        for field, value in [('f32_absolute', 2 ** -15), ('s16_lsb', 2),
                             ('timeline', 'approximate'), ('feature', 'default')]:
            with self.subTest(field=field):
                record = copy.deepcopy(self.record)
                aac = next(row for row in record['decoders'] if row['codec'] == 'aac')
                aac['tolerance'][field] = value
                with self.assertRaisesRegex(ValueError, 'tolerance or build scope'):
                    eligibility.validate(record)
        aac = next(row for row in self.record['decoders'] if row['codec'] == 'aac')
        for requirements in ([], ['aac_lc'], ['aac_lc', 'supported_mp4_timeline']):
            aac['presentations'][0]['requirements'] = requirements
            with self.assertRaisesRegex(ValueError, 'content and presentation predicates'):
                eligibility.validate(self.record)

    def test_opus_scope_needs_both_registered_profile_and_mp4_timeline(self):
        opus = next(row for row in self.record['decoders'] if row['codec'] == 'opus')
        opus['presentations'][0].update(
            eligible=True, requirements=['opus_mapping0', 'supported_mp4_timeline'])
        eligibility.validate(self.record)
        opus['presentations'][0]['requirements'] = ['opus_mapping0']
        with self.assertRaisesRegex(ValueError, 'timeline predicate'):
            eligibility.validate(self.record)
        opus['presentations'][0]['requirements'] = ['future', 'supported_mp4_timeline']
        with self.assertRaisesRegex(ValueError, 'unknown requirement'):
            eligibility.validate(self.record)

    def test_record_changes_start_all_three_conformance_jobs(self):
        workflow = (eligibility.ROOT / '.github/workflows/audio-conformance.yml').read_text()
        for event in ('push:', 'pull_request:'):
            block = workflow.split('  ' + event, 1)[1].split('  schedule:', 1)[0]
            block = block.split('  pull_request:', 1)[0]
            self.assertIn('"docs/compat/audio-decoder-eligibility.json"', block)
        self.assertIn('python3 -I scripts/audio_decoder_eligibility.py', workflow)
        self.assertEqual(workflow.count('"scripts/pr-text-requirements.txt"'), 2)
        self.assertIn('--require-hashes -r scripts/pr-text-requirements.txt', workflow)
        for job in ('mp3', 'opus', 'aac'):
            block = re.split(r'\n  [a-z][a-z0-9-]*:',
                             workflow.split('\n  ' + job + ':', 1)[1], maxsplit=1)[0]
            self.assertIn("if: (github.event_name != 'pull_request' || !github.event.pull_request.draft) && !inputs.swr_characterization && !inputs.raw_verification", block)
        for command in ('--test mp3 compliance_bitstreams',
                        '-p coreav-opus -- --ignored',
                        '--test conformance conformance_streams'):
            self.assertIn(command, workflow)


if __name__ == '__main__':
    unittest.main()
