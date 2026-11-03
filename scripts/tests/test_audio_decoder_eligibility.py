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
        for field, value in [('codec', 'future'), ('kind', 'tolerance'),
                             ('decoder_eligible', False), ('decoder_eligible', 1),
                             ('evidence', '../outside.md#evidence')]:
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

    def test_opus_presentation_cannot_be_enabled_by_its_vector_pass(self):
        opus = next(row for row in self.record['decoders'] if row['codec'] == 'opus')
        self.assertTrue(opus['decoder_eligible'])
        self.assertTrue(all(not row['eligible'] for row in opus['presentations']))
        opus['presentations'][0].update(eligible=True, requirements=['supported_mp4_timeline'])
        with self.assertRaisesRegex(ValueError, 'profile predicate'):
            eligibility.validate(self.record)

    def test_record_changes_start_all_three_conformance_jobs(self):
        workflow = (eligibility.ROOT / '.github/workflows/audio-conformance.yml').read_text()
        for event in ('push:', 'pull_request:'):
            block = workflow.split('  ' + event, 1)[1].split('  schedule:', 1)[0]
            block = block.split('  pull_request:', 1)[0]
            self.assertIn('"docs/compat/audio-decoder-eligibility.json"', block)
        self.assertIn('python3 -I scripts/audio_decoder_eligibility.py', workflow)
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
