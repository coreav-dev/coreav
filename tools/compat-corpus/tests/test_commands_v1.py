"""tools/compat-corpus/commands-v1/commands.jsonl: the committed set stays what the
rules say it is (commands-v1/README.md, docs/design/compat-cli.md)."""
import collections
import importlib.util
import json
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    'classify', ROOT / 'tools' / 'compat-corpus' / 'classify.py')
classify = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(classify)
SET = ROOT / 'tools' / 'compat-corpus' / 'commands-v1' / 'commands.jsonl'
NOTE = ROOT / 'docs' / 'design' / 'compat-cli.md'
PAGE = ROOT / 'tools' / 'compat-corpus' / 'commands-v1' / 'README.md'
FIELDS = {'tool', 'args', 'label', 'reason', 'sources'}
SOURCE_FIELDS = {'question', 'answer', 'link', 'score', 'license', 'tag'}
# B5: every option this common is implemented or refused by design, never
# missing from both (docs/design/compat-cli.md, "The bars").
VOCABULARY_BAR = 10
# The two floors of the same section, which only guard against the
# vocabulary narrowing later.
FLOORS = {'ffprobe': 0.60, 'ffmpeg': 0.07}
# A file name from a post must never reach the committed set.
PLACEHOLDER = re.compile(r'\A(INPUT|OUTPUT|FILE)\d*\Z')


def vocabulary():
    return (classify.FFPROBE_VALUE | classify.FFPROBE_FLAG | classify.FFMPEG_VALUE
            | classify.FFMPEG_FLAG | set(classify.FFMPEG_CODEC))


class CommandsV1(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.records = [json.loads(line) for line in SET.read_text(encoding='utf-8').splitlines()]
        cls.note = NOTE.read_text(encoding='utf-8')
        cls.page = PAGE.read_text(encoding='utf-8')

    def options(self, tool):
        counts = collections.Counter()
        for record in self.records:
            if record['tool'] != tool:
                continue
            for token in record['args']:
                if classify.is_option(token):
                    counts[token.split('=', 1)[0]] += 1
        return counts

    def named_in_the_note(self, option):
        base = option.split(':', 1)[0]
        return f'`{option}`' in self.note or f'`{base}:*`' in self.note

    def test_every_record_has_every_field(self):
        for record in self.records:
            self.assertEqual(set(record), FIELDS, record['args'])
            self.assertIn(record['tool'], ('ffmpeg', 'ffprobe'))
            self.assertTrue(record['args'])
            self.assertTrue(record['sources'])
            for source in record['sources']:
                self.assertEqual(set(source), SOURCE_FIELDS, source)

    def test_every_label_is_what_classify_says(self):
        for record in self.records:
            label, reason = classify.label(record['tool'], record['args'])
            self.assertEqual((label, reason), (record['label'], record['reason']),
                             f'{record["sources"][0]["link"]}: {" ".join(record["args"])}')

    def test_the_committed_args_are_already_normalised(self):
        """Every file name was replaced, and replacing again changes
        nothing: the committed record is the shape, not a post's names."""
        for record in self.records:
            self.assertEqual(classify.normalize(record['tool'], record['args']),
                             record['args'], record['sources'][0]['link'])
            options, bare = classify.walk(
                record['args'], classify.value_options(record['tool']))
            # The two positions `normalize` replaces: every `-i` value and
            # the last bare argument, FFmpeg's output and ffprobe's input.
            for name in [v for o, v in options if o == '-i' and v] + bare[-1:]:
                self.assertTrue(PLACEHOLDER.match(name),
                                f'{record["sources"][0]["link"]}: {name}')

    def test_no_home_directory_names_a_person(self):
        for record in self.records:
            for token in record['args']:
                self.assertEqual(classify.scrub(token), token,
                                 f'{record["sources"][0]["link"]}: {token}')

    def test_every_source_is_attributed_and_licensed(self):
        for record in self.records:
            for source in record['sources']:
                self.assertTrue(source['license'].startswith('CC BY-SA'), source)
                self.assertEqual(source['link'],
                                 f'https://stackoverflow.com/a/{source["answer"]}')
            answers = [s['answer'] for s in record['sources']]
            self.assertEqual(len(answers), len(set(answers)))

    def test_the_set_is_distinct(self):
        keys = [(r['tool'], tuple(r['args'])) for r in self.records]
        self.assertEqual(len(keys), len(set(keys)))

    def test_every_common_option_is_implemented_or_refused_by_design(self):
        """B5. An option in 10 or more commands that the note names
        nowhere would be silently ignored by the command."""
        known = vocabulary()
        missing = []
        for tool in ('ffprobe', 'ffmpeg'):
            for option, count in self.options(tool).items():
                if count < VOCABULARY_BAR:
                    continue
                if option.split(':', 1)[0] in known or option in known:
                    continue
                if self.named_in_the_note(option):
                    continue
                missing.append(f'{option} ({count})')
        self.assertEqual(sorted(missing), [],
                         'name each in docs/design/compat-cli.md, implemented or refused by design')

    def test_the_note_implements_exactly_what_classify_implements(self):
        """B5, the other direction: the pre-registered vocabulary table and
        the code that labels the set may not drift apart."""
        for option in sorted(vocabulary()):
            self.assertTrue(self.named_in_the_note(option),
                            f'{option} is in classify.py but in no table of {NOTE.name}')

    def test_the_readme_counts_the_set_it_describes(self):
        """A number in a page comes from a committed measurement. These come from the committed file."""
        pairs = sum(len(r['sources']) for r in self.records)
        licences = collections.Counter(s['license'] for r in self.records
                                       for s in r['sources'])
        expected = [
            f'{pairs:,} command-and-answer pairs',
            f'{licences["CC BY-SA 4.0"]:,} of the {pairs:,}',
            f'{licences["CC BY-SA 3.0"]:,} under 3.0',
            f'{licences["CC BY-SA 2.5"]:,} under',
            f'| Commands | {len(self.records):,} distinct option sequences |',
        ]
        for tool, name in (('ffprobe', '`ffprobe`'), ('ffmpeg', '`ffmpeg`')):
            sub = [r for r in self.records if r['tool'] == tool]
            counts = collections.Counter(r['label'] for r in sub)
            expected.append(f'| {name} | {len(sub):,} commands: {counts["native"]:,} native, '
                            f'{counts["out-of-scope"]:,} out of scope, '
                            f'{counts["unusable"]:,} unusable |')
        for text in expected:
            self.assertIn(text, self.page, 'the README does not count the committed set')

    def test_every_value_the_vocabulary_allows_is_in_the_note(self):
        """B5 covers option names; the values that also decide `native` —
        the writers, the sections, the demuxers and the muxers — have to
        be in the note's tables too, or the pre-registered vocabulary is
        not the one the classifier applies."""
        for value in sorted(classify.FFPROBE_WRITERS | classify.FFPROBE_SECTIONS
                            | classify.FFMPEG_DEMUXERS | classify.FFMPEG_MUXERS):
            self.assertIn(f'`{value}`', self.note,
                          f'{value} decides a label but is in no table of {NOTE.name}')

    def test_the_native_share_has_not_fallen(self):
        for tool, floor in FLOORS.items():
            sub = [r for r in self.records if r['tool'] == tool]
            usable = [r for r in sub if r['label'] != 'unusable']
            share = sum(r['label'] == 'native' for r in usable) / len(usable)
            self.assertGreaterEqual(round(share, 4), floor, tool)


if __name__ == '__main__':
    unittest.main()
