#!/usr/bin/env python3
"""A pull request's title and commits keep the Conventional Commits shape
the pull request template states (docs/design/ci-checks.md, "The pull
request's shape"): what review kept checking by hand. Run by
.github/workflows/pr-text.yml on every pull request event, and by hand
before opening one.

Fails on:
- a title that is not `type(scope): summary` (or `type(scope)!: summary`)
  with one of the template's types (feat, fix, docs, test, chore,
  refactor, perf, ci), a summary longer than 50 characters, or a title of
  72 characters or more;
- a commit whose subject is not of that form (merge commits excepted) or is
  longer than 72 characters;
- a commit list that is not in the form below: none of it is checked.

    pr_shape_check.py --event EVENT.json [--commits FILE]
    pr_shape_check.py --title TITLE [--commits FILE]

FILE holds the commits as `git log --format='%H%x00%P%x00%B%x00' BASE..HEAD`
writes them: NUL, which Git does not allow in a commit message, ends each
field (sha, parents, message). Prints each failure with what to do
instead. Exit status 0 when nothing fails, 1 otherwise. Python standard
library only.
"""
import argparse
import json
import re
import sys

TYPES = ('feat', 'fix', 'docs', 'test', 'chore', 'refactor', 'perf', 'ci')
CONVENTIONAL = re.compile(r'\A(?P<type>[a-z]+)(?:\([A-Za-z0-9][A-Za-z0-9._/-]*\))?!?: (?P<summary>\S.*)\Z')
FORM = f'`type(scope): summary` with a type of {", ".join(TYPES)}'
SUMMARY_MAX = 50
TITLE_LIMIT = 72
SUBJECT_MAX = 72
SHA = re.compile(r'[0-9a-f]{40}|[0-9a-f]{64}')
PARENTS = re.compile(r'(?:[0-9a-f]{40}|[0-9a-f]{64})(?: (?:[0-9a-f]{40}|[0-9a-f]{64}))*|')


def conventional(text):
    found = CONVENTIONAL.match(text)
    return found if found and found['type'] in TYPES else None


def title_failures(title):
    """What is wrong with a pull request's title, each with its fix."""
    found = conventional(title)
    if not found:
        return [f'the title is not {FORM}']
    out = []
    if len(found['summary']) > SUMMARY_MAX:
        out.append(f'the title\'s summary is {len(found["summary"])} characters: describe one outcome in '
                   f'{SUMMARY_MAX} or fewer and put parameters and identifiers in the TL;DR or Decisions')
    if len(title) >= TITLE_LIMIT:
        out.append(f'the title is {len(title)} characters: keep it under {TITLE_LIMIT}')
    return out


def commit_failures(log):
    """What is wrong with the commits of `git log --format='%H%x00%P%x00%B%x00'`."""
    fields = log.split('\x00')
    shas = [sha.strip('\n') for sha in fields[0:-1:3]]
    parents = fields[1:-1:3]
    if (len(fields) % 3 != 1 or fields[-1].strip('\n') or not all(SHA.fullmatch(sha) for sha in shas)
            or not all(PARENTS.fullmatch(p) for p in parents)):
        return ["the commit list is not what `git log --format='%H%x00%P%x00%B%x00' BASE..HEAD` writes, so no "
                'commit was checked: write it with that command']
    out = []
    for sha, parent, message in zip(shas, parents, fields[2:-1:3]):
        # Git's subject is the whole first paragraph, up to a blank or
        # whitespace-only line, its lines trimmed and joined.
        first = re.split(r'\n[ \t]*\n', message.strip('\n'), maxsplit=1)[0]
        subject = ' '.join(line.rstrip() for line in first.split('\n'))
        if len(parent.split()) < 2 and not conventional(subject):
            out.append(f'commit {sha[:12]}: its subject is not {FORM}; reword it '
                       '(git commit --amend, or git rebase -i and reword)')
        if len(subject) > SUBJECT_MAX:
            out.append(f'commit {sha[:12]}: its subject is {len(subject)} characters; '
                       f'shorten it to {SUBJECT_MAX} or fewer (git commit --amend, or a fixup and rebase)')
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--event', help='a GitHub pull_request event file')
    ap.add_argument('--title', help="the pull request's title")
    ap.add_argument('--commits', help="the pull request's commits (see above)")
    a = ap.parse_args()
    if a.event:
        with open(a.event, encoding='utf-8') as f:
            title = (json.load(f).get('pull_request') or {}).get('title') or ''
    elif a.title is not None:
        title = a.title
    else:
        sys.exit('pr-shape: give --event, or --title')
    failures = title_failures(title)
    if a.commits:
        try:
            with open(a.commits, encoding='utf-8') as f:
                failures += commit_failures(f.read())
        except OSError as e:
            sys.exit(f'pr-shape: cannot read {a.commits}: {e.strerror}')
    for failure in failures:
        print(failure)
    print(f'pr-shape: {len(failures)} failure(s)' if failures else 'pr-shape: the title and commits keep their shape')
    sys.exit(1 if failures else 0)


if __name__ == '__main__':
    main()
