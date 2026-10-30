"""tools/compat-corpus/commands-0.1.0/coverage.json: the encode breadth set stays
what tools/compat-corpus/coverage_set.py derives from commands-v1 and
commands-v1/outputs.json, and docs/design/compat-cli.md ("The 0.1.0 codec
coverage bars") quotes the numbers it derives."""
import importlib.util
import json
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
HERE = ROOT / 'tools' / 'compat-corpus'
SPEC = importlib.util.spec_from_file_location('coverage_set', HERE / 'coverage_set.py')
coverage_set = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(coverage_set)
NOTE = ROOT / 'docs' / 'design' / 'compat-cli.md'
# What an answer's output may become in the committed file: never a name.
TEMPLATE = re.compile(r'\A(OUTPUT(%\d*d)?(\.[a-z0-9]+)?|-|pipe:\d*|)\Z')


class CoverageSet(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.records = [json.loads(line) for line in
                       coverage_set.COMMANDS.read_text(encoding='utf-8').splitlines()]
        cls.outputs = json.loads(coverage_set.OUTPUTS.read_text(encoding='utf-8'))
        cls.derived = coverage_set.derive(cls.records, cls.outputs)
        cls.committed = json.loads(coverage_set.OUT.read_text(encoding='utf-8'))
        cls.note = ' '.join(NOTE.read_text(encoding='utf-8').split())

    def test_the_committed_set_is_what_the_script_derives(self):
        self.assertEqual(self.committed, json.loads(json.dumps(self.derived)),
                         'run tools/compat-corpus/coverage_set.py and commit commands-0.1.0/coverage.json')

    def test_the_outputs_are_those_of_every_usable_ffmpeg_command(self):
        usable = [r['args'] for r in self.records
                  if r['tool'] == 'ffmpeg' and r['label'] != 'unusable']
        self.assertEqual([c['args'] for c in self.outputs['commands']], usable)

    def test_no_output_keeps_a_name_from_an_answer(self):
        for command in self.outputs['commands']:
            for output in command['outputs']:
                self.assertRegex(output, TEMPLATE, command['args'])

    def test_the_note_quotes_the_derived_numbers(self):
        d, classes = self.derived, self.derived['classes']
        media = classes['media']
        expected = [
            f'at least **40%** of the {media} commands of the encode set',
            f'{d["filter_free"]} of {d["usable"]}, and of those the {media} whose output',
            f'The other {d["filter_free"] - media} are {classes["unknown"]} whose container',
            f'{classes["decode-only"]} that only decode',
            f'{classes["subtitles"]} subtitle files and {classes["other"]} other outputs',
            f'read on {d["outputs_read"]}',
            f'The share of the {media},',
            f'over the {d["filter_free"]} filter-free commands',
            f'Reported as a share of {d["filter_free"]},',
        ]
        for text in expected:
            self.assertIn(text, self.note, 'docs/design/compat-cli.md does not quote coverage.json')


if __name__ == '__main__':
    unittest.main()
