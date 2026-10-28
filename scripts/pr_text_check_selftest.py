#!/usr/bin/env python3
"""Tests of scripts/pr_text_check.py."""
import importlib.util
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('pr_text_check', HERE / 'pr_text_check.py')
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)


def kinds(text, terms=()):
    return [k for k, _ in check.matches(text, [check.re.compile(t, check.re.I) for t in terms])]


class Matches(unittest.TestCase):
    def test_each_kind(self):
        self.assertEqual(kinds('한국어 요약'), ['Korean text'])
        self.assertEqual(kinds('session 01234567-89ab-cdef-0123-456789abcdef'), ['a session or run identifier'])
        self.assertEqual(kinds('Spend: none.'), ['spend details'])
        self.assertEqual(kinds('**Spend**: 3.6 USD'), ['spend details'])
        self.assertEqual(kinds('Cloud cost 0.85 USD'), ['spend details'])
        self.assertEqual(kinds('Cloud: about $3.20'), ['spend details'])
        self.assertEqual(kinds('Spend \u2014 none'), ['spend details'])
        self.assertEqual(kinds('built on the Dev Box', [r'\bdev box\b']), ['a pattern of the private list'])
        self.assertEqual(kinds('a devbox', [r'\bdev box\b']), [])

    def test_ordinary_text_passes(self):
        for text in ('Fixes #12: the probe reads `moov` once.', 'spending less time in CABAC',
                     'PMT parsing (MPEG-TS)', "awk '{print $2}' and sed s/x/$1/", 'sha256 0123456789abcdef', '🤖 Generated with [Claude Code](https://claude.com/claude-code)'):
            self.assertEqual(kinds(text), [], text)

    def test_the_command_reports_lines_not_text(self):
        with tempfile.TemporaryDirectory() as d:
            event = pathlib.Path(d) / 'event.json'
            event.write_text(json.dumps({'pull_request': {'title': 'Fine', 'body': 'a\nsecret-word here'}}))
            env = {**os.environ, 'PR_TEXT_TERMS': r'secret-w\w+' + '\n'}
            run = subprocess.run([sys.executable, '-I', str(HERE / 'pr_text_check.py'), '--event', str(event)],
                                 capture_output=True, text=True, env=env)
            self.assertEqual(run.returncode, 1)
            self.assertIn('body, line 2', run.stdout)
            self.assertNotIn('secret-word', run.stdout)
            event.write_text(json.dumps({'pull_request': {'title': 'Fine', 'body': None}}))
            run = subprocess.run([sys.executable, '-I', str(HERE / 'pr_text_check.py'), '--event', str(event)],
                                 capture_output=True, text=True, env=env)
            self.assertEqual(run.returncode, 0, run.stdout)
            # Comment and blank lines are not patterns, as in the export's
            # list files; an empty list fails where one is required.
            env['PR_TEXT_TERMS'] = '# names (of the hosts\n\n#\n'
            run = subprocess.run([sys.executable, '-I', str(HERE / 'pr_text_check.py'), '--event', str(event)],
                                 capture_output=True, text=True, env=env)
            self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
            env['PR_TEXT_REQUIRE_TERMS'] = '1'
            run = subprocess.run([sys.executable, '-I', str(HERE / 'pr_text_check.py'), '--event', str(event)],
                                 capture_output=True, text=True, env=env)
            self.assertEqual(run.returncode, 1)
            env['PR_TEXT_TERMS'] = '(unclosed'
            run = subprocess.run([sys.executable, '-I', str(HERE / 'pr_text_check.py'), '--event', str(event)],
                                 capture_output=True, text=True, env=env)
            self.assertEqual(run.returncode, 1)
            self.assertNotIn('unclosed', run.stdout + run.stderr)


if __name__ == '__main__':
    unittest.main()
