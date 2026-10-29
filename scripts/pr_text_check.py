#!/usr/bin/env python3
"""A pull request's title and body hold nothing that stays out of this
repository: a pre-flight check an agent runs before opening a pull request
and after editing its text (docs/design/ci-checks.md, "Before opening a
pull request"); no workflow runs it.

Fails on, in the title or the body:
- Korean text (Hangul): the repository is written in English;
- a session or run identifier (a UUID);
- spend details (an amount in USD or dollars, a "Spend" line);
- any pattern of the newline-separated list in the environment variable
  PR_TEXT_TERMS, each a case-insensitive regular expression: the private
  names and the forms of internal messages, kept outside this repository.
  A match is reported by its line, never by the pattern or the text. Blank
  lines and lines starting with `#` are not patterns.

    PR_TEXT_TERMS="$(cat TERMS)" pr_text_check.py --title TITLE --body-file BODY.md
    pr_text_check.py --event EVENT.json

Prints the kind and the line of each match, never the matched text. Exit
status 0 when nothing matches, 1 otherwise. Python standard library only.
"""
import argparse
import json
import os
import re
import sys

KINDS = [
    ('Korean text', re.compile(r'[ᄀ-ᇿ㄰-㆏가-힣]')),
    ('a session or run identifier', re.compile(r'\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b', re.I)),
    ('spend details', re.compile(r'\b\d+(?:[.,]\d+)?\s?USD\b|\$\s?\d+[.,]\d|\$\s?\d{2,}|^\W*spend\b\W*[:=\u2013\u2014-]', re.I | re.M)),
]


def matches(text, terms):
    """(kind, line) of each match in `text`."""
    out = []
    for n, line in enumerate(text.split('\n'), 1):
        for kind, pattern in KINDS:
            if pattern.search(line):
                out.append((kind, n))
        if any(t.search(line) for t in terms):
            out.append(('a pattern of the private list', n))
    return out


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--title', help="the pull request's title")
    ap.add_argument('--body-file', help="a file holding the pull request's body")
    ap.add_argument('--event', help='a GitHub pull_request event file instead')
    a = ap.parse_args()
    if a.event:
        pr = json.load(open(a.event, encoding='utf-8')).get('pull_request') or {}
    elif a.title is not None or a.body_file:
        pr = {'title': a.title or '', 'body': open(a.body_file, encoding='utf-8').read() if a.body_file else ''}
    else:
        sys.exit('pr-text: give --title and --body-file, or --event')
    try:
        terms = [re.compile(t.strip(), re.I) for t in os.environ.get('PR_TEXT_TERMS', '').split('\n')
                 if t.strip() and not t.lstrip().startswith('#')]
    except re.error:
        # The pattern is not printed: the list is secret.
        sys.exit('pr-text: PR_TEXT_TERMS holds a pattern that does not compile')
    found = [('title', k, n) for k, n in matches(pr.get('title') or '', terms)]
    found += [('body', k, n) for k, n in matches(pr.get('body') or '', terms)]
    for where, kind, n in found:
        print(f'{kind} in the {where}, line {n}')
    print(f'pr-text: {len(found)} match(es)' if found else 'pr-text: the title and body are clean')
    sys.exit(1 if found else 0)


if __name__ == '__main__':
    main()
