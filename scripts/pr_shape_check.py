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
  sections, or with a paragraph or list item broken across two lines
  (a hard wrap).

    pr_shape_check.py --event EVENT.json [--commits FILE]
    pr_shape_check.py --title TITLE [--body-file BODY.md] [--commits FILE]

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

# The sections .github/pull_request_template.md opens with `## `, split by
# whether it tells the author to keep them. A section whose comment says
# "delete this section" is optional and the rest are required. The test reads
# the template, takes that comment as the marker, and fails when these two
# lists no longer hold its level-2 headings.
REQUIRED_SECTIONS = ('TL;DR', 'Background', 'What changes', 'Decisions', 'Evidence')
OPTIONAL_SECTIONS = ('How it works', 'Needs the maintainer')

# The block structure of a Markdown body, from CommonMark's and GitHub
# Flavored Markdown's rules for where a block starts and where it ends: a
# line break inside a paragraph or list item is the hard wrap this check
# reports, and every other block keeps its own line breaks. Each constant
# below names the rule it reads.
FENCE_OPEN = re.compile(r'\A {0,3}(`{3,}|~{3,})(.*)\Z')
FENCE_CLOSE = re.compile(r'\A {0,3}(`{3,}|~{3,})[ \t]*\Z')
HTML_BLOCK = re.compile(r'\A {0,3}(?:<!--|</?[A-Za-z][A-Za-z0-9-]*(?:[ \t/>]|\Z)|<[!?])')
# CommonMark gives the HTML blocks that start a line their own end markers;
# the rest, a block-level tag's line among them, end at the next blank line.
HTML_ENDS = (('<!--', '-->'), ('<![cdata[', ']]>'), ('<?', '?>'), ('<!', '>'),
             ('<script', '</script>'), ('<pre', '</pre>'), ('<style', '</style>'),
             ('<textarea', '</textarea>'))
HEADING = re.compile(r'\A {0,3}(#{1,6})(?:[ \t]+(.*))?\Z')
HEADING_CLOSE = re.compile(r'[ \t]+#+[ \t]*\Z')
QUOTE = re.compile(r'\A {0,3}>')
LIST_ITEM = re.compile(r'\A\s*(?:[-*+]|\d{1,9}[.)])(?:[ \t]|\Z)')
THEMATIC_BREAK = re.compile(r'\A {0,3}(?:(?:\*[ \t]*){3,}|(?:_[ \t]*){3,}|(?:-[ \t]*){3,})\Z')
SETEXT_UNDERLINE = re.compile(r'\A {0,3}(?:=+|-+)[ \t]*\Z')
TABLE_CELL = re.compile(r'[ \t]*:?-+:?[ \t]*')
INDENTED_CODE = re.compile(r'\A(?: {4}|\t)')


def fence_opens(text):
    """The opening fence of a code block, or None. A backtick fence's info
    string cannot hold a backtick; a tilde fence's may hold anything."""
    found = FENCE_OPEN.match(text)
    if not found or found.group(1)[0] == '`' and '`' in found.group(2):
        return None
    return found.group(1)


def fence_closes(text, fence):
    """Whether `text` closes a block opened by `fence`: the same character,
    at least as long, and nothing but spaces after it."""
    found = FENCE_CLOSE.match(text)
    return bool(found) and found.group(1)[0] == fence[0] and len(found.group(1)) >= len(fence)


def html_end(text):
    """Where the HTML block starting at `text` ends: its own marker, or None
    when only a blank line ends it."""
    low = text.lstrip().lower()
    for start, end in HTML_ENDS:
        if low.startswith(start):
            return end
    return None


def heading_text(found):
    """A heading's text, without the closing sequence of `#`s, which has to
    be preceded by a space and is otherwise the heading's own text."""
    return HEADING_CLOSE.sub('', found.group(2) or '').strip()


def is_table_delimiter(text):
    """GitHub's delimiter row: cells of dashes with optional alignment
    colons, separated by pipes, and at least one pipe among them."""
    body = text.strip()
    if '|' not in body or '-' not in body:
        return False
    body = body[1:] if body.startswith('|') else body
    body = body[:-1] if body.endswith('|') else body
    cells = body.split('|')
    return all(TABLE_CELL.fullmatch(cell) for cell in cells)


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
    headings, wraps = scan_body(body)
    normalized = {' '.join(text.split()).casefold() for text, _ in headings}
    out = []
    for section in REQUIRED_SECTIONS:
        if section.casefold() not in normalized:
            out.append(f'the body has no `## {section}` section: the template requires one, so write that heading '
                       f'and the section it opens')
    for start, end in wraps:
        out.append(f'the body breaks a paragraph or list item across lines {start} and {end}: join them into one '
                   'line, and use a blank line where a new paragraph starts')
    return out


def scan_body(body):
    """(heading text, line) of each heading, and (line, line) of each paragraph
    or list item a line break runs through."""
    headings, wraps = [], []
    fence = None    # the opening fence, until its own marker closes it
    html = False    # an HTML block, and `ends_at` holds where it ends
    ends_at = None
    code = False    # an indented code block, until a line indented less
    table = False   # a table's rows, from its delimiter row to a blank line
    quote = False   # a block quote, whose lazy continuation lines have no marker
    open_ = None    # the line the paragraph under way started at
    broken = None   # (its line, the line that continued it) once one does

    def close(exempt):
        """End the paragraph, keeping its line break unless the line that
        ends it exempts the paragraph's own last line (`exempt`)."""
        nonlocal open_, broken
        if broken and not exempt:
            wraps.append(broken)
        open_ = broken = None

    for number, line in enumerate(body.split('\n'), 1):
        text = line.rstrip()
        stripped = text.strip()
        if fence:
            if fence_closes(text, fence):
                fence = None
            continue
        if html:
            ended = not stripped if ends_at is None else ends_at in text
            if ended:
                html = False
            continue
        if code:
            if stripped and not INDENTED_CODE.match(text):
                code = False
            else:
                continue
        if not stripped:
            close(False)
            table = quote = False
            continue
        found = fence_opens(text)
        if found:
            close(False)
            fence = found
            table = quote = False
            continue
        if HTML_BLOCK.match(text):
            close(False)
            ends_at = html_end(text)
            html = ends_at is None or ends_at not in text
            table = quote = False
            continue
        found = HEADING.match(text)
        if found:
            close(False)
            table = quote = False
            if len(found.group(1)) == 2:
                headings.append((heading_text(found), number))
            continue
        if QUOTE.match(text):
            close(False)
            table = False
            quote = True
            continue
        setext = bool(SETEXT_UNDERLINE.match(text))
        if setext or THEMATIC_BREAK.match(text):
            # `---` and `===` under a paragraph make a setext heading, whose
            # text is that paragraph, so a reader sees the lines above it as
            # one line. A `***` or `___` rule is a break of its own and keeps
            # them broken.
            close(setext)
            table = quote = False
            continue
        if LIST_ITEM.match(text):
            close(False)
            table = quote = False
            open_ = number
            continue
        if table or quote:
            continue        # a table row, or a quote's lazy continuation
        if is_table_delimiter(text):
            close(True)     # the delimiter row makes the line above the table's header
            table, quote = True, False
            continue
        if open_ is None and INDENTED_CODE.match(text):
            code = True     # indented code, which a paragraph's line cannot start
            continue
        if open_ is None:
            open_ = number
        elif broken is None:
            broken = (open_, number)
    close(False)
    return headings, wraps


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
