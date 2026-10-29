#!/usr/bin/env python3
"""Ties docs/measurements/compat-ffprobe.md to the B3a evidence it cites,
docs/measurements/compat-ffprobe/b3a-roundN.json
(docs/design/compat-cli.md, "The measurements, fixed before the runs",
"The record").

    record.py
        Writes into the record, for each round N, every per-key block
        (the line `**<container>**: ...:` and the table under it) as
        b3a-roundN.json renders it, then checks the record as --check does.

    record.py --check
        Writes nothing. Exit 1, naming the file line, the round, the key
        and the JSON value, when a per-key block differs from its
        rendering, a key table stands outside a block, the prose quotes a
        figure of a key or a count of keys below the bar that the JSON
        does not hold, the file counts of a round's corpus paragraphs
        differ from its JSON, or a JSON's own rate or below_98 contradicts
        its counts.

Python 3 standard library only.
"""
import argparse
import bisect
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
RECORD = ROOT / 'docs' / 'measurements' / 'compat-ffprobe.md'
BAR = 98
CONTAINERS = {'isobmff': 'ISO base media', 'matroska': 'Matroska and WebM', 'mpegts': 'MPEG-TS'}
NAMES = {'ISO base media': 'isobmff', 'Matroska and WebM': 'matroska', 'Matroska': 'matroska',
         'MPEG-TS': 'mpegts'}
PARTS = {'development': 'development', 'held out': 'holdout'}
ROUND = re.compile(r'^## Round (\d+)$')
CORPUS = '### B3a on the corpus'
HEADING = re.compile(r'^\*\*(ISO base media|Matroska and WebM|MPEG-TS)\*\*:')
SENTENCE_END = re.compile(r'\.(?:\*\*)?\s')
AGAINST = re.compile(r'[Aa]gainst round (\d+)')
JOIN = re.compile(r'(?:,|,? and) (?:stream )?')
TOKEN = re.compile(
    r'(?P<skip>rounds? \d+(?: to \d+)?|#\d+)'
    r'|(?P<container>ISO base media|Matroska(?: and WebM)?|MPEG-TS)'
    r'|(?P<open>\()|(?P<close>\))'
    r'|`(?P<key>[^`]+)`'
    r'|(?P<a>\d+(?:\.\d+)?%?) to (?P<b>\d+(?:\.\d+)?%?)(?: of (?P<n>\d+))?'
    r'|(?P<agree>\d+) of (?P<files>\d+)'
    r'|(?P<rate>\d+\.\d+%)')
FILES = (
    (re.compile(r'(?:^|: |\()(\d+) files ?[(;,]'), 'files compared or not',
     lambda d: d['files']),
    (re.compile(r'(\d+) that `ffprobe`(?: itself)? rejects'), 'files `ffprobe` rejects',
     lambda d: d['by_status']['ffprobe_failed']),
    (re.compile(r'(\d+) that `coreav ffprobe` rejects|`coreav ffprobe` rejects (?:the same )?(\d+)'),
     'files `coreav ffprobe` rejects', lambda d: d['rejected_by_coreav']),
    (re.compile(r'(\d+) (?:of them )?held out\)'), 'held-out files', lambda d: d['holdout_files']),
)
PER_CONTAINER = re.compile(r'(\d+) (ISO base media|Matroska and WebM|MPEG-TS)'
                          r'(?:: (\d+) development, (\d+) held out|: (\d+) and (\d+))?')
COUNT = re.compile(r'(ISO base media|Matroska and WebM|MPEG-TS|development|held out) (\d+) of (\d+)'
                   r'|round (\d+): (\d+)')


def below(entry):
    """A key is below the bar on its unrounded share."""
    return entry['agree'] * 100 < BAR * entry['files']


def rate(agree, files):
    """`agree / files` in percent, with two decimals, or more where two
    would put it on the other side of the bar: 1028/1049 is 97.998%, not
    98.00%."""
    for places in range(2, 10):
        text = f'{100 * agree / files:.{places}f}'
        if (float(text) < BAR) == (agree * 100 < BAR * files):
            break
    return text + '%'


def table_keys(table):
    """The keys the record lists: found in at least five files, tags
    apart (they are in the JSON alone)."""
    return {k: e for k, e in table.items() if e.get('files', 0) >= 5 and '.tags.' not in k}


def render(doc, container):
    """The per-key block of `container`: its counts line, a blank line and
    the table of the keys below the bar, lowest rounded rate first, as the
    tables of every round were ordered, then by name."""
    keys = table_keys(doc[container])
    low = sorted((k for k, e in keys.items() if below(e)),
                 key=lambda k: (keys[k]['rate'], k))
    lines = [f'**{CONTAINERS[container]}**: {len(keys)} keys found in at least five files; '
             f'{len(keys) - len(low)} at or above {BAR}%, {len(low)} below:', '']
    if f'{container}:development' not in doc:
        lines += ['| Key | Files | Agree | Rate |', '|---|---|---|---|']
        for k in low:
            e = keys[k]
            lines.append(f"| `{k}` | {e['files']} | {e['agree']} | {rate(e['agree'], e['files'])} |")
        return lines
    lines += ['| Key | All files | Rate | Development | Held out |', '|---|---|---|---|---|']
    for k in low:
        e = keys[k]
        cells = [f'`{k}`', f"{e['agree']}/{e['files']}", rate(e['agree'], e['files'])]
        for part in ('development', 'holdout'):
            p = doc[f'{container}:{part}'][k]
            cells.append(f"{p['agree']}/{p['files']} ({rate(p['agree'], p['files'])})")
        lines.append('| ' + ' | '.join(cells) + ' |')
    return lines


def consistency(name, doc):
    """A JSON's `rate` is its share rounded to two decimals, and its
    `below_98` the unrounded share against the bar."""
    problems = []
    for table_name, table in doc.items():
        if not isinstance(table, dict):
            continue
        for k, e in table.items():
            if not isinstance(e, dict) or 'files' not in e:
                continue
            rounded = round(100 * e['agree'] / e['files'], 2)
            if e['rate'] != rounded:
                problems.append(f"{name}: {table_name} `{k}`: rate {e['rate']}, but "
                                f"{e['agree']}/{e['files']} rounds to {rounded}")
            if 'below_98' in e and e['below_98'] != below(e):
                problems.append(f"{name}: {table_name} `{k}`: below_98 is {e['below_98']}, but "
                                f"{e['agree']}/{e['files']} is {rate(e['agree'], e['files'])} "
                                f'(a key is below the bar on its unrounded share)')
    return problems


class Record:
    """The record's lines, split into round sections, per-key blocks and
    prose paragraphs, with the evidence of each round."""

    def __init__(self, text, evidence_dir, name):
        self.lines = text.split('\n')
        self.evidence_dir = pathlib.Path(evidence_dir)
        self.name = name
        self.docs = {}
        self.blocks = []
        self.paragraphs = []
        self.stray_tables = []
        round_n, section, para = None, None, []
        i = 0
        while i < len(self.lines):
            line = self.lines[i]
            heading = HEADING.match(line) if round_n else None
            if para and (heading or not line.strip() or line.startswith(('#', '|'))):
                self.paragraphs.append((round_n, section, para))
                para = []
            if line.startswith('## '):
                m = ROUND.match(line)
                round_n, section = (int(m.group(1)) if m else None), None
            elif line.startswith('### '):
                section = line
            if heading:
                end = i + 1
                if end + 1 < len(self.lines) and not self.lines[end].strip() \
                        and self.lines[end + 1].startswith('|'):
                    end += 1
                    while end < len(self.lines) and self.lines[end].startswith('|'):
                        end += 1
                self.blocks.append((round_n, NAMES[heading.group(1)], i, end))
                i = end
                continue
            if line.startswith('| Key |'):
                self.stray_tables.append(i)
            if round_n and line.strip() and not line.startswith(('#', '|')):
                para.append((i + 1, line))
            i += 1
        if para:
            self.paragraphs.append((round_n, section, para))

    def evidence(self, n):
        """(file name, parsed JSON or None) of round `n`."""
        if n not in self.docs:
            path = self.evidence_dir / f'b3a-round{n}.json'
            self.docs[n] = (path.name, json.loads(path.read_text()) if path.exists() else None)
        return self.docs[n]

    def rendered(self):
        """The record with every block as its JSON renders it."""
        lines = list(self.lines)
        for n, container, start, end in reversed(self.blocks):
            _, doc = self.evidence(n)
            if doc is not None:
                lines[start:end] = render(doc, container)
        return '\n'.join(lines)

    def check(self):
        problems = [f'{self.name}:{i + 1}: a per-key table outside a `**<container>**: ...` '
                    f'block of a round, which record.py cannot check' for i in self.stray_tables]
        for n, container, start, end in self.blocks:
            json_name, doc = self.evidence(n)
            if doc is None:
                problems.append(f'{self.name}:{start + 1}: round {n} has a per-key block, '
                                f'but there is no {json_name}')
                continue
            want, have = render(doc, container), self.lines[start:end]
            for j in range(max(len(want), len(have))):
                w = want[j] if j < len(want) else '<no line>'
                h = have[j] if j < len(have) else '<no line>'
                if w == h:
                    continue
                key = re.search(r'`([^`]+)`', w + h)
                about = f', `{key.group(1)}`' if key else ''
                problems.append(
                    f'{self.name}:{start + j + 1}: round {n}, {CONTAINERS[container]}{about}: '
                    f'the record has {h!r}, {json_name} renders {w!r} '
                    f'(tools/compat-corpus/record.py writes the blocks)')
                break
        for n, section, para in self.paragraphs:
            problems.extend(self.check_paragraph(n, para))
            if section == CORPUS:
                problems.extend(self.check_files(n, para))
        for n in sorted({b[0] for b in self.blocks} | {p[0] for p in self.paragraphs if p[0]}):
            name, doc = self.evidence(n)
            if doc is not None:
                problems.extend(consistency(name, doc))
        return problems

    def check_files(self, n, para):
        """The file counts of a paragraph of round `n`'s corpus section: the
        files of the round, those each tool rejects, the held-out files,
        and each container's files (and its development and held-out
        parts), against b3a-roundN.json."""
        json_name, doc = self.evidence(n)
        if doc is None:
            return []
        problems = []
        for lineno, said, what, value in self.figures(para, doc):
            if said != value:
                problems.append(f'{self.name}:{lineno}: round {n}: the text says {said} {what}, '
                                f'{json_name} has {value}')
        return problems

    @staticmethod
    def figures(para, doc):
        """(line, number said, what it counts, the JSON's number) for each
        file count of `FILES` and `PER_CONTAINER` in the paragraph."""
        text, starts = '', []
        for _, line in para:
            starts.append(len(text))
            text += line + ' '

        def lineno(offset):
            return para[bisect.bisect_right(starts, offset) - 1][0]

        for pattern, what, value in FILES:
            for m in pattern.finditer(text):
                said = next(i for i, g in enumerate(m.groups(), 1) if g)
                yield lineno(m.start(said)), int(m.group(said)), what, value(doc)
        for m in PER_CONTAINER.finditer(text):
            c = NAMES[m.group(2)]
            files = doc['per_container_files'][c]
            parts = files if isinstance(files, dict) else {}
            total = sum(parts.values()) if parts else files
            yield lineno(m.start()), int(m.group(1)), f'{CONTAINERS[c]} files', total
            for groups, part in (((3, 5), 'development'), ((4, 6), 'holdout')):
                said = next((g for g in groups if m.group(g)), None)
                if said:
                    yield (lineno(m.start(said)), int(m.group(said)), f'{CONTAINERS[c]} {part} files',
                           parts.get(part))

    def check_paragraph(self, n, para):
        """The figures the prose of round `n` ties to a key, and its counts
        of keys below the bar, against b3a-roundN.json. In `X to Y`, Y is
        round n and X the round before, or the round an `against round K`
        of the paragraph names. A key named without a dot is a stream key
        (stream `bit_rate`); the container is the last one the paragraph
        names before the key. Keys joined by a comma or "and" share the
        figures that follow them, up to the next key or the parenthesis
        that closes around them. In a sentence that says "below 98%",
        `<container> N of M` (or `development`, `held out` after it) is the
        count of the container's keys below the bar, and `round K: N` that
        count in round K."""
        json_name, doc = self.evidence(n)
        if doc is None:
            return []
        text, starts = '', []
        for _, line in para:
            starts.append(len(text))
            text += line + ' '
        against = AGAINST.search(text)
        before = int(against.group(1)) if against else n - 1
        ends = [m.end() for m in SENTENCE_END.finditer(text)]
        problems = []

        def where(offset):
            return f'{self.name}:{para[bisect.bisect_right(starts, offset) - 1][0]}'

        def compare(offset, said, k, c, m, field):
            name, d = self.evidence(m)
            entry = d[c].get(k) if d else None
            if entry is None:
                problems.append(f'{where(offset)}: round {n}, {CONTAINERS[c]}, `{k}`: the text '
                                f'quotes {said} for round {m}, where there is no {name} or no such key')
                return
            share = rate(entry['agree'], entry['files'])
            value = {'rate': share, 'agree': str(entry['agree']), 'files': str(entry['files'])}[field]
            if said != value:
                of_round = '' if m == n else f' (round {m})'
                problems.append(f"{where(offset)}: round {n}, {CONTAINERS[c]}, `{k}`: the text says "
                                f"{said}, {name} has {entry['agree']}/{entry['files']} = {share}{of_round}")

        container, group, sentence, depth, group_depth, last_key = None, [], 0, 0, 0, 0
        for t in TOKEN.finditer(text):
            at = bisect.bisect_right(ends, t.start())
            if at != sentence:
                sentence, group, depth = at, [], 0
            if t.group('container'):
                container = NAMES[t.group('container')]
            elif t.group('open'):
                depth += 1
            elif t.group('close'):
                depth -= 1
                if depth < group_depth:
                    group = []
            elif t.group('key'):
                k = t.group('key') if '.' in t.group('key') else f"streams.{t.group('key')}"
                if not any(k in table for table in doc.values() if isinstance(table, dict)):
                    continue
                if container is None:
                    problems.append(f'{where(t.start())}: round {n}, `{k}`: the paragraph names no '
                                    f'container before the key, so its figures cannot be checked')
                    continue
                if not group or not JOIN.fullmatch(text[last_key:t.start()]):
                    group, group_depth = [], depth
                group.append((k, container))
                last_key = t.end()
            elif group and not t.group('skip'):
                for k, c in group:
                    if t.group('rate'):
                        compare(t.start(), t.group('rate'), k, c, n, 'rate')
                    elif t.group('agree'):
                        compare(t.start('agree'), t.group('agree'), k, c, n, 'agree')
                        compare(t.start('files'), t.group('files'), k, c, n, 'files')
                    else:
                        for g, m in (('a', before), ('b', n)):
                            said = t.group(g)
                            compare(t.start(g), said, k, c, m, 'rate' if said.endswith('%') else 'agree')
                        if t.group('n'):
                            compare(t.start('n'), t.group('n'), k, c, n, 'files')

        for s, e in zip([0] + ends, ends + [len(text)]):
            if f'below {BAR}%' not in text[s:e]:
                continue
            c = None
            for m in COUNT.finditer(text, s, e):
                if m.group(4):
                    if c is None:
                        continue
                    earlier, low = int(m.group(4)), int(m.group(5))
                    name, d = self.evidence(earlier)
                    counted = sum(below(x) for x in table_keys(d[c]).values()) if d else None
                    if low != counted:
                        problems.append(f'{where(m.start(5))}: round {n}, {CONTAINERS[c]}: the text says '
                                        f'{low} keys below {BAR}% in round {earlier}, {name} has {counted}')
                    continue
                label, low, total = m.group(1), int(m.group(2)), int(m.group(3))
                if label in NAMES:
                    c, part = NAMES[label], ''
                elif c is None:
                    continue
                else:
                    part = PARTS[label]
                keys = table_keys(doc[f'{c}:{part}' if part else c])
                counted = sum(below(x) for x in keys.values())
                if (low, total) != (counted, len(keys)):
                    problems.append(f'{where(m.start(2))}: round {n}, {CONTAINERS[c]} {part}: the text '
                                    f'says {low} of {total} keys below {BAR}%, {json_name} has '
                                    f'{counted} of {len(keys)}'.replace(' :', ':'))
        return problems


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--record', type=pathlib.Path, default=RECORD)
    opts = parser.parse_args()
    path = opts.record.resolve()
    name = str(path.relative_to(ROOT) if path.is_relative_to(ROOT) else path)
    record = Record(path.read_text(), path.parent / 'compat-ffprobe', name)
    if not opts.check:
        path.write_text(record.rendered())
        record = Record(path.read_text(), path.parent / 'compat-ffprobe', name)
    problems = record.check()
    for p in problems:
        print(p)
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
