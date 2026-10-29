#!/usr/bin/env python3
"""A pull request's title and commits keep the shape the pull request
template states (docs/design/ci-checks.md, "The pull request's shape"):
what review kept checking by hand. Run by .github/workflows/pr-text.yml on
every pull request event, and by hand before opening one.

Fails on:
- a title that is not `type(scope): summary` with one of the template's
  types (feat, fix, docs, test, chore, refactor, perf, ci), a summary
  longer than 50 characters, or a title of 72 characters or more;
- a commit whose subject is longer than 72 characters;
- a `Co-Authored-By` trailer or a "Generated with" line in a commit
  message or in the body.

    pr_shape_check.py --event EVENT.json [--commits FILE]
    pr_shape_check.py --title TITLE [--body-file BODY.md] [--commits FILE]

FILE holds commit messages as `git log --format='%H%x00%B%x1e' BASE..HEAD`
writes them. Prints each failure with what to do instead. Exit status 0
when nothing fails, 1 otherwise. Python standard library only.
"""
import argparse
import json
import re
import sys

TYPES = ('feat', 'fix', 'docs', 'test', 'chore', 'refactor', 'perf', 'ci')
TITLE = re.compile(r'\A(?P<type>[a-z]+)(?:\([a-z0-9][a-z0-9._/-]*\))?: (?P<summary>\S.*)\Z')
SUMMARY_MAX = 50
TITLE_LIMIT = 72
SUBJECT_MAX = 72
ATTRIBUTION = re.compile(r'^[ \t]*(?:co-authored-by:|(?:\U0001f916\s*)?generated with\b)', re.I | re.M)


def title_failures(title):
    """What is wrong with a pull request's title, each with its fix."""
    found = TITLE.match(title)
    if not found or found['type'] not in TYPES:
        return [f'the title is not `type(scope): summary` with a type of {", ".join(TYPES)}']
    out = []
    if len(found['summary']) > SUMMARY_MAX:
        out.append(f'the title\'s summary is {len(found["summary"])} characters: describe one outcome in '
                   f'{SUMMARY_MAX} or fewer and put parameters and identifiers in the TL;DR or Decisions')
    if len(title) >= TITLE_LIMIT:
        out.append(f'the title is {len(title)} characters: keep it under {TITLE_LIMIT}')
    return out


def text_failures(where, text):
    """Attribution lines in a commit message or the body."""
    return [f'{where} line {text.count(chr(10), 0, m.start()) + 1} carries an attribution '
            '(`Co-Authored-By` or "Generated with"): remove it; commits carry only their sign-off'
            for m in ATTRIBUTION.finditer(text)]


def commit_failures(log):
    """What is wrong with the commits of `git log --format='%H%x00%B%x1e'`."""
    out = []
    for record in log.split('\x1e'):
        sha, _, message = record.strip('\n').partition('\x00')
        if not sha:
            continue
        # Git's subject is the whole first paragraph, up to a blank or
        # whitespace-only line, its lines trimmed and joined.
        first = re.split(r'\n[ \t]*\n', message.strip('\n'), maxsplit=1)[0]
        subject = ' '.join(line.rstrip() for line in first.split('\n'))
        if len(subject) > SUBJECT_MAX:
            out.append(f'commit {sha[:12]}: its subject is {len(subject)} characters; '
                       f'shorten it to {SUBJECT_MAX} or fewer (git commit --amend, or a fixup and rebase)')
        out += text_failures(f'commit {sha[:12]}:', message)
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--event', help='a GitHub pull_request event file')
    ap.add_argument('--title', help="the pull request's title")
    ap.add_argument('--body-file', help="a file holding the pull request's body")
    ap.add_argument('--commits', help="the pull request's commit messages (see above)")
    a = ap.parse_args()
    if a.event:
        with open(a.event, encoding='utf-8') as f:
            pr = json.load(f).get('pull_request') or {}
    elif a.title is not None:
        body = ''
        if a.body_file:
            with open(a.body_file, encoding='utf-8') as f:
                body = f.read()
        pr = {'title': a.title, 'body': body}
    else:
        sys.exit('pr-shape: give --event, or --title')
    failures = title_failures(pr.get('title') or '') + text_failures('body', pr.get('body') or '')
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
