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


def log(*messages):
    return ''.join(f'{n:040x}\x00{m}\n\x1e\n' for n, m in enumerate(messages, 1))


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
                      'perf(mpegts): parse a PES header once per packet', 'docs(h264): official speed snapshot of 2026-10-29'):
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
        failures = check.commit_failures(log(f'{"a" * 40}\n{"b" * 40}\n\nSigned-off-by: A <a@b.c>'))
        self.assertEqual(len(failures), 1)
        self.assertIn('81 characters', failures[0])

    def test_a_whitespace_only_line_ends_the_subject(self):
        self.assertEqual(check.commit_failures(log(f'{"a" * 72}  \n \t\n{"b" * 300}\n\nSigned-off-by: A <a@b.c>')), [])

    def test_attribution_trailers(self):
        failures = check.commit_failures(log(
            'fix(cli): hls package refuses - as its directory\n\nCo-Authored-By: Claude Opus 5.5 (1M context) <noreply@anthropic.com>\nSigned-off-by: A <a@b.c>',
            'fix: x\n\n\U0001f916 Generated with [Claude Code](https://claude.com/claude-code)\n\nSigned-off-by: A <a@b.c>'))
        self.assertEqual(len(failures), 2, failures)
        self.assertIn('line 3', failures[0])
        self.assertTrue(check.text_failures('body', 'text\nco-authored-by: someone'))
        self.assertTrue(check.text_failures('body', 'Generated with Claude Code'))
        self.assertEqual(check.text_failures('body', '- "Generated with" lines are caught here'), [])

    def test_ordinary_commits_pass(self):
        self.assertEqual(check.commit_failures(log(
            'perf(mpegts): parse a PES header once per packet\n\nThe co-authored code of #9 and text generated with ffmpeg stay.\n\nSigned-off-by: A <a@b.c>')), [])


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
            commits.write_text(log('ci: fine\n\nCo-Authored-By: B <b@c.d>'))
            run = self.run_check('--event', str(event), '--commits', str(commits))
            self.assertEqual(run.returncode, 1)
            self.assertIn('attribution', run.stdout)
            run = self.run_check('--event', str(event), '--commits', str(pathlib.Path(d) / 'nope'))
            self.assertEqual(run.returncode, 1)
            self.assertIn('pr-shape: cannot read', run.stderr)


if __name__ == '__main__':
    unittest.main()
