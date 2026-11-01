""".github/actions/changed-paths/changed_paths.py: the filter's decisions."""
import importlib.util
import os
import pathlib
import subprocess
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location(
    'changed_paths', pathlib.Path(__file__).resolve().parents[1] / 'changed_paths.py')
cp = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cp)


class Patterns(unittest.TestCase):
    def match(self, pattern, name):
        return bool(cp.to_regex(pattern).match(name))

    def test_a_star_does_not_cross_a_slash(self):
        self.assertTrue(self.match('*.md', 'README.md'))
        self.assertFalse(self.match('*.md', 'docs/README.md'))

    def test_two_stars_cross_a_slash(self):
        self.assertTrue(self.match('docs/design/**', 'docs/design/a/b.md'))
        self.assertTrue(self.match('crates/**', 'crates/coreav-mkv/src/lib.rs'))
        self.assertFalse(self.match('crates/**', 'bench/guard/baseline.json'))

    def test_two_stars_between_slashes_need_a_directory(self):
        # GitHub's `**` matches zero or more of any character, so the
        # slashes around it are literal: docs/README.md is not matched.
        self.assertTrue(self.match('docs/**/README.md', 'docs/spikes/README.md'))
        self.assertFalse(self.match('docs/**/README.md', 'docs/README.md'))

    def test_quantifiers_and_escapes_are_refused(self):
        # GitHub's `?` and `+` quantify the preceding character and `\`
        # escapes, so `*.jsx?` matches .js and .jsx there and nothing of
        # the sort under a glob reading. Refused rather than guessed at.
        for pattern in ('*.jsx?', r'docs/\*.md'):
            with self.assertRaises(SystemExit):
                cp.to_regex(pattern)

    def test_a_pattern_must_match_the_whole_path(self):
        self.assertFalse(self.match('crates', 'crates/coreav-mkv/src/lib.rs'))
        self.assertFalse(self.match('Cargo.toml', 'crates/coreav-mkv/Cargo.toml'))

    def test_a_dot_is_literal(self):
        self.assertFalse(self.match('Cargo.toml', 'CargoXtoml'))

    def test_an_unsupported_pattern_is_refused(self):
        for pattern in ('!docs/**', 'docs/a+.md', 'docs/[ab].md'):
            with self.assertRaises(SystemExit):
                cp.to_regex(pattern)


class Decisions(unittest.TestCase):
    paths = ['crates/**', 'Cargo.lock', '.github/workflows/bench-guard.yml']
    ignore = ['docs/design/**', '*.md', 'spikes/**/README.md']

    def test_paths_runs_when_one_file_matches(self):
        run, _ = cp.decide(['README.md', 'crates/coreav-mkv/src/lib.rs'], self.paths, [])
        self.assertTrue(run)

    def test_paths_skips_when_no_file_matches(self):
        run, _ = cp.decide(['README.md', 'docs/design/mkv.md'], self.paths, [])
        self.assertFalse(run)

    def test_paths_ignore_runs_when_one_file_is_outside(self):
        run, _ = cp.decide(['README.md', 'crates/coreav-mkv/src/lib.rs'], [], self.ignore)
        self.assertTrue(run)

    def test_paths_ignore_skips_when_every_file_is_inside(self):
        run, _ = cp.decide(['README.md', 'docs/design/mkv.md', 'spikes/h264-n2/README.md'],
                           [], self.ignore)
        self.assertFalse(run)

    def test_unknown_changed_files_run_everything(self):
        self.assertTrue(cp.decide(None, self.paths, [])[0])
        self.assertTrue(cp.decide(None, [], self.ignore)[0])

    def test_an_event_that_changes_no_file_runs_everything(self):
        self.assertTrue(cp.decide([], self.paths, [])[0])

    def test_exactly_one_filter_is_required(self):
        with self.assertRaises(SystemExit):
            cp.decide(['README.md'], [], [])
        with self.assertRaises(SystemExit):
            cp.decide(['README.md'], self.paths, self.ignore)


class MergeGroups(unittest.TestCase):
    """A merge group's changed files, in a checkout shaped as the queue's:
    `fetch-depth: 2` of a group whose base is more than one parent away."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = pathlib.Path(tmp.name)
        work, origin, clone = root / 'work', root / 'origin.git', root / 'clone'

        def git(*args, cwd=work):
            return subprocess.run(('git', '-c', 'user.name=t', '-c', 'user.email=t@t', *args), cwd=cwd,
                                  check=True, capture_output=True, text=True).stdout.strip()
        work.mkdir()
        git('init', '-q', '-b', 'main')
        self.commits = {}
        for name, path in (('first', 'README.md'), ('base', 'Cargo.lock'),
                           ('ahead', 'crates/coreav-mkv/src/lib.rs'), ('group', 'docs/design/mkv.md')):
            (work / path).parent.mkdir(parents=True, exist_ok=True)
            (work / path).write_text(name)
            git('add', path)
            git('commit', '-q', '-m', name)
            self.commits[name] = git('rev-parse', 'HEAD')
        git('clone', '-q', '--bare', str(work), str(origin), cwd=root)
        git('clone', '-q', '--depth=2', origin.as_uri(), str(clone), cwd=root)
        cwd = os.getcwd()
        os.chdir(clone)
        self.addCleanup(os.chdir, cwd)

    def test_the_group_is_diffed_against_its_base_which_is_fetched(self):
        base = self.commits['base']
        missing = subprocess.run(('git', 'cat-file', '-e', base + '^{commit}'), capture_output=True)
        self.assertNotEqual(missing.returncode, 0)
        self.assertEqual(sorted(cp.changed_files('merge_group', '', base)),
                         ['crates/coreav-mkv/src/lib.rs', 'docs/design/mkv.md'])

    def test_a_group_without_a_reachable_base_runs_everything(self):
        self.assertIsNone(cp.changed_files('merge_group', '', ''))
        self.assertIsNone(cp.changed_files('merge_group', '', 'f' * 40))


class Patternlists(unittest.TestCase):
    def test_blank_lines_and_spaces_are_dropped(self):
        self.assertEqual(cp.patterns_of('\n crates/** \n\n Cargo.lock\n'),
                         ['crates/**', 'Cargo.lock'])
        self.assertEqual(cp.patterns_of(''), [])

    def test_comments_are_dropped(self):
        self.assertEqual(cp.patterns_of('# why\ncrates/**\n  # and why\nCargo.lock\n'),
                         ['crates/**', 'Cargo.lock'])


if __name__ == '__main__':
    unittest.main()
