#!/usr/bin/env python3
"""A pull request's title, body and commits keep the shape the pull
request template states (docs/design/ci-checks.md, "The pull request's
shape"): what review kept checking by hand. Run by
.github/workflows/pr-text.yml on every pull request event, and by hand
before opening one.

Fails on:
- a title that is not `type(scope): summary` (or `type(scope)!: summary`)
  with one of the template's types (feat, fix, docs, test, chore,
  refactor, perf, ci), a summary longer than 50 characters, or a title of
  72 characters or more;
- a commit whose subject is not of that form (merge commits excepted) or is
  longer than 72 characters;
- a commit list that is not in the form below: none of it is checked;
- a body without a level-2 heading for one of the template's required
  sections. The headings come from markdown-it-py's tokens, so a `## ` line
  inside a code fence, a table, an HTML block or a block quote is not one
  of them: those are the parser's blocks, not the body's sections.

    pr_shape_check.py --event EVENT.json [--commits FILE]
    pr_shape_check.py --title TITLE [--body-file BODY.md] [--commits FILE]

FILE holds the commits as `git log --format='%H%x00%P%x00%B%x00' BASE..HEAD`
writes them: NUL, which Git does not allow in a commit message, ends each
field (sha, parents, message). Prints each failure with what to do
instead. Exit status 0 when nothing fails, 1 otherwise. Python 3.8 or
later, with markdown-it-py and mdurl from scripts/pr-text-requirements.txt
installed first: `pip install --require-hashes -r` that file.
"""
import argparse
import json
import re
import sys

from markdown_it import MarkdownIt

TYPES = ('feat', 'fix', 'docs', 'test', 'chore', 'refactor', 'perf', 'ci')
CONVENTIONAL = re.compile(r'\A(?P<type>[a-z]+)(?:\([A-Za-z0-9][A-Za-z0-9._/-]*\))?!?: (?P<summary>\S.*)\Z')
FORM = f'`type(scope): summary` with a type of {", ".join(TYPES)}'
SUMMARY_MAX = 50
TITLE_LIMIT = 72
SUBJECT_MAX = 72
SHA = re.compile(r'[0-9a-f]{40}|[0-9a-f]{64}')
PARENTS = re.compile(r'(?:[0-9a-f]{40}|[0-9a-f]{64})(?: (?:[0-9a-f]{40}|[0-9a-f]{64}))*|')

# The sections .github/pull_request_template.md opens with `## `, split by
# whether it tells the author to keep them. A section whose comment says
# "delete this section" is optional and the rest are required. The test reads
# the template, takes that comment as the marker, and fails when these two
# lists no longer hold its level-2 headings.
REQUIRED_SECTIONS = ('TL;DR', 'Background', 'What changes', 'Decisions', 'Evidence')
OPTIONAL_SECTIONS = ('How it works', 'Needs the maintainer')

# The parser the body is read with: markdown-it's own preset, which carries
# GitHub's table and strikethrough rules, with raw HTML read as the block
# it is rather than as text. Its tokens say where every block starts and
# ends, so a block's extent is the parser's answer, never this file's.
PARSER = MarkdownIt('js-default', {'html': True})


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


def body_failures(body):
    """What is wrong with a pull request's body, each with its fix."""
    headings = scan_body(body)
    normalized = {' '.join(text.split()).casefold() for text, _ in headings}
    return [f'the body has no level-2 heading for its `{section}` section: the template requires one, so '
            f'write `## {section}` and the section it opens'
            for section in REQUIRED_SECTIONS if section.casefold() not in normalized]


def scan_body(body):
    """(heading text, line) of each level-2 heading the body states, a
    heading inside a block quote excluded: a quote's heading is quoted text,
    not one of the sections."""
    headings = []
    quote = 0       # how deep the tokens under way are inside a block quote
    heading = None  # the heading_open whose text has not been read yet
    for token in PARSER.parse(body):
        if token.type == 'blockquote_open':
            quote += 1
        elif token.type == 'blockquote_close':
            quote -= 1
        elif token.type == 'heading_open':
            heading = token
        elif token.type == 'heading_close':
            heading = None
        elif token.type == 'inline' and heading is not None:
            if not quote and heading.tag == 'h2' and heading.map:
                headings.append((token.content, heading.map[0] + 1))
            heading = None
    return headings


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--event', help='a GitHub pull_request event file')
    ap.add_argument('--title', help="the pull request's title")
    ap.add_argument('--body-file', help="a file holding the pull request's body")
    ap.add_argument('--commits', help="the pull request's commits (see above)")
    a = ap.parse_args()
    body = None
    if a.event:
        with open(a.event, encoding='utf-8') as f:
            pr = json.load(f).get('pull_request') or {}
        body = pr.get('body') or ''
    elif a.title is not None:
        pr = {'title': a.title}
        if a.body_file is not None:
            try:
                with open(a.body_file, encoding='utf-8') as f:
                    body = f.read()
            except OSError as e:
                sys.exit(f'pr-shape: cannot read {a.body_file}: {e.strerror}')
    else:
        sys.exit('pr-shape: give --event, or --title')
    failures = title_failures(pr.get('title') or '')
    if body is not None:
        failures += body_failures(body)
    if a.commits:
        try:
            with open(a.commits, encoding='utf-8') as f:
                failures += commit_failures(f.read())
        except OSError as e:
            sys.exit(f'pr-shape: cannot read {a.commits}: {e.strerror}')
    for failure in failures:
        print(failure)
    checked = ['title'] + (['body'] if body is not None else []) + (['commits'] if a.commits else [])
    if failures:
        print(f'pr-shape: {len(failures)} failure(s)')
    elif len(checked) > 1:
        print(f'pr-shape: the {", ".join(checked[:-1])} and {checked[-1]} keep their shape')
    else:
        print(f'pr-shape: the {checked[0]} keeps its shape')
    sys.exit(1 if failures else 0)


if __name__ == '__main__':
    main()
