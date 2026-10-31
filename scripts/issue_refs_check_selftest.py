#!/usr/bin/env python3
"""Tests of scripts/issue_refs_check.py."""
import importlib.util
import pathlib
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('issue_refs_check', HERE / 'issue_refs_check.py')
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)


def found(text):
    return [s for _, _, s in check.matches(text)]


class Matches(unittest.TestCase):
    def test_each_form_is_refused(self):
        for text, ref in (('the walk of every sample (#123)', '#123'),
                          ('#45\'s raw records', '#45'),
                          ('"change": "#678"', '#678'),
                          ('a review finding on pull request 12', 'pull request 12'),
                          ('Gating run of PR #123', 'PR #123'),
                          ('labels pre45 and pr45', 'pr45'),
                          ('salted with holdout-issue12', 'issue12')):
            self.assertIn(ref, found(text), text)

    def test_a_name_wrapped_onto_the_next_line(self):
        self.assertEqual(found('two probe defects (fixed in pull\nrequest 12), errors'), ['pull request 12'])
        self.assertEqual(found('# lie about their lengths (issue\n# 123): tables'), ['issue 123'])
        self.assertIn('#123', found('declare samples the file does not back (issue\n    # #123).'))

    def test_a_reference_that_names_the_repository_passes(self):
        for text in ('fixed in coreav-dev/coreav#56', 'the issue coreav-dev/coreav#31 tracks',
                     'https://github.com/coreav-dev/coreav/issues/31'):
            self.assertEqual(found(text), [], text)

    def test_numbering_that_is_no_reference_passes(self):
        for text in ('#2\tINITED cov: 73 ft: 78 corp: 2', 'Linux vm 6.12.113 #1 SMP PREEMPT_DYNAMIC',
                     '  Stream #0:2 -> #0:1 (copy)', 'Output stream #1:0 (video)', '-read_intervals %+#2',
                     'PASS after 1080p #2 11:14:20Z', 'FAIL model106 before g.sd #1 04:40Z',
                     '"ffv1" => Some("FFmpeg video codec #1")', '#[derive(Debug)]', 'colour #ffcc00', '&#123;',
                     'Sperschneider, Fraunhofer IIS, issue 1.2, 1997', "SALT = 'coreav-corpus-v1-holdout-issue31:'",
                     'a PR that adds a field', 'issues the request'):
            self.assertEqual(found(text), [], text)


class Paths(unittest.TestCase):
    def test_a_numbered_name_is_refused(self):
        for path in ('crates/coreav-wasm/fuzz/regressions/ops/issue-123-crash', 'docs/evidence/pr45/results.json',
                     'docs/spikes/issue_12.md'):
            self.assertTrue(check.PATH_KIND[1].search(path), path)

    def test_a_named_input_passes(self):
        for path in ('crates/coreav-wasm/fuzz/regressions/ops/campaign-2-crash', 'docs/spikes/w2-hls-index/README.md',
                     'docs/measurements/h264-rounds/profile-82ad7c31/stages_fn.py'):
            self.assertIsNone(check.PATH_KIND[1].search(path), path)

    def test_every_exempt_file_is_tracked(self):
        self.assertEqual(sorted(set(check.EXEMPT) - set(check.tracked(list(check.EXEMPT)))), [])


if __name__ == '__main__':
    unittest.main()
