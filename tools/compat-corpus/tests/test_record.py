"""tools/compat-corpus/record.py: docs/measurements/compat-ffprobe.md says what
its evidence, docs/measurements/compat-ffprobe/b3a-roundN.json, holds."""
import copy
import importlib.util
import json
import pathlib
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('record', ROOT / 'tools' / 'compat-corpus' / 'record.py')
record = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(record)

RECORD = ROOT / 'docs' / 'measurements' / 'compat-ffprobe.md'
EVIDENCE = ROOT / 'docs' / 'measurements' / 'compat-ffprobe'
NAME = 'docs/measurements/compat-ffprobe.md'


def check(text, evidence=EVIDENCE):
    return record.Record(text, evidence, NAME).check()


def replace_once(text, old, new):
    """`text` with `old` replaced by `new` where it occurs once after the
    heading of round 6, and the number of the line it starts on."""
    start = text.index('## Round 6\n')
    assert text.count(old, start) == 1, (old, text.count(old, start))
    at = text.index(old, start)
    return text[:at] + new + text[at + len(old):], text[:at].count('\n') + 1


class CommittedRecord(unittest.TestCase):
    def test_the_record_agrees_with_its_evidence(self):
        problems = check(RECORD.read_text())
        self.assertEqual(problems, [], '\n' + '\n'.join(problems)
                         + '\n(tools/compat-corpus/record.py rewrites the per-key blocks; '
                         'prose figures are fixed by hand)')

    def test_writing_the_blocks_changes_nothing(self):
        text = RECORD.read_text()
        self.assertEqual(record.Record(text, EVIDENCE, NAME).rendered(), text)

    def test_every_round_with_tables_is_rendered_from_its_evidence(self):
        blocks = record.Record(RECORD.read_text(), EVIDENCE, NAME).blocks
        self.assertEqual(sorted({n for n, _, _, _ in blocks}), [1, 3, 4, 5, 6])
        self.assertEqual(len(blocks), 15)


class PastMistakes(unittest.TestCase):
    """Each kind was committed once; each is now refused with the line,
    the round, the key and the evidence's value."""

    def test_a_prose_rate_that_contradicts_the_evidence(self):
        # Round 6 once quoted format.duration at 97.81%, the rate of the
        # stream duration beside it, where the JSON holds 1025/1049.
        text, line = replace_once(RECORD.read_text(), '`format.duration` (96.85% to 97.71%',
                                  '`format.duration` (96.85% to 97.81%')
        self.assertEqual(check(text), [
            f'{NAME}:{line}: round 6, ISO base media, `format.duration`: the text says 97.81%, '
            'b3a-round6.json has 1025/1049 = 97.71%'])

    def test_a_table_cell_that_contradicts_the_evidence(self):
        text, line = replace_once(
            RECORD.read_text(),
            '| `format.duration` | 1025/1049 | 97.71% | 729/746 (97.72%) | 296/303 (97.69%) |',
            '| `format.duration` | 1025/1049 | 97.71% | 729/746 (97.72%) | 297/303 (97.69%) |')
        problems = check(text)
        self.assertEqual(len(problems), 1, problems)
        self.assertTrue(problems[0].startswith(
            f'{NAME}:{line}: round 6, ISO base media, `format.duration`: the record has '), problems)
        self.assertIn('297/303', problems[0])
        self.assertIn('b3a-round6.json renders', problems[0])
        self.assertIn('296/303 (97.69%)', problems[0])

    def test_a_rate_rounded_onto_the_bar(self):
        # 1028/1049 is 97.998%; two decimals would print 98.00% for a key
        # below the bar.
        self.assertEqual(record.rate(1028, 1049), '97.998%')
        self.assertEqual(record.rate(1025, 1049), '97.71%')
        text, line = replace_once(RECORD.read_text(), '| `packets.pos` | 1028/1049 | 97.998% | 732/746',
                                  '| `packets.pos` | 1028/1049 | 98.00% | 732/746')
        problems = check(text)
        self.assertEqual(len(problems), 1, problems)
        self.assertTrue(problems[0].startswith(f'{NAME}:{line}: round 6, ISO base media, `packets.pos`'),
                        problems)

    def test_a_count_of_keys_below_the_bar(self):
        text, line = replace_once(RECORD.read_text(), 'MPEG-TS 35 of 157 (round 5: 65;',
                                  'MPEG-TS 36 of 157 (round 5: 65;')
        self.assertEqual(check(text), [
            f'{NAME}:{line}: round 6, MPEG-TS: the text says 36 of 157 keys below 98%, '
            'b3a-round6.json has 35 of 157'])

    def test_a_count_of_files(self):
        text, line = replace_once(RECORD.read_text(), '6 files as round 5', '7 files as round 5')
        self.assertEqual(check(text), [
            f'{NAME}:{line}: round 6: the text says 7 files `coreav ffprobe` rejects, '
            'b3a-round6.json has 6'])

    def test_a_figure_of_the_round_before(self):
        text, line = replace_once(RECORD.read_text(), '(90.61% to 98.18% of', '(90.16% to 98.18% of')
        self.assertEqual(check(text), [
            f'{NAME}:{line}: round 6, ISO base media, `streams.bit_rate`: the text says 90.16%, '
            'b3a-round5.json has 946/1044 = 90.61% (round 5)'])

    def test_a_key_table_outside_a_block(self):
        text, _ = replace_once(RECORD.read_text(), '## B6\n', '## B6\n\n| Key | Files | Agree | Rate |\n')
        problems = check(text)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn('a per-key table outside', problems[0])


class Evidence(unittest.TestCase):
    def test_a_flag_that_contradicts_its_counts(self):
        doc = json.loads((EVIDENCE / 'b3a-round6.json').read_text())
        bad = copy.deepcopy(doc)
        bad['isobmff']['packets.pos']['below_98'] = False
        self.assertEqual(record.consistency('b3a-round6.json', doc), [])
        self.assertEqual(record.consistency('b3a-round6.json', bad), [
            'b3a-round6.json: isobmff `packets.pos`: below_98 is False, but 1028/1049 is 97.998% '
            '(a key is below the bar on its unrounded share)'])

    def test_a_round_without_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            for p in EVIDENCE.glob('b3a-round[1-5].json'):
                (pathlib.Path(tmp) / p.name).write_bytes(p.read_bytes())
            problems = check(RECORD.read_text(), tmp)
        self.assertTrue(problems)
        self.assertTrue(all('round 6' in p and 'b3a-round6.json' in p for p in problems), problems)


if __name__ == '__main__':
    unittest.main()
