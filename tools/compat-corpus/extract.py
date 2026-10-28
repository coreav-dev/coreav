#!/usr/bin/env python3
"""Extracts the command-coverage set from the snapshots `collect.py` took
(tools/compat-corpus/commands-v1/README.md).

Reads the code blocks of every answer, finds the `ffmpeg` and `ffprobe`
invocations in them and writes one record per distinct invocation, with
where it came from and what `classify.py` makes of it.

    extract.py --snapshot snapshot-ffprobe.json --snapshot snapshot-ffmpeg.json \\
        --out tools/compat-corpus/commands-v1/commands.jsonl

The rules, in order, so that the same snapshots always give the same set:

1. Only `<code>` and `<pre>` blocks of answers, never prose, never a
   question: an answer's command is one the site's readers voted on, which
   is what a compatibility command must accept. Answers scoring below 1 are
   dropped.
2. Lines ending in a backslash, or in a caret on a Windows command line,
   are joined with the next line.
3. A leading shell prompt (`$`, `>`, `PS>`) and a leading `sudo` are
   removed, and so is a path or `.exe` on the program name. A line
   starting with `#` is a comment and is dropped, not unquoted.
4. A line is split at the shell operators `|`, `||`, `&&`, `;` and `&`, and
   each part that starts with `ffmpeg` or `ffprobe` is a candidate. A
   redirection (`>`, `>>`, `<`, `2>`, `2>&1`) ends the candidate.
5. The arguments are split as a POSIX shell would (`shlex`). A candidate
   is dropped when it does not split, holds no argument, holds a shell
   substitution (`$(`, backtick), reads as program output (`Copyright (c)`,
   a first argument of `version`), or is a line of some other language
   that happens to name the tool (a bracket or an `=` where an option or a
   file name belongs).
6. The file names are replaced by placeholders (`classify.normalize`): a
   coverage set is about which options are combined, every run substitutes
   a corpus file for each input, and the names in a post are examples.
7. Identical normalised argument lists are one record. Every answer that
   produced it is kept in `sources`, highest score first, so none of them
   goes uncredited and the licence counts are not an artefact of which was
   found first.

Each record: the tool, the normalised argument list, the label
`classify.py` gives it, and every source with its ids, link, score and
content licence. `extract.py` also prints what it dropped and why, which
`commands-v1/README.md` records. Python 3.11 standard library only.
"""
import argparse
import html
import json
import pathlib
import re
import shlex
import sys
from html.parser import HTMLParser

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import classify  # noqa: E402

TOOLS = ('ffmpeg', 'ffprobe')
PROMPT = re.compile(r'\A\s*(?:PS\s*[A-Za-z]:[^>]*>|[$>]\s|\$\Z)\s*')
PROGRAM = re.compile(r'\A(?:[A-Za-z]:)?[\w./\\-]*[/\\]?(ffmpeg|ffprobe)(?:\.exe)?\Z')
OPERATORS = ('||', '&&', '|', ';', '&')
REDIRECT = re.compile(r'\s(?:\d?>>?|\d?<|\d>&\d)\s*\S*')
SUBSTITUTION = ('$(', '`')
OUTPUT = 'Copyright (c)'


class CodeBlocks(HTMLParser):
    """The text of every `code` and `pre` element of an answer body."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.depth = 0
        self.blocks = []
        self.current = []

    def handle_starttag(self, tag, attrs):
        if tag in ('code', 'pre'):
            self.depth += 1

    def handle_endtag(self, tag):
        if tag in ('code', 'pre') and self.depth:
            self.depth -= 1
            if not self.depth:
                self.blocks.append(''.join(self.current))
                self.current = []

    def handle_data(self, data):
        if self.depth:
            self.current.append(data)


def code_blocks(body):
    parser = CodeBlocks()
    parser.feed(body or '')
    parser.close()
    return parser.blocks


def joined_lines(block):
    """The block's lines, with continuations joined: a backslash on a
    POSIX shell, a caret on a Windows one."""
    out, pending = [], ''
    for line in html.unescape(block).splitlines():
        line = line.rstrip()
        if line.endswith(('\\', '^')):
            pending += line[:-1] + ' '
            continue
        out.append(pending + line)
        pending = ''
    if pending:
        out.append(pending)
    return out


def split_operators(line):
    """The line's commands, split at `|`, `||`, `&&`, `;` and `&` outside
    quotes. Splitting the raw text would break a URL's query string and a
    filter graph, both of which hold those characters inside quotes."""
    parts, current, quote, i = [], [], None, 0
    while i < len(line):
        char = line[i]
        if quote:
            current.append(char)
            if char == quote:
                quote = None
            i += 1
            continue
        if char in '\'"':
            quote = char
            current.append(char)
            i += 1
            continue
        found = next((op for op in OPERATORS if line.startswith(op, i)), None)
        if found:
            parts.append(''.join(current))
            current = []
            i += len(found)
            continue
        current.append(char)
        i += 1
    parts.append(''.join(current))
    return parts


def candidates(line):
    """The invocations of one line, as (tool, argument string)."""
    out = []
    if line.lstrip().startswith('#'):
        return out
    for part in split_operators(line):
        part = PROMPT.sub('', part).strip()
        if part.lower().startswith('sudo '):
            part = part[5:].lstrip()
        if not part:
            continue
        head, _, rest = part.partition(' ')
        match = PROGRAM.match(head)
        if not match:
            continue
        rest = REDIRECT.split(' ' + rest)[0].strip()
        out.append((match.group(1), rest))
    return out


def arguments(text):
    """The argument list of a candidate, or None when it is unusable."""
    if any(mark in text for mark in SUBSTITUTION) or OUTPUT in text:
        return None
    try:
        args = shlex.split(text, comments=True, posix=True)
    except ValueError:
        return None
    if not args or args[0] == 'version':
        return None
    # An assignment where the first argument belongs: a line of Java,
    # JavaScript or Python that names the tool, not an invocation.
    if args[0].startswith('='):
        return None
    return args


def answers_of(snapshot):
    for answer in snapshot['answers']:
        if answer.get('score', 0) >= 1 and answer.get('body'):
            yield answer


def extract(snapshots):
    """(records, what was dropped and why)."""
    found, dropped = {}, {}
    for snapshot in snapshots:
        for answer in sorted(answers_of(snapshot),
                             key=lambda a: (-a.get('score', 0), a['answer_id'])):
            for block in code_blocks(answer['body']):
                for line in joined_lines(block):
                    for tool, text in candidates(line):
                        args = arguments(text)
                        if args is None:
                            dropped['not an invocation'] = dropped.get('not an invocation', 0) + 1
                            continue
                        key = (tool, tuple(classify.normalize(tool, args)))
                        source = {
                            'question': answer['question_id'],
                            'answer': answer['answer_id'],
                            'link': f'https://stackoverflow.com/a/{answer["answer_id"]}',
                            'score': answer.get('score', 0),
                            'license': answer.get('content_license'),
                            'tag': snapshot['tag'],
                        }
                        if key in found:
                            # The same answer can arrive through both
                            # queries, when a question carries both tags.
                            if source['answer'] not in {s['answer'] for s
                                                        in found[key]['sources']}:
                                found[key]['sources'].append(source)
                            continue
                        found[key] = {'tool': tool, 'args': list(key[1]), 'sources': [source]}
    records = list(found.values())
    for record in records:
        label, reason = classify.label(record['tool'], record['args'])
        record['label'] = label
        record['reason'] = reason
        record['sources'].sort(key=lambda s: (-s['score'], s['answer']))
    return records, dropped


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--snapshot', type=pathlib.Path, action='append', required=True)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    args = parser.parse_args()
    snapshots = [json.loads(path.read_text(encoding='utf-8')) for path in args.snapshot]
    records, dropped = extract(snapshots)
    order = {'ffprobe': 0, 'ffmpeg': 1}
    records.sort(key=lambda r: (order[r['tool']], -r['sources'][0]['score'],
                                r['sources'][0]['answer'], r['args']))
    with args.out.open('w', encoding='utf-8') as handle:
        for record in records:
            handle.write(json.dumps(record, sort_keys=True) + '\n')
    counts = {}
    for record in records:
        key = (record['tool'], record['label'])
        counts[key] = counts.get(key, 0) + 1
    print(f'{len(records)} commands -> {args.out}')
    for key in sorted(counts):
        print(f'  {key[0]:8} {key[1]:12} {counts[key]}')
    for reason, count in sorted(dropped.items()):
        print(f'  dropped: {reason}: {count}')
    return 0


if __name__ == '__main__':
    sys.exit(main())
