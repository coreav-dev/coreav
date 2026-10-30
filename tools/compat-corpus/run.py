#!/usr/bin/env python3
"""Runs the command-coverage set against `coreav ffprobe` and the pinned
`ffprobe` (docs/design/compat-cli.md, "The measurements").

    run.py commands --coreav PATH --media DIR [DIR...] [--report FILE]
        Every `ffprobe` command of commands-v1 on every file of the
        directories (a MANIFEST with `generated NAME` lines selects the
        files when present). `out-of-scope` commands: coreav must exit
        non-zero, print a message on standard error and nothing on standard
        output (B1). `native` commands: coreav must exit 0 (B2) and print
        what ffprobe prints, byte for byte, on every file ffprobe reads (B3a
        on the fixtures). Exit 1 when any bar is missed.

    run.py keys --coreav PATH --media DIR [DIR...] [--list FILE] [--report FILE]
        Per file, both tools print `-show_format -show_streams
        -show_chapters -show_programs` and `-count_packets -show_packets`
        in JSON; every key ffprobe prints is compared (B3a on the corpus).
        Reports, per key, the share of files it appears in on which every
        occurrence agrees.

ffprobe is a black box: only its output is read. Python 3 standard
library only.
"""
import argparse
import collections
import concurrent.futures
import functools
import importlib.util
import json
import os
import pathlib
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
COMMANDS = HERE / 'commands-v1' / 'commands.jsonl'
TIMEOUT = 600
# `coreav` in strict-safe mode: a command it does not run natively is
# refused, never handed to the `ffprobe` on PATH, which would agree with
# itself (docs/design/compat-cli.md, "Fallback to FFmpeg").
ENV = {**os.environ, 'COREAV_STRICT_SAFE': '1'}


def media_files(dirs):
    out = []
    for d in dirs:
        d = pathlib.Path(d)
        if d.is_file():
            out.append(d)
            continue
        manifest = d / 'MANIFEST'
        if manifest.exists():
            for line in manifest.read_text().splitlines():
                parts = line.split()
                if len(parts) >= 2 and parts[0] == 'generated':
                    out.append(d / parts[1])
        else:
            out.extend(sorted(p for p in d.iterdir()
                              if p.is_file() and p.suffix not in ('.json', '.log', '')))
    return out


def run(cmd):
    try:
        p = subprocess.run(cmd, capture_output=True, timeout=TIMEOUT, env=ENV)
        return p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        return None, b'', b'timeout'


def substitute(args, path):
    return [str(path) if a == 'INPUT' else a for a in args]


def load_commands():
    with open(COMMANDS) as f:
        return [json.loads(line) for line in f if line.strip()]


# Fixtures crafted to show a documented divergence: the record, and the keys
# it names. A difference on such a fixture is set aside, and counted apart,
# only when every key the two tools print differently over the whole file
# (the stream listing and the packet listing of `run.py keys`) is one the
# record names; `packets.*` stands for whole packets, which the record lets
# one side list and the other not. Any other difference fails.
DECLARED_SIZE = ('KD-DECLARED-SIZE (docs/known-divergences.md)',
                 {'streams.width', 'streams.height', 'streams.display_aspect_ratio'})
# ffprobe drops the parameter set, so it prints none of what the set gives.
HEVC_WINDOW = ('KD-HEVC-WINDOW (docs/known-divergences.md)',
               {'streams.profile', 'streams.level', 'streams.color_range', 'streams.pix_fmt',
                'streams.chroma_location', 'streams.coded_width', 'streams.coded_height',
                'streams.has_b_frames'})
SET_ASIDE = {
    'size_entry_understated.mp4': DECLARED_SIZE,
    'size_prores_understated.mov': DECLARED_SIZE,
    'size_hevc_window.mp4': HEVC_WINDOW,
    'size_hevc_mixed_window.mp4': HEVC_WINDOW,
    'ts_duplicates.ts': ('KD-TS-DUPLICATE (docs/known-divergences-ts.md)',
                         {'packets.*', 'streams.bit_rate', 'programs.streams.bit_rate'}),
}


def covered_by_record(coreav, path):
    """The record that covers every difference of `path`, or None. Keys
    CoreAV prints and ffprobe does not, and a packet listing of another
    length, count as differences too."""
    record, fields = SET_ASIDE.get(path.name, (None, set()))
    if record is None:
        return None
    fields = fields | set(ffmpeg9().get(path.name, {}))
    _, ref_out, _ = run(['ffprobe'] + SECTION_ARGS + [str(path)])
    code, out, _ = run([coreav, 'ffprobe'] + SECTION_ARGS + [str(path)])
    try:
        extra = {key_name(k) for k in set(flatten(json.loads(out))) - set(flatten(json.loads(ref_out)))}
    except ValueError:
        return None
    extra -= fields
    if code != 0 or extra:
        return None
    if 'packets.*' not in fields:
        listings = [run(tool + PACKET_ARGS + [str(path)])[1].count(b'\n')
                    for tool in (['ffprobe'], [coreav, 'ffprobe'])]
        if listings[0] != listings[1]:
            return None
    for args in (SECTION_ARGS, PACKET_ARGS):
        r = compare_file(coreav, path, args)
        if r is None or r[2] != 0:
            return None
        for name, ok in r[0].items():
            if ok or name in fields or ('packets.*' in fields and name.startswith('packets.')):
                continue
            return None
    return record


def cmd_commands(opts):
    files = media_files(opts.media)
    records = [r for r in load_commands() if r['tool'] == 'ffprobe']
    aside = {p.name: covered_by_record(opts.coreav, p) for p in files if p.name in SET_ASIDE}
    set_aside = collections.Counter()

    b1 = collections.Counter()
    b2 = collections.Counter()
    b3 = collections.Counter()
    failures = []
    for r in records:
        if r['label'] == 'unusable':
            continue
        args = r['args']
        if r['label'] == 'out-of-scope':
            # A refusal does not depend on the file: one is enough.
            target = files[0] if files else 'INPUT'
            code, out, err = run([opts.coreav, 'ffprobe'] + substitute(args, target))
            ok = code not in (0, None) and not out and err.strip()
            b1['pass' if ok else 'fail'] += 1
            if not ok:
                failures.append({'bar': 'B1', 'args': args, 'exit': code,
                                 'stdout': out[:200].decode('utf-8', 'replace')})
            continue
        def both(path):
            argv = substitute(args, path)
            ref_code, ref_out, _ = run(['ffprobe'] + argv)
            if ref_code != 0:
                return path, None, (None, None)
            got = run([opts.coreav, 'ffprobe'] + argv)
            if got[0] != 0 or got[1] == ref_out or aside.get(path.name):
                return path, (ref_out,) + got, (None, None)
            return path, (ref_out,) + got, ffmpeg9_covers(opts.coreav, args, path, ref_out, got[1])
        with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
            pairs = list(pool.map(both, files))
        for path, pair, (entries, keys) in pairs:
            if pair is None:
                continue
            ref_out, code, out, err = pair
            if code != 0:
                b2['fail'] += 1
                failures.append({'bar': 'B2', 'args': args, 'file': path.name, 'exit': code,
                                 'stderr': err[:300].decode('utf-8', 'replace')})
                continue
            b2['pass'] += 1
            if out == ref_out:
                b3['pass'] += 1
            elif aside.get(path.name):
                b3['set aside'] += 1
                set_aside[aside[path.name]] += 1
            elif entries:
                b3['set aside'] += 1
                set_aside.update(entries)
            else:
                b3['fail'] += 1
                failures.append({'bar': 'B3', 'args': args, 'file': path.name,
                                 'diff': first_difference(ref_out, out), 'keys': keys})
    b1_ffmpeg, _ = ffmpeg_refusals(opts, files, collections.Counter(), failures)
    report = {'B1': dict(b1), 'B1 ffmpeg': dict(b1_ffmpeg), 'B2': dict(b2), 'B3': dict(b3),
              'set aside': dict(set_aside), 'failures': failures}
    if opts.report:
        pathlib.Path(opts.report).write_text(json.dumps(report, indent=1))
    print(json.dumps({k: report[k] for k in ('B1', 'B1 ffmpeg', 'B2', 'B3', 'set aside')}))
    by = collections.Counter((f['bar'], ' '.join(f['args'])) for f in failures)
    for (bar, args), n in by.most_common(opts.top):
        print(f'{bar} x{n}: {args}')
    found = sorted({failure_key(f) for f in failures})
    if opts.write_baseline:
        pathlib.Path(opts.write_baseline).write_text(json.dumps(
            {'note': 'failing (bar, command, fixture) triples of tools/compat-corpus/run.py '
                     'commands on the generated fixtures; CI fails on any other',
             'failing': found}, indent=1) + '\n')
    if opts.baseline:
        known = set(json.loads(pathlib.Path(opts.baseline).read_text())['failing'])
        new = [k for k in found if k not in known]
        fixed = [k for k in known if k not in set(found)]
        by_key = {failure_key(f): f for f in failures}
        for k in new[:opts.top]:
            print(f'NEW FAILURE: {k}')
            detail = by_key.get(k, {})
            if 'diff' in detail or 'stderr' in detail or 'exit' in detail:
                print('    ' + json.dumps({x: detail.get(x) for x in ('diff', 'exit', 'stderr') if x in detail})[:600])
        print(f'{len(new)} new failures, {len(fixed)} fixed since the baseline')
        return 1 if new else 0
    return 1 if failures else 0


def failure_key(f):
    """One failure, as the baseline lists it: the bar, the command, and the
    fixture (none for a refusal, which does not depend on the file)."""
    return '|'.join([f['bar'], ' '.join(f['args']), f.get('file', '')])


def ffmpeg_refusals(opts, files, b1, failures):
    """B1 for `coreav ffmpeg`: every out-of-scope `ffmpeg` command of the
    set exits non-zero with a message, prints nothing and writes no output
    file. The output placeholders name files in a new directory, which
    must stay empty."""
    import tempfile
    target = str(files[0]) if files else 'INPUT'
    for r in load_commands():
        if r['tool'] != 'ffmpeg' or r['label'] != 'out-of-scope':
            continue
        with tempfile.TemporaryDirectory() as tmp:
            argv = []
            for a in r['args']:
                if a.startswith('INPUT'):
                    argv.append(target)
                elif a in ('OUTPUT', 'FILE'):
                    argv.append(str(pathlib.Path(tmp) / f'{a.lower()}.mp4'))
                else:
                    argv.append(a)
            code, out, err = run([opts.coreav, 'ffmpeg'] + argv)
            ok = code not in (0, None) and not out and err.strip() and not any(pathlib.Path(tmp).iterdir())
        b1['pass' if ok else 'fail'] += 1
        if not ok:
            failures.append({'bar': 'B1', 'tool': 'ffmpeg', 'args': r['args'], 'exit': code})
    return b1, failures


def first_difference(a, b):
    la, lb = a.decode('utf-8', 'replace').splitlines(), b.decode('utf-8', 'replace').splitlines()
    for i in range(max(len(la), len(lb))):
        x = la[i] if i < len(la) else '<end>'
        y = lb[i] if i < len(lb) else '<end>'
        if x != y:
            return {'line': i + 1, 'ffprobe': x[:300], 'coreav': y[:300]}
    return {'line': None}


# Differences that come from FFmpeg 9.0.2 itself, not from CoreAV: keys it
# prints that `coreav ffprobe` does not yet print, keys it no longer prints,
# and values it gives otherwise. ffmpeg9.json names each key and the files it
# differs on, under the entry of docs/known-divergences-ffprobe9.md that says
# why. A B3 difference is set aside only when the two outputs are byte for
# byte the same once those keys of that file, and nothing else, are taken out
# of both, in the command's own output format.
FFMPEG9 = HERE / 'ffmpeg9.json'
WRITER_FLAGS = ('-of', '-print_format', '-output_format')
WRITER_OPTIONS = {'nokey': 'nk', 'noprint_wrappers': 'nw', 'print_section': 'p',
                  'item_sep': 's', 'sep_char': 's', 'escape': 'e', 'compact': 'c'}
WRITER_DEFAULTS = {'default': {'nk': '0', 'nw': '0'},
                   'compact': {'nk': '0', 'p': '1', 's': '|', 'e': 'c'},
                   'csv': {'nk': '1', 'p': '1', 's': ',', 'e': 'csv'},
                   'flat': {'s': '.'},
                   'json': {}}
# The options under which a writer shows every key, section name and wrapper.
WRITER_FULL = {'default': {'nk': '0', 'nw': '0'},
               'compact': {'nk': '0', 'p': '1'},
               'csv': {'nk': '0', 'p': '1'},
               'flat': {'s': '.'},
               'json': {}}


@functools.cache
def ffmpeg9():
    """{file name: {key: entry id}} from ffmpeg9.json."""
    out = collections.defaultdict(dict)
    for entry in json.loads(FFMPEG9.read_text())['entries']:
        for key, names in entry['keys'].items():
            for name in names:
                out[name][key] = entry['id']
    return out


def writer_of(args):
    """The writer a command prints with, and its options, defaults included."""
    spec = 'default'
    for flag, value in zip(args, args[1:]):
        if flag in WRITER_FLAGS:
            spec = value
    name, _, rest = spec.partition('=')
    options = dict(WRITER_DEFAULTS.get(name, {}))
    for item in filter(None, rest.split(':')):
        k, _, v = item.partition('=')
        options[WRITER_OPTIONS.get(k, k)] = v
    return name, options


def on(options, name):
    return options.get(name, '0') not in ('0', '')


def full_args(args, name, options):
    """`args` with the writer set to show every key, section name and
    wrapper, or None when it already does."""
    full = {**options, **WRITER_FULL[name]}
    if full == options:
        return None
    spec = name + '=' + ':'.join(f'{k}={v}' for k, v in full.items())
    return [spec if i and args[i - 1] in WRITER_FLAGS else a for i, a in enumerate(args)]


def records(text, escape):
    """(start, line) for each line of a compact or CSV output; a CSV value in
    quotes may hold line ends."""
    out, start, quoted = [], 0, False
    for i, ch in enumerate(text):
        if escape == 'csv' and ch == '"':
            quoted = not quoted
        elif ch == '\n' and not quoted:
            out.append((start, text[start:i]))
            start = i + 1
    return out + [(start, text[start:])]


def split_items(line, sep, escape):
    """A compact or CSV line cut at each separator that is not escaped."""
    items, start, quoted, i = [], 0, False, 0
    while i < len(line):
        ch = line[i]
        if escape == 'c' and ch == '\\':
            i += 2
            continue
        if escape == 'csv' and ch == '"':
            quoted = not quoted
        elif ch == sep and not quoted:
            items.append(line[start:i])
            start = i + 1
        i += 1
    items.append(line[start:])
    return items


def derive(text, name, options):
    """What a writer prints under `options`, from what it prints under
    WRITER_FULL."""
    lines = text.split('\n')
    if name == 'default':
        if on(options, 'nw'):
            lines = [x for x in lines if not (x.startswith('[') and x.endswith(']'))]
        if on(options, 'nk'):
            lines = [x.split('=', 1)[-1] for x in lines]
    elif name in ('compact', 'csv'):
        out = []
        for _, x in records(text, options['e']):
            items = split_items(x, options['s'], options['e']) if x else []
            while items and not on(options, 'p') and '=' not in items[0]:
                items = items[1:]
            if on(options, 'nk'):
                items = [i.split('=', 1)[-1] for i in items]
            out.append(options['s'].join(items))
        lines = out
    elif name == 'flat':
        lines = [x.partition('=')[0].replace('.', options['s']) + ''.join(x.partition('=')[1:])
                 for x in lines]
    return '\n'.join(lines)


def section_key(name):
    """`STREAM` -> `streams`, `FORMAT` -> `format`: a section as
    `key_name` names it."""
    name = name.lower()
    return name if name == 'format' else name + 's'


def leaf_key(path, key):
    for prefix, sub in (('TAG:', 'tags.'), ('DISPOSITION:', 'disposition.'),
                        ('tag:', 'tags.'), ('disposition:', 'disposition.')):
        if key.startswith(prefix):
            key = sub + key[len(prefix):]
    return '.'.join(path + [key])


# A stream group (FFmpeg 9.0.2 makes one of a QuickTime timecode track) is
# one item, `stream_groups`, in every writer.
def side_data_key(path, kind):
    """One side data entry, by its type: `packets.side_data_list[Skip Samples]`."""
    return '.'.join(path + ['side_data_list']) + f'[{kind}]'


def parse_default(text):
    """[(key, start, end)]: each line of a key, and each side data section,
    of the default writer."""
    spans, path, group, opaque, pos = [], [], None, None, 0
    for line in text.split('\n'):
        end = pos + len(line) + 1
        if opaque is not None:
            if line == '[/STREAM_GROUP]':
                spans.append(('stream_groups', opaque, end))
                opaque = None
        elif line == '[STREAM_GROUP]':
            opaque = pos
        elif line.startswith('[') and line.endswith(']'):
            close, section = line.startswith('[/'), line.strip('[/]')
            if section == 'SIDE_DATA':
                if close and group:
                    spans.append((side_data_key(path, group[1]), group[0], end))
                group = None if close else [pos, '']
            elif close:
                path = path[:-1]
            else:
                path = path + [section_key(section)]
        elif group is not None:
            if line.startswith('side_data_type='):
                group[1] = line.split('=', 1)[1]
        elif '=' in line:
            spans.append((leaf_key(path, line.split('=', 1)[0]), pos, end))
        pos = end
    return spans


def parse_compact(text, options):
    """[(key, start, end)]: each item of a key with the separator before it,
    and each side data entry, of the compact and CSV writers."""
    spans, sep, opaque = [], options['s'], None
    for pos, line in records(text, options['e']):
        items = split_items(line, sep, options['e'])
        if opaque is not None or items[0] == 'stream_group':
            opaque = pos if opaque is None else opaque
            if not line:
                spans.append(('stream_groups', opaque, pos + 1))
                opaque = None
            continue
        path = [section_key(items[0])] if items[0] else []
        at, groups = pos + len(items[0]), {}
        for item in items[1:]:
            start, at = at, at + len(sep) + len(item)
            key, eq, value = item.partition('=')
            prefix, colon, rest = key.partition(':')
            if not eq:
                groups[start] = [start, at, '']
            elif colon and prefix.startswith('side_datum/'):
                g = groups.setdefault(prefix, [start, at, ''])
                g[1] = at
                if rest == 'side_data_type':
                    g[2] = value
            else:
                spans.append((leaf_key(path, key), start, at))
        spans += [(side_data_key(path, kind), s, e) for s, e, kind in groups.values()]
    return spans


def parse_flat(text):
    """[(key, start, end)]: each line of a key of the flat writer, and the
    lines of each side data entry together."""
    spans, groups, pos = [], {}, 0
    for line in text.split('\n'):
        end = pos + len(line) + 1
        name, eq, value = line.partition('=')
        parts, path, i, group = name.split('.'), [], 0, None
        while i < len(parts):
            if i + 2 < len(parts) and parts[i + 2].isdigit():
                if parts[i] == 'side_data_list':
                    group = '.'.join(parts[:i + 3])
                    break
                path.append(parts[i])
                i += 3
            else:
                path.append(parts[i])
                i += 1
        if parts[0] == 'stream_groups':
            g = groups.setdefault('stream_groups', [None, pos, end, ''])
            g[2] = end
        elif group:
            g = groups.setdefault(group, [path, pos, end, ''])
            g[2] = end
            if parts[-1] == 'side_data_type':
                g[3] = value.strip('"')
        elif eq:
            spans.append(('.'.join(path), pos, end))
        pos = end
    return spans + [(side_data_key(p, kind) if p is not None else 'stream_groups', s, e)
                    for p, s, e, kind in groups.values()]


class JsonNode:
    def __init__(self, key, start):
        self.key, self.start, self.end = key, start, start
        self.open, self.children, self.value = None, [], None


def parse_json_text(text):
    """The JSON writer's output as a tree; a member's span covers its name
    and its value."""
    pos = 0

    def ws():
        nonlocal pos
        while pos < len(text) and text[pos] in ' \t\r\n':
            pos += 1

    def string():
        nonlocal pos
        start = pos
        pos += 1
        while text[pos] != '"':
            pos += 2 if text[pos] == '\\' else 1
        pos += 1
        return json.loads(text[start:pos])

    def value(node):
        nonlocal pos
        ch = text[pos]
        if ch in '{[':
            node.open, close = ch, '}' if ch == '{' else ']'
            pos += 1
            ws()
            while text[pos] != close:
                child = JsonNode(node.key, pos)
                if ch == '{':
                    name = string()
                    ws()
                    pos += 1
                    ws()
                    child.key = f'{node.key}.{name}' if node.key else name
                value(child)
                child.end = pos
                node.children.append(child)
                ws()
                if text[pos] == ',':
                    pos += 1
                    ws()
            pos += 1
        elif ch == '"':
            node.value = string()
        else:
            start = pos
            while pos < len(text) and text[pos] not in ',}] \t\r\n':
                pos += 1
            node.value = text[start:pos]

    root = JsonNode('', 0)
    ws()
    if pos < len(text):
        value(root)
    root.start, root.end = 0, len(text)
    return root


def json_side_data(node):
    """The side data key of an entry of a `side_data_list`."""
    kind = next((c.value for c in node.children if c.key.endswith('.side_data_type')), '')
    base = node.key[:-len('side_data_list')].rstrip('.')
    return side_data_key([base] if base else [], kind)


def parse_json(text):
    """[(key, start, end)]: each leaf member, and each side data entry, of
    the JSON writer."""
    spans = []

    def walk(node, in_list):
        for child in node.children:
            if in_list:
                spans.append((json_side_data(child), child.start, child.end))
            elif child.key == 'stream_groups':
                spans.append((child.key, child.start, child.end))
            elif child.open:
                walk(child, child.open == '[' and child.key.endswith('side_data_list'))
            else:
                spans.append((child.key, child.start, child.end))
    walk(parse_json_text(text), False)
    return spans


def json_without(text, keys):
    """The JSON writer's output with the members of `keys` taken out, the
    separators between those left kept, and a container left empty taken
    out too."""
    def render(node, in_list):
        if in_list:
            return None if json_side_data(node) in keys else text[node.start:node.end]
        if not node.open or node.key == 'stream_groups':
            return None if node.key in keys else text[node.start:node.end]
        kids = node.children
        if not kids:
            return text[node.start:node.end]
        inner = node.open == '[' and node.key.endswith('side_data_list')
        kept = [(i, r) for i, r in ((i, render(c, inner)) for i, c in enumerate(kids)) if r is not None]
        if not kept:
            return None
        out = [text[node.start:kids[0].start]]
        for j, (i, r) in enumerate(kept):
            if j:
                out.append(text[kids[i - 1].end:kids[i].start])
            out.append(r)
        out.append(text[kids[-1].end:node.end])
        return ''.join(out)
    return render(parse_json_text(text), False) or ''


def item_spans(text, name, options):
    if name == 'default':
        return parse_default(text)
    if name in ('compact', 'csv'):
        return parse_compact(text, options)
    if name == 'flat':
        return parse_flat(text)
    return parse_json(text)


def without(text, name, options, keys):
    """`text` with every item of `keys` taken out."""
    if name == 'json':
        return json_without(text, keys)
    out, at = [], 0
    for key, start, end in sorted(item_spans(text, name, options), key=lambda s: s[1]):
        if key in keys:
            out.append(text[at:start])
            at = end
    return ''.join(out) + text[at:]


def differing_keys(a, b, name, options):
    """The keys whose items differ between two outputs of one writer."""
    def items(text):
        out = collections.defaultdict(list)
        for key, start, end in item_spans(text, name, options):
            out[key].append(text[start:end].strip(' ,\n' + options.get('s', '')))
        return out
    x, y = items(a), items(b)
    return sorted(k for k in set(x) | set(y) if x.get(k) != y.get(k))


def full_outputs(coreav, args, path, ref_out, out):
    """(writer, options, [ffprobe's output, coreav's]) with every key, section
    name and wrapper shown, or None when the writer is not one of
    WRITER_FULL or either tool's output under them does not give back what
    it printed."""
    name, options = writer_of(args)
    if name not in WRITER_FULL:
        return None
    printed = [ref_out.decode('utf-8', 'replace'), out.decode('utf-8', 'replace')]
    full = full_args(args, name, options)
    if full is None:
        return name, options, printed
    texts = []
    for tool, text in zip((['ffprobe'], [coreav, 'ffprobe']), printed):
        code, again, _ = run(tool + substitute(full, path))
        again = again.decode('utf-8', 'replace')
        if code != 0 or derive(again, name, options) != text:
            return None
        texts.append(again)
    return name, {**options, **WRITER_FULL[name]}, texts


def ffmpeg9_covers(coreav, args, path, ref_out, out):
    """For two outputs of one command on one file that differ: the
    ffmpeg9.json entries that explain every difference, or None, and the
    keys that differ, or None when the writer's output cannot be read."""
    found = full_outputs(coreav, args, path, ref_out, out)
    if found is None:
        return None, None
    name, options, (a, b) = found
    keys = differing_keys(a, b, name, options)
    allowed = ffmpeg9().get(path.name, {})
    if not allowed or without(a, name, options, allowed) != without(b, name, options, allowed):
        return None, keys
    return sorted({allowed[k] for k in keys if k in allowed}) or None, keys


SECTION_ARGS = ['-v', 'error', '-of', 'json', '-show_format', '-show_streams',
                '-show_chapters', '-show_programs']
PACKET_ARGS = ['-v', 'error', '-of', 'compact', '-count_packets', '-show_packets']


def flatten(doc):
    """(key path, value) for every leaf; array elements keep their index,
    so streams pair by index."""
    out = {}

    def walk(prefix, node):
        if isinstance(node, dict):
            for k, v in node.items():
                walk(f'{prefix}.{k}' if prefix else k, v)
        elif isinstance(node, list):
            for i, v in enumerate(node):
                walk(f'{prefix}[{i}]', v)
        else:
            out[prefix] = node
    walk('', doc)
    return out


def key_name(path):
    """`streams[1].tags.language` -> `streams.tags.language`."""
    return '.'.join(part.split('[', 1)[0] for part in path.split('.'))


def packet_fields(line):
    """A compact packet line as {key: value}; side data keys under
    `side_data_list.`."""
    out = {}
    for item in line.rstrip('\n').split('|')[1:]:
        if '=' not in item:
            continue
        k, v = item.split('=', 1)
        if ':' in k:
            k = 'side_data_list.' + k.split(':', 1)[1]
        out[k] = v
    return out


def run_to_file(cmd, path):
    with open(path, 'wb') as f:
        try:
            return subprocess.run(cmd, stdout=f, stderr=subprocess.DEVNULL, timeout=TIMEOUT,
                                  env=ENV).returncode
        except subprocess.TimeoutExpired:
            return None


def compare_packets(coreav, path):
    """Packets paired by position, read line by line from both listings, so
    that a long file needs no memory."""
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        ref_path, got_path = pathlib.Path(tmp) / 'ref', pathlib.Path(tmp) / 'got'
        if run_to_file(['ffprobe'] + PACKET_ARGS + [str(path)], ref_path) != 0:
            return None
        code = run_to_file([coreav, 'ffprobe'] + PACKET_ARGS + [str(path)], got_path)
        result, examples = {}, {}
        with open(ref_path, errors='replace') as ref, open(got_path, errors='replace') as got:
            n = 0
            for line in ref:
                other = got.readline() if code == 0 else ''
                a, b = packet_fields(line), packet_fields(other)
                for k, v in a.items():
                    name = 'packets.' + k
                    ok = b.get(k) == v
                    result[name] = result.get(name, True) and ok
                    if not ok and name not in examples:
                        examples[name] = (f'packets[{n}].{k}', v, b.get(k))
                n += 1
        return result, examples, code


def compare_file(coreav, path, args):
    if args is PACKET_ARGS:
        return compare_packets(coreav, path)
    ref_code, ref_out, _ = run(['ffprobe'] + args + [str(path)])
    if ref_code != 0:
        return None
    code, out, err = run([coreav, 'ffprobe'] + args + [str(path)])
    try:
        ref = flatten(json.loads(ref_out))
    except ValueError:
        return None
    try:
        got = flatten(json.loads(out)) if code == 0 else {}
    except ValueError:
        got = {}
    result = {}
    examples = {}
    for k, v in ref.items():
        name = key_name(k)
        ok = got.get(k) == v
        result[name] = result.get(name, True) and ok
        if not ok:
            examples.setdefault(name, (k, v, got.get(k)))
    return result, examples, code


STRICT = 'KD-STRICT (docs/known-divergences.md)'


def strict_rejection(coreav, path):
    """Whether KD-STRICT covers CoreAV rejecting `path`: `ffprobe` reads the
    file as ISO base media (`format_name` `mov,mp4,...`, whatever its
    extension), `coreav probe` names the kind of defect (truncated or
    invalid structure, a limit), and the file's bytes independently show
    that kind, read by the walk of `scripts/oracle_kd.py` that the library
    oracle uses for the same record. A valid file CoreAV rejects is never
    covered, nor a file of another container, whose first bytes the box
    walk would read as a box past the end."""
    code, out, _ = run(['ffprobe', '-v', 'error', '-of', 'json', '-show_entries',
                        'format=format_name', str(path)])
    try:
        name = json.loads(out)['format']['format_name'] if code == 0 else ''
    except (ValueError, KeyError, TypeError):
        return False
    if not name.startswith('mov,mp4'):
        return False
    _, _, err = run([coreav, 'probe', str(path)])
    try:
        kind = json.loads(err).get('kind')
    except (ValueError, AttributeError):
        return False
    kd = oracle_kd()
    evidence = kd.EVIDENCE.get(kind, set())
    return bool(evidence and evidence & kd.media_defects(path.parent, path.name))


@functools.cache
def oracle_kd():
    """`scripts/oracle_kd.py`, from the repository or beside this file."""
    for d in (HERE.parent.parent / 'scripts', HERE):
        if (d / 'oracle_kd.py').exists():
            spec = importlib.util.spec_from_file_location('oracle_kd', d / 'oracle_kd.py')
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            return module
    raise FileNotFoundError('oracle_kd.py')


def compare_all(coreav, path):
    """Both listings of `path` compared key by key: (keys, examples, failed,
    record, covered), or None when ffprobe reads neither. `keys` maps each key
    ffprobe prints to whether every occurrence agrees. When `coreav
    ffprobe` rejects the file (exit 1, its status for invalid input) and a
    known divergence covers the rejection, `record` names it and the keys
    of the listings it rejected (`covered`) agree through it (they stay in
    the denominator); else `record` is None and `covered` empty. `failed`:
    a listing exited other than 0 that `record` does not cover."""
    keys, examples, other, rejected = {}, {}, False, set()
    for args in (SECTION_ARGS, PACKET_ARGS):
        r = compare_file(coreav, path, args)
        if r is None:
            continue
        result, ex, code = r
        if code == 1:
            rejected |= set(result)
        elif code != 0:
            other = True
        for name, ok in result.items():
            keys[name] = keys.get(name, True) and ok
        for name, e in ex.items():
            examples.setdefault(name, e)
    if not keys:
        return None
    record = STRICT if rejected and strict_rejection(coreav, path) else None
    covered = rejected if record else set()
    for name in covered:
        keys[name] = True
        examples.pop(name, None)
    return keys, examples, other or bool(rejected and not record), record, covered


def cmd_keys(opts):
    files = media_files(opts.media)
    if opts.list:
        files = [pathlib.Path(line.strip()) for line in open(opts.list) if line.strip()]
    seen = collections.Counter()
    agree = collections.Counter()
    covered = collections.Counter()
    examples = collections.defaultdict(list)
    rejected = []
    with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
        results = list(pool.map(lambda path: (path, compare_all(opts.coreav, path)), files))
    for path, r in results:
        if r is None:
            continue
        result, ex, failed, record, names = r
        if failed or record:
            rejected.append({'file': str(path), 'covered_by': None if failed else record})
        for name, ok in result.items():
            seen[name] += 1
            agree[name] += ok
            covered[name] += name in names
        for name, e in ex.items():
            if len(examples[name]) < 5:
                examples[name].append({'file': path.name, 'path': e[0],
                                       'ffprobe': e[1], 'coreav': e[2]})
    rows = []
    for name in sorted(seen, key=lambda n: (agree[n] / seen[n], n)):
        rows.append({'key': name, 'files': seen[name], 'agree': agree[name],
                     'via_known_divergence': covered[name],
                     'rate': round(100.0 * agree[name] / seen[name], 2)})
    report = {'files': len(files), 'rejected': rejected, 'keys': rows,
              'examples': examples}
    if opts.report:
        pathlib.Path(opts.report).write_text(json.dumps(report, indent=1))
    # A key is below the bar on its unrounded share, as the measurement
    # record counts it: 97.998% of files misses a bar of 98, where the
    # rounded `rate` of the tables reads 98.0.
    below = [r for r in rows if r['agree'] * 100.0 < opts.bar * r['files']]
    uncovered = [r for r in rejected if not r['covered_by']]
    print(json.dumps({'files': len(files), 'keys': len(rows), 'below_bar': len(below),
                      'rejected': len(rejected), 'rejected_uncovered': len(uncovered)}))
    for r in below[:opts.top]:
        print(f"{r['rate']:6.2f}% {r['agree']}/{r['files']} {r['key']}")
    return 1 if below or uncovered else 0


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='mode', required=True)
    for name in ('commands', 'keys'):
        p = sub.add_parser(name)
        p.add_argument('--coreav', required=True)
        p.add_argument('--media', nargs='+', default=[])
        p.add_argument('--report')
        p.add_argument('--top', type=int, default=40)
        p.add_argument('--jobs', type=int, default=os.cpu_count() or 4)
        if name == 'commands':
            p.add_argument('--baseline')
            p.add_argument('--write-baseline')
        if name == 'keys':
            p.add_argument('--list')
            p.add_argument('--bar', type=float, default=98.0)
    opts = parser.parse_args()
    return cmd_commands(opts) if opts.mode == 'commands' else cmd_keys(opts)


if __name__ == '__main__':
    sys.exit(main())
