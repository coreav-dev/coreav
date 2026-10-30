#!/usr/bin/env python3
"""No public file names an issue or pull request by a bare number.

The code here started in another repository, whose issue and pull
request numbers were cited as "#123", "issue 123", "PR #123" or
"pull request 12" in comments, tests, scripts, notes and file names. Those
numbers mean nothing here, or name an unrelated issue of this repository,
and they are not to be published. A number alone cannot tell the two
repositories apart, as the same number names an issue in each, so the
check goes by form: a reference to an issue or pull request of this
repository names the repository, as coreav-dev/coreav#56 or its URL, and
every other form fails.

Fails on, in every tracked text file and every tracked path:
- a bare "#N" (N from 1);
- "issue N", "issue #N", "issue-N", "PR #N", "prN", "pull request N" and
  the like, also when the name is wrapped onto the next line.
Machine output that numbers things with "#" (libFuzzer's status lines,
uname's build number, ffmpeg's stream specifiers, ffprobe's read
intervals, the host gate's run counters) is not a reference, and neither
is the salt of a held-out draw, whose text fixes the draw. Files whose
data are pull request or commit text, where GitHub reads "#N" as this
repository's, are listed in EXEMPT. Compressed and binary files are not
read. A number alone in a file name ("summary-123.json") is not caught:
names hold counts, dates and sizes too.

    python3 -I scripts/issue_refs_check.py [PATH...]

Prints each match as path:line: text, and the fix. Exit status 0 when
nothing matches, 1 otherwise. Python standard library only.
"""
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]

FIX = ('say what the issue or pull request is about instead of its number; '
       'one of this repository is written coreav-dev/coreav#N (docs/design/ci-checks.md, "Issue references")')

NAMED = ('an issue or pull request named by number',
         re.compile(r'\b(?:issues?|pull requests?|pulls?|prs?|tickets?)[-_ ]?#?[1-9]\d*\b(?![.,]\d)', re.I))
KINDS = [
    ('a bare issue or pull request number', re.compile(r'(?<![\w&/%+#])#[1-9]\d*\b(?!:\d)')),
    NAMED,
]

NOT_REFERENCES = re.compile(
    r'^#\d+\t'
    r'|#\d+ SMP\b'
    r'|\b(?:sd|720p|1080p|4k) #\d'
    r'|\bcodec #\d'
    r'|\bholdout-issue\d+:')

EXEMPT = {
    '.github/workflows/tests/test_pr_redirect.py': 'pull request bodies and markers',
    'docs/measurements/h264-rounds/gcp-1t-fixed/calibration.py': "the host gate's run counters",
    'scripts/pr_shape_check_selftest.py': 'pull request titles and commit messages',
    'scripts/pr_text_check_selftest.py': 'pull request text',
    'scripts/issue_refs_check.py': 'the forms this check refuses',
    'scripts/issue_refs_check_selftest.py': 'the forms this check refuses',
}

PATH_KIND = ('a file name with an issue or pull request number',
             re.compile(r'(?:^|[-_./])(?:issue|pr)[-_]?\d+(?=$|[-_./])', re.I))


def matches(text):
    """(line number, kind, matched text) of each reference in text."""
    out = []
    tail = ''
    for n, line in enumerate(text.split('\n'), 1):
        line = NOT_REFERENCES.sub('', line)
        for kind, pattern in KINDS:
            out += [(n, kind, m.group(0)) for m in pattern.finditer(line)]
        # A name wrapped onto the next line ("pull\nrequest 17"): the last
        # two words of the line before, joined to this line's text.
        joined = tail + ' ' + line.lstrip(' \t#/*!>;-')
        out += [(n, NAMED[0], m.group(0)) for m in NAMED[1].finditer(joined) if m.start() < len(tail) < m.end()]
        tail = ' '.join(re.findall(r'[\w#]+', line)[-2:])
    return out


def tracked(paths):
    out = subprocess.run(['git', 'ls-files', '-z', '--', *paths], cwd=ROOT, check=True,
                         capture_output=True).stdout.decode('utf-8').split('\0')
    return [p for p in out if p]


def text_of(path):
    """The file's text, or None for a binary file or one that is not UTF-8."""
    data = (ROOT / path).read_bytes()
    if b'\0' in data:
        return None
    try:
        return data.decode('utf-8')
    except UnicodeDecodeError:
        return None


def check(paths):
    found = []
    for path in paths:
        if PATH_KIND[1].search(path):
            found.append((path, 0, PATH_KIND[0], path))
        if path in EXEMPT or not (ROOT / path).is_file():
            continue
        text = text_of(path)
        if text is not None:
            found += [(path, n, kind, s) for n, kind, s in matches(text)]
    return found


def main():
    found = check(tracked(sys.argv[1:]))
    for path, n, kind, s in found:
        print(f'{path}:{n}: {kind}: {s!r}')
    if found:
        print(f'issue-refs: {len(found)} match(es): {FIX}')
        sys.exit(1)
    print('issue-refs: no issue or pull request named by a bare number')


if __name__ == '__main__':
    main()
