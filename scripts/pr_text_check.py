#!/usr/bin/env python3
"""A pull request's title and body hold nothing that stays out of the public
repository (docs/design/ci-checks.md; `pr-text.yml`).

Fails on, in the title or the body:
- Korean text (Hangul): the repository is written in English;
- a session or run identifier (a UUID);
- spend details (an amount in USD or dollars, a "Spend" line);
- any pattern of the newline-separated list in the environment variable
  PR_TEXT_TERMS, each a case-insensitive regular expression (a repository
  secret, so the list stays private: the names of the build machines and
  the provider, and the forms of internal coordination messages, as the
  export's extra terms; a match is reported by its line, never by the
  pattern or the text). Blank lines and lines starting with `#` are not
  patterns, as in the export's list files. With PR_TEXT_REQUIRE_TERMS set,
  an empty list fails: the workflow sets it, so a secret that is missing
  does not pass in silence.

    pr_text_check.py [--event EVENT.json]

Reads the pull request from the event file (by default GITHUB_EVENT_PATH).
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
    ap.add_argument('--event', default=os.environ.get('GITHUB_EVENT_PATH'))
    a = ap.parse_args()
    if not a.event:
        sys.exit('pr-text: no event file (--event or GITHUB_EVENT_PATH)')
    pr = json.load(open(a.event, encoding='utf-8')).get('pull_request') or {}
    try:
        terms = [re.compile(t.strip(), re.I) for t in os.environ.get('PR_TEXT_TERMS', '').split('\n')
                 if t.strip() and not t.lstrip().startswith('#')]
    except re.error:
        # The pattern is not printed: the list is secret.
        sys.exit('pr-text: PR_TEXT_TERMS holds a pattern that does not compile')
    if not terms and os.environ.get('PR_TEXT_REQUIRE_TERMS'):
        sys.exit('pr-text: PR_TEXT_TERMS is empty or not set')
    found = [('title', k, n) for k, n in matches(pr.get('title') or '', terms)]
    found += [('body', k, n) for k, n in matches(pr.get('body') or '', terms)]
    for where, kind, n in found:
        print(f'::error::{kind} in the {where}, line {n}')
    print(f'pr-text: {len(found)} match(es)' if found else 'pr-text: the title and body are clean')
    sys.exit(1 if found else 0)


if __name__ == '__main__':
    main()
