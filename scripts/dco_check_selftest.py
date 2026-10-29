#!/usr/bin/env python3
"""Tests of scripts/dco_check.py on throwaway repositories: a sign-off
whose name differs from the author's, a missing sign-off, a commit already
on main, and merge commits."""
import importlib.util
import os
import pathlib
import subprocess
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('dco_check', HERE / 'dco_check.py')
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)

AUTHOR = ('Gunwoo Kim', 'gunwooterry@gmail.com')


class Repo:
    def __init__(self, path):
        self.path = path
        self.git('init', '-q', '-b', 'main')
        self.commit('chore: root', signoff='Gunwoo Kim <gunwooterry@gmail.com>')

    def git(self, *args, author=AUTHOR):
        env = {'PATH': os.environ['PATH'], 'HOME': self.path, 'GIT_CONFIG_NOSYSTEM': '1',
               'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_AUTHOR_NAME': author[0], 'GIT_AUTHOR_EMAIL': author[1],
               'GIT_COMMITTER_NAME': author[0], 'GIT_COMMITTER_EMAIL': author[1]}
        return subprocess.run(['git', '-C', self.path, *args], env=env, check=True,
                              capture_output=True, text=True).stdout.strip()

    def commit(self, subject, signoff=None, author=AUTHOR):
        message = subject + (f'\n\nSigned-off-by: {signoff}' if signoff else '')
        self.git('commit', '-q', '--allow-empty', '-m', message, author=author)
        return self.git('rev-parse', 'HEAD')

    def merge(self, branch, signoff=None):
        message = f'Merge {branch}' + (f'\n\nSigned-off-by: {signoff}' if signoff else '')
        self.git('merge', '-q', '--no-ff', '-m', message, branch)
        return self.git('rev-parse', 'HEAD')


class DcoCheck(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.repo = Repo(self.dir.name)
        self.base = self.repo.git('rev-parse', 'HEAD')
        self.repo.git('switch', '-q', '-c', 'feature')

    def tearDown(self):
        self.dir.cleanup()

    def problems(self):
        return check.problems(self.base, 'HEAD', 'main', self.dir.name)

    def test_a_sign_off_with_the_authors_email_passes_whatever_its_name(self):
        self.repo.commit('fix: a', signoff='gunwooterry <gunwooterry@gmail.com>')
        self.repo.commit('fix: b', signoff='someone <GunwooTerry@Gmail.com>')
        self.assertEqual(self.problems(), [])

    def test_a_missing_or_foreign_sign_off_fails_and_names_the_fix(self):
        missing = self.repo.commit('fix: unsigned')
        foreign = self.repo.commit('fix: signed by another', signoff='Gunwoo Kim <other@example.com>')
        found = self.problems()
        self.assertEqual(len(found), 2, found)
        self.assertTrue(any(missing[:12] in p for p in found), found)
        self.assertTrue(any(foreign[:12] in p for p in found), found)
        for problem in found:
            self.assertIn('git commit --amend -s', problem)
            self.assertIn(f'git rebase --signoff {self.base}', problem)
            self.assertIn(f'git rebase --signoff --rebase-merges {self.base}', problem)
            self.assertIn('git merge --signoff', problem)

    def test_a_commit_already_on_main_is_skipped(self):
        self.repo.commit('feat: own', signoff='Gunwoo Kim <gunwooterry@gmail.com>')
        self.repo.git('switch', '-q', 'main')
        self.repo.commit('docs: squash message rewritten without a sign-off')
        self.repo.git('switch', '-q', 'feature')
        self.repo.merge('main', signoff='Gunwoo Kim <gunwooterry@gmail.com>')
        self.assertEqual(self.problems(), [])
        self.assertEqual(len(check.problems(self.base, 'HEAD', self.base, self.dir.name)), 1)

    def test_a_merge_commit_is_judged_like_any_other(self):
        self.repo.git('switch', '-q', '-c', 'side')
        self.repo.commit('feat: side', signoff='Gunwoo Kim <gunwooterry@gmail.com>')
        self.repo.git('switch', '-q', 'feature')
        self.repo.commit('feat: own', signoff='Gunwoo Kim <gunwooterry@gmail.com>')
        merge = self.repo.merge('side')
        found = self.problems()
        self.assertEqual(len(found), 1, found)
        self.assertIn(merge[:12], found[0])
        self.repo.git('commit', '-q', '--amend', '--no-edit', '-s')
        self.assertEqual(self.problems(), [])

    def test_a_missing_main_ref_fails(self):
        self.assertTrue(check.problems(self.base, 'HEAD', 'no-such-ref', self.dir.name))


if __name__ == '__main__':
    unittest.main()
