"""tools/compat-corpus/commands-0.1.0/commands.json: the 0.1.0 command set stays
what tools/compat-corpus/release_set.py derives from commands-v1, and
docs/design/compat-cli.md ("The 0.1.0 command set") quotes the set it derives."""
import importlib.util
import json
import pathlib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
HERE = ROOT / 'tools' / 'compat-corpus'
SPEC = importlib.util.spec_from_file_location('release_set', HERE / 'release_set.py')
release_set = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release_set)
NOTE = ROOT / 'docs' / 'design' / 'compat-cli.md'


class Commands010(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        records = [json.loads(line) for line in
                   release_set.COMMANDS.read_text(encoding='utf-8').splitlines()]
        cls.records = records
        cls.derived = release_set.derive(records)
        cls.committed = json.loads(release_set.OUT.read_text(encoding='utf-8'))
        cls.note = ' '.join(NOTE.read_text(encoding='utf-8').split())

    def test_the_committed_set_is_what_the_script_derives(self):
        self.assertEqual(self.committed, json.loads(json.dumps(self.derived)),
                         'run tools/compat-corpus/release_set.py and commit commands-0.1.0/commands.json')

    def test_every_entry_is_made_of_native_commands(self):
        native = {tuple(r['args']) for r in self.records if r['label'] == 'native'}
        for entry in self.derived['entries']:
            for command in entry['commands']:
                self.assertIn(tuple(command['args']), native, entry['id'])

    def test_the_cut_takes_the_fewest_shapes_and_whole_ties(self):
        entries = self.derived['entries']
        eligible = self.derived['eligible']['pairs']
        before_last_tier = sum(e['pairs'] for e in entries if e['pairs'] > entries[-1]['pairs'])
        self.assertLess(before_last_tier, release_set.COVERAGE * eligible)
        self.assertGreaterEqual(self.derived['selected']['pairs'], release_set.COVERAGE * eligible)
        for shape in self.derived['not_selected']:
            self.assertLess(shape['pairs'], entries[-1]['pairs'])

    def first_over_coverage(self):
        target, total = release_set.COVERAGE * self.derived['eligible']['pairs'], 0
        for n, entry in enumerate(self.derived['entries'], 1):
            total += entry['pairs']
            if total >= target:
                return n
        return None

    def test_the_note_quotes_the_derived_numbers(self):
        w, eligible, native = (self.derived['selected'], self.derived['eligible'],
                               self.derived['native'])
        expected = [
            f'{w["shapes"]} shapes',
            f'{w["commands"]} of the {eligible["commands"]} eligible native commands',
            f'{w["pairs"]} of their {eligible["pairs"]} command-and-answer pairs '
            f'({w["share_of_eligible_pairs"]:.1%})',
            f'{w["share_of_native_pairs"]:.1%} of all {native["pairs"]} native pairs',
            f'{release_set.COVERAGE:.0%} of the eligible',
            f'They hold {native["pairs"] - eligible["pairs"]} of the {native["pairs"]}',
            f'The {len(self.derived["not_selected"])} shapes below the cut',
            f'would have made {self.first_over_coverage()} and left out',
        ]
        for text in expected:
            self.assertIn(text, self.note, 'docs/design/compat-cli.md does not quote commands-0.1.0')

    def test_the_note_tables_every_entry_as_derived(self):
        """Each row whole: the family, the pair count and the shape as well as
        the identifier and the example, in the derived order."""
        rows = release_set.table(self.derived).split('\n')
        for row in rows:
            self.assertIn(' '.join(row.split()), self.note,
                          'docs/design/compat-cli.md: paste the output of release_set.py --table')
        self.assertIn(' '.join(' '.join(rows).split()), self.note, 'the rows are out of order')


if __name__ == '__main__':
    unittest.main()
