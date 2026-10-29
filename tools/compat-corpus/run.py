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
SET_ASIDE = {
    'size_entry_understated.mp4': DECLARED_SIZE,
    'size_prores_understated.mov': DECLARED_SIZE,
    'size_hevc_window.mp4': DECLARED_SIZE,
    'size_hevc_mixed_window.mp4': DECLARED_SIZE,
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
    _, ref_out, _ = run(['ffprobe'] + SECTION_ARGS + [str(path)])
    code, out, _ = run([coreav, 'ffprobe'] + SECTION_ARGS + [str(path)])
    try:
        extra = set(flatten(json.loads(out))) - set(flatten(json.loads(ref_out)))
    except ValueError:
        return None
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
                return path, None
            return path, (ref_out,) + run([opts.coreav, 'ffprobe'] + argv)
        with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
            pairs = list(pool.map(both, files))
        for path, pair in pairs:
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
            else:
                b3['fail'] += 1
                failures.append({'bar': 'B3', 'args': args, 'file': path.name,
                                 'diff': first_difference(ref_out, out)})
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
