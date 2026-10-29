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


class Command(unittest.TestCase):
    def run_check(self, *args):
        return subprocess.run([sys.executable, '-I', str(HERE / 'pr_shape_check.py'), *args],
                              capture_output=True, text=True)

    def test_event_and_commits(self):
        with tempfile.TemporaryDirectory() as d:
            event = pathlib.Path(d) / 'event.json'
            commits = pathlib.Path(d) / 'commits'
            event.write_text(json.dumps({'pull_request': {'title': 'ci: fine', 'body': None}}))
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
