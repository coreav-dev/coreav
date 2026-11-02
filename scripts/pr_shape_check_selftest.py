#!/usr/bin/env python3
"""Tests of scripts/pr_shape_check.py, on titles and commits this
repository really had."""
import importlib.util
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('pr_shape_check', HERE / 'pr_shape_check.py')
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)
NOT_FORM = 'is not ' + chr(96) + 'type(scope): summary' + chr(96)


def log(*messages):
    return ''.join(f'{n:040x}'[::-1] + f'\x00{0:040x}\x00{m}\n\x00\n' for n, m in enumerate(messages, 1))


class Titles(unittest.TestCase):
    def test_merged_titles_that_were_too_long(self):
        # #9 and #8 as merged: one summary each, of 93 and 84 characters.
        for title in ('fix(compat): transport streams without tables, payload-named streams, DTS and codec fields agree with ffprobe',
                      'fix(compat): ISO bit rates over the whole media, fragmented streams that start late, subtitles in fragments'):
            failures = check.title_failures(title)
            self.assertTrue(any('50 or fewer' in f for f in failures), failures)
            self.assertTrue(any('under 72' in f for f in failures), failures)

    def test_titles_that_keep_the_shape(self):
        for title in ('fix(thumbnail): image for in-band H.264 Matroska', 'ci: drop the cross-repository and text checks',
                      'perf(mpegts): parse a PES header once per packet', 'docs(h264): official speed snapshot of 2026-10-29',
                      'fix!: reject invalid input', 'fix(api)!: reject invalid input', 'fix(H264): reject invalid input'):
            self.assertEqual(check.title_failures(title), [], title)

    def test_the_type_is_one_of_the_template(self):
        self.assertTrue(check.title_failures('Minimal site: hero only, and the introduction post as a draft'))
        self.assertTrue(check.title_failures('build(cli): a type the template does not list'))
        self.assertTrue(check.title_failures('fix:no space'))


class Commits(unittest.TestCase):
    def test_a_subject_over_72_characters(self):
        # #21's commit as first pushed (73 characters).
        failures = check.commit_failures(log(
            'docs(compat): B3a round 6: the MPEG-TS work generalizes to held-out files\n\nSigned-off-by: A <a@b.c>'))
        self.assertEqual(len(failures), 1)
        self.assertIn('73 characters', failures[0])

    def test_a_subject_wrapped_over_two_lines(self):
        # Git joins the first paragraph into one subject of 81 characters.
        failures = check.commit_failures(log(f'fix: {"a" * 35}\n{"b" * 40}\n\nSigned-off-by: A <a@b.c>'))
        self.assertEqual(len(failures), 1)
        self.assertIn('81 characters', failures[0])

    def test_a_whitespace_only_line_ends_the_subject(self):
        self.assertEqual(check.commit_failures(log(f'fix: {"a" * 67}  \n \t\n{"b" * 300}\n\nSigned-off-by: A <a@b.c>')), [])

    def test_a_subject_that_is_not_conventional(self):
        # Subjects main had before the check: no type, and a type the template does not list.
        for subject in ('Minimal site: hero only, and the introduction post as a draft', 'build(cli): x', 'fix:no space'):
            with self.subTest(subject=subject):
                failures = check.commit_failures(log(subject))
                self.assertEqual(len(failures), 1, failures)
                self.assertIn(NOT_FORM, failures[0])
        for subject in ('fix!: reject invalid input', 'fix(H264): reject invalid input', 'ci: fine'):
            self.assertEqual(check.commit_failures(log(subject)), [], subject)

    def test_a_merge_commit_needs_no_type(self):
        merge = f'{1:040x}\x00{2:040x} {3:040x}\x00Merge remote-tracking branch \'origin/main\' into x\n\x00\n'
        self.assertEqual(check.commit_failures(merge), [])
        self.assertEqual(len(check.commit_failures(merge.replace(f' {3:040x}', ''))), 1)

    def test_attribution_lines_are_not_checked(self):
        self.assertEqual(check.commit_failures(log(
            'fix(cli): hls package refuses - as its directory\n\nCo-Authored-By: A <a@b.c>\nSigned-off-by: A <a@b.c>',
            'fix: x\n\n\U0001f916 Generated with [a tool](https://example.com)\n\nSigned-off-by: A <a@b.c>')), [])

    def test_a_record_separator_in_a_message_hides_nothing(self):
        failures = check.commit_failures(log('update things\n\n\x1e\nmore', 'ci: fine'))
        self.assertEqual(len(failures), 1, failures)
        self.assertIn(f'commit 1{0:011x}', failures[0])

    def test_a_commit_list_in_another_form_is_refused(self):
        sha = 'a' * 40
        for text in (f'{sha}\x00fix: x\n\x1e\n', f'{sha}\x00\x00fix: x\n', f'{sha}\x00\x00fix: x\n\x00\n{sha}',
                     'not a sha\x00\x00fix: x\n\x00\n', f'{sha}\x00fix: x\x00extra\x00\n', f'{sha}\x00fix: x\n\x00\n'):
            failures = check.commit_failures(text)
            self.assertEqual(len(failures), 1, text)
            self.assertIn('no commit was checked', failures[0])
        self.assertEqual(check.commit_failures(''), [])

    def test_ordinary_commits_pass(self):
        self.assertEqual(check.commit_failures(log(
            'perf(mpegts): parse a PES header once per packet\n\nThe code of #9 stays.\n\nSigned-off-by: A <a@b.c>')), [])


BODY = """## TL;DR

- A probe of a fragmented file reads each `moof` once, so a clip of 500 fragments probes faster.
- Nothing outside the probe path changes.

## Background

A fragmented file names its samples in every fragment, and the probe read all of them before this change. coreav-dev/coreav#12 records the slow probe, and the fix reads each fragment header once.

## What changes

**What changes for users of the library or CLI?** A probe of a fragmented file is faster, and its output is unchanged.

**What does this cover, and what nearby areas did it deliberately leave alone?**

- Covered: the fragment walk in `coreav-isobmff`, with a fixture of 500 fragments.
- Not covered: non-fragmented files, and the thumbnail path.

**What must a person do after merging?** None.

## How it works

<details><summary>Probe a fragmented file</summary>

```mermaid
sequenceDiagram
  participant C as coreav CLI
  participant L as library API
  C->>L: probe(path)
  Note over C,L: Changed: previously every fragment was read
```

</details>

## Decisions

- Read the headers in one pass rather than caching the walk, so a caller that stops early stops reading.

## Evidence

**Verification run**: `cargo test -p coreav-isobmff` passed.

- [x] Every commit is signed off (`git commit -s`, DCO).
"""

EXEMPT = """## TL;DR

-
- A bullet, on one line.

## Background

A paragraph, on one line.

Another paragraph, on one line.

> A quote
> that runs over two lines.

> A quoted paragraph
with a lazy continuation
and another quoted line.

| a | b |
|---|---|
| 1 | 2 |
| 3 | 4 |

Name | Result
--- | ---
unit | pass
fixture | pass

```text
a code block
  whose lines
  wrap
```

```mermaid
sequenceDiagram
  A->>B: a message
```

    An indented code block
      whose lines
      wrap

<!--
a comment
over two lines
-->

<a href="a">
  <img src="b">
</a>

***

Setext heading
==============

## What changes

- Covered: one
  - a nested item
- Not covered: two

## Decisions

A paragraph.

<!-- Evidence follows -->
## Evidence

Text.

<!--
A comment
with a blank line inside

and a line after it
-->
"""


PARAGRAPH = [line for line in BODY.split('\n') if line.startswith('A fragmented file names')][0]


def with_paragraph(text):
    """`BODY` with its Background paragraph, one line, replaced by `text`."""
    return BODY.replace(PARAGRAPH, text)


class Body(unittest.TestCase):
    def assert_passes(self, text):
        """`text`, which may span lines, as the paragraph the body holds."""
        self.assertEqual(check.body_failures(with_paragraph(text)), [])

    def test_a_body_of_the_template_passes(self):
        self.assertEqual(check.body_failures(BODY), [])

    def test_every_exempt_construct_passes(self):
        self.assertEqual(check.body_failures(EXEMPT), [])

    def test_the_template_and_the_required_sections_agree(self):
        # The template owns the section list, and its own comment marks the
        # sections an author may delete: one of those is an optional section.
        template = (HERE.parent / '.github' / 'pull_request_template.md').read_text(encoding='utf-8')
        required, optional, comment, collecting, pending = [], [], '', False, None

        def classify():
            (optional if 'delete this section' in comment.casefold() else required).append(pending)

        for line in template.split('\n'):
            found = check.HEADING.match(line)
            if found and len(found.group(1)) == 2:
                if pending is not None:
                    classify()
                pending, comment, collecting = found.group(2), '', False
                continue
            collecting = collecting or '<!--' in line
            if collecting:
                comment += line + '\n'
                collecting = '-->' not in line.split('<!--', 1)[-1]
        if pending is not None:
            classify()
        self.assertEqual(required, list(check.REQUIRED_SECTIONS))
        self.assertEqual(optional, list(check.OPTIONAL_SECTIONS))

    def test_a_table_without_pipes_at_its_edges_passes(self):
        self.assert_passes('Name | Result\n--- | ---\nunit | pass\nfixture | pass')

    def test_a_comment_ends_at_its_terminator_not_at_a_blank_line(self):
        self.assert_passes('<!--\nA comment\nwith a blank line inside\n\nand a line after it\n-->')

    def test_a_one_line_comment_does_not_hide_the_heading_after_it(self):
        self.assertEqual(check.body_failures(BODY.replace('## Evidence', '<!-- Evidence follows -->\n## Evidence')), [])

    def test_a_table_keeps_its_rows_whatever_their_edges(self):
        self.assert_passes('| Name | Result |\n| --- | --- |\n| unit | pass |\nfixture | pass\nsmoke | pass')

    def test_a_fence_closes_only_at_its_own_marker(self):
        # GFM: the same character, at least as long, and only spaces after it.
        for lines in (['```text', '```not a closing fence', 'code line one', 'code line two', '```'],
                      ['````text', '```', 'code line one', 'code line two', '````'],
                      ['~~~text', '```', 'code line one', 'code line two', '~~~']):
            with self.subTest(opening=lines[0]):
                self.assert_passes('\n'.join(lines))

    def test_a_paragraph_before_a_thematic_break_is_still_a_line_break(self):
        # `***` is a rule, not a setext underline: the two lines stay two lines.
        failures = check.body_failures(with_paragraph('First line\nsecond line\n***'))
        self.assertEqual(len(failures), 1, failures)
        self.assertIn('lines 8 and 9', failures[0])

    def test_a_setext_heading_is_not_a_line_break(self):
        self.assert_passes('Setext heading\n===')

    def test_a_pre_block_ends_at_its_closing_tag(self):
        self.assert_passes('<pre>\ncode line one\ncode line two\n</pre>')

    def test_a_hash_attached_to_a_heading_is_not_a_closing_sequence(self):
        # `## Evidence#` is a heading named `Evidence#`, not the Evidence section.
        failures = check.body_failures(BODY.replace('## Evidence', '## Evidence#'))
        self.assertEqual(len(failures), 1, failures)
        self.assertIn('no `## Evidence` section', failures[0])

    def test_a_lazy_continuation_of_a_quote_passes(self):
        self.assert_passes('> A quoted paragraph\nwith a lazy continuation\nand another quoted line.')

    def test_a_missing_section(self):
        for section in check.REQUIRED_SECTIONS:
            with self.subTest(section=section):
                failures = check.body_failures(BODY.replace(f'## {section}', '## Another'))
                self.assertEqual(len(failures), 1, failures)
                self.assertIn(f'no `## {section}` section', failures[0])

    def test_a_body_with_no_section_at_all(self):
        failures = check.body_failures('A description of the change.')
        self.assertEqual(len(failures), len(check.REQUIRED_SECTIONS), failures)

    def test_a_section_name_in_another_case_is_the_same_section(self):
        self.assertEqual(check.body_failures(BODY.replace('## TL;DR', '## tl;dr')), [])

    def test_a_paragraph_over_two_lines(self):
        # coreav-dev/coreav#174 as merged: two lines GitHub shows as one paragraph.
        failures = check.body_failures(with_paragraph('Before: the slow probe.\nAfter: the fast probe.'))
        self.assertEqual(len(failures), 1, failures)
        self.assertIn('lines 8 and 9', failures[0])
        self.assertIn('join them into one line', failures[0])

    def test_a_list_item_over_two_lines(self):
        failures = check.body_failures(BODY.replace('- Not covered: non-fragmented files, and the thumbnail path.',
                                                   '- Not covered: non-fragmented files, and the thumbnail path,\n  which the next change reads.'))
        self.assertEqual(len(failures), 1, failures)
        self.assertIn('lines 17 and 18', failures[0])

    def test_an_indented_line_continuing_a_paragraph_is_still_a_line_break(self):
        # Indented code cannot start inside a paragraph, so this is a wrap.
        failures = check.body_failures(with_paragraph('A fragmented file names\n    and an indented continuation'))
        self.assertEqual(len(failures), 1, failures)

    def test_one_failure_per_broken_block(self):
        failures = check.body_failures(with_paragraph('a\nb\nc\nd'))
        self.assertEqual(len(failures), 1, failures)


class Command(unittest.TestCase):
    def run_check(self, *args):
        return subprocess.run([sys.executable, '-I', str(HERE / 'pr_shape_check.py'), *args],
                              capture_output=True, text=True)

    def test_event_and_commits(self):
        with tempfile.TemporaryDirectory() as d:
            event = pathlib.Path(d) / 'event.json'
            commits = pathlib.Path(d) / 'commits'
            event.write_text(json.dumps({'pull_request': {'title': 'ci: fine', 'body': BODY}}))
            commits.write_text(log('ci: fine\n\nSigned-off-by: A <a@b.c>'))
            run = self.run_check('--event', str(event), '--commits', str(commits))
            self.assertEqual(run.returncode, 0, run.stdout)
            commits.write_text(log('fine\n\nSigned-off-by: A <a@b.c>'))
            run = self.run_check('--event', str(event), '--commits', str(commits))
            self.assertEqual(run.returncode, 1)
            self.assertIn(NOT_FORM, run.stdout)
            commits.write_text(log('ci: fine').replace('\x00\n', '\x1e\n'))
            run = self.run_check('--event', str(event), '--commits', str(commits))
            self.assertEqual(run.returncode, 1)
            self.assertIn('no commit was checked', run.stdout)
            run = self.run_check('--event', str(event), '--commits', str(pathlib.Path(d) / 'nope'))
            self.assertEqual(run.returncode, 1)
            self.assertIn('pr-shape: cannot read', run.stderr)

    def test_a_title_alone(self):
        run = self.run_check('--title', 'ci: fine')
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertIn('the title keeps its shape', run.stdout)
        run = self.run_check('--title', 'fine')
        self.assertEqual(run.returncode, 1)
        self.assertIn(NOT_FORM, run.stdout)

    def test_title_and_body_files(self):
        with tempfile.TemporaryDirectory() as d:
            body = pathlib.Path(d) / 'body.md'
            body.write_text(BODY.replace('## Decisions', '## Decision'))
            run = self.run_check('--title', 'ci: fine', '--body-file', str(body))
            self.assertEqual(run.returncode, 1)
            self.assertIn('no `## Decisions` section', run.stdout)
            body.write_text(BODY)
            run = self.run_check('--title', 'ci: fine', '--body-file', str(body))
            self.assertEqual(run.returncode, 0, run.stdout)
            run = self.run_check('--title', 'ci: fine', '--body-file', str(pathlib.Path(d) / 'nope'))
            self.assertEqual(run.returncode, 1)
            self.assertIn('pr-shape: cannot read', run.stderr)

    def test_the_workflow_command_on_a_real_repository(self):
        # The log command pr-text.yml runs: a merge, a message holding a record separator, a plain subject.
        with tempfile.TemporaryDirectory() as d:
            git = ['git', '-C', d, '-c', 'user.name=T', '-c', 'user.email=t@t.t']

            def run(*args):
                return subprocess.run(git + list(args), check=True, capture_output=True, text=True).stdout
            run('init', '-q', '-b', 'main')
            run('commit', '-q', '--allow-empty', '-m', 'ci: base')
            run('checkout', '-q', '-b', 'side')
            run('commit', '-q', '--allow-empty', '-m', 'fix: x\n\n\x1e\nupdate things')
            run('checkout', '-q', 'main')
            run('commit', '-q', '--allow-empty', '-m', 'update things')
            run('merge', '-q', '--no-ff', '--no-edit', 'side')
            failures = check.commit_failures(run('log', '--format=%H%x00%P%x00%B%x00', 'HEAD~2..HEAD'))
            self.assertEqual(len(failures), 1, failures)
            self.assertIn(NOT_FORM, failures[0])


if __name__ == '__main__':
    unittest.main()
