#!/usr/bin/env python3
"""Every commit a pull request adds carries a Developer Certificate of
Origin sign-off by its author (docs/design/ci-checks.md, "DCO sign-off").
Run by .github/workflows/deny.yml on every pull request, and by hand
before pushing.

A commit passes when one of its `Signed-off-by` trailers carries its
author's email, compared without regard to case. The name may differ:
GitHub signs a squash merge with the account's login while the author is
the account's display name. Merge commits are judged like any other.
Commits already reachable from MAIN are not the pull request's own and
are skipped, even when BASE..HEAD holds them, as it does for a stacked
branch that merged main in.

    dco_check.py --base BASE --head HEAD [--main MAIN] [--repo DIR]

MAIN defaults to origin/main. Prints each failing commit with the
command that fixes it. Exit status 0 when every commit passes, 1
otherwise. Python standard library and git.
"""
import argparse
import re
import subprocess
import sys

EMAIL = re.compile(r'<([^<>]+)>\s*\Z')


def problems(base, head, main, repo='.'):
    """Each commit in BASE..HEAD, and not on MAIN, whose sign-offs miss its author's email."""
    log = subprocess.run(
        ['git', '-C', repo, 'log', '-z',
         '--format=%H%n%ae%n%(trailers:key=Signed-off-by,valueonly,unfold,separator=%x01)%n%s',
         head, f'^{base}', f'^{main}', '--'],
        capture_output=True, text=True)
    if log.returncode != 0:
        return [f'git log {head} ^{base} ^{main} failed: {log.stderr.strip()}']
    out = []
    for record in filter(None, log.stdout.split('\0')):
        sha, email, signoffs, subject = record.split('\n', 3)
        signed = {found[1].casefold() for value in signoffs.split('\x01') if (found := EMAIL.search(value))}
        if email.casefold() not in signed:
            out.append(f'commit {sha[:12]} ("{subject}") has no Signed-off-by with its author\'s email <{email}>: '
                       f'sign it off with `git commit --amend -s --no-edit` if it is the last commit. Otherwise '
                       f'`git rebase --signoff {base}` signs every commit of the branch, but it rewrites history (push '
                       f'with --force-with-lease) and drops merge commits; on a branch with merges, '
                       f'`git rebase --signoff --rebase-merges {base}` keeps them but leaves each merge unsigned, so '
                       f'redo an unsigned merge with `git merge --signoff`')
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--base', required=True)
    parser.add_argument('--head', required=True)
    parser.add_argument('--main', default='origin/main')
    parser.add_argument('--repo', default='.')
    args = parser.parse_args()
    found = problems(args.base, args.head, args.main, args.repo)
    for problem in found:
        print(problem, file=sys.stderr)
    if found:
        return 1
    print(f'dco-check: every commit in {args.base}..{args.head} not on {args.main} is signed off by its author')
    return 0


if __name__ == '__main__':
    sys.exit(main())
