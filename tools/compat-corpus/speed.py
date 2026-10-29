#!/usr/bin/env python3
"""B4: the speed of `coreav ffprobe` against the pinned `ffprobe`
(docs/design/compat-cli.md, "The measurements", fixed before the run).

    speed.py select --out DIR
        Downloads the held-out files B4 names: from each container (ISO base
        media from corpus v1, Matroska and WebM from corpus v1, transport
        streams from corpus v1-ts-audio), the first 20 held-out `real` files
        of at most 256 MB in the order sha256("coreav-compat-b4:" + sha256).
        Writes DIR/selection.json. R2 credentials come from the environment
        and are never printed.

    speed.py run --coreav PATH --dir DIR --out RESULTS.json [--rounds 5]
        Every native `ffprobe` command of commands-v1 on every selected
        file: `coreav ffprobe` and `ffprobe` alternate, one process at a
        time, wall time of the whole process, after one untimed run of each
        (the page cache). A pair's ratio is the median of CoreAV's rounds
        over the median of ffprobe's. A command coreav refuses is reported
        as refused and has no ratio.

    speed.py report --results RESULTS.json
        Per command, per container and overall: the median ratio and its
        bootstrap 95% interval (10,000 resamples of the files, seed 0).

Python 3 standard library only, besides boto3 for `select`.
"""
import argparse
import hashlib
import json
import os
import pathlib
import random
import statistics
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
COMMANDS = HERE / 'commands-v1' / 'commands.jsonl'
SALT = 'coreav-compat-b4:'
# The first path component of an object key in the private store.
STORE_ROOT = 'corpus'
LIMIT = 256 * 1024 * 1024
PER_CONTAINER = 20
CLASSES = {
    'isobmff': ('v1', {'mp4', 'mov', 'm4a', 'm4b', 'm4v', '3gp'}),
    'matroska': ('v1', {'mkv', 'webm'}),
    'mpegts': ('v1-ts-audio', {'ts', 'm2ts', 'mts', 'm2t'}),
}


def entries(corpus):
    data = json.loads((ROOT / 'corpus' / corpus / 'manifest.json').read_text())
    return data['files'] if isinstance(data, dict) else data


def order(entry):
    return hashlib.sha256((SALT + entry['sha256']).encode()).hexdigest()


def select(opts):
    sys.path.insert(0, str(ROOT / 'corpus' / 'v1' / 'scripts'))
    import common  # noqa: E402
    out = pathlib.Path(opts.out)
    out.mkdir(parents=True, exist_ok=True)
    chosen = []
    for name, (corpus, exts) in CLASSES.items():
        pool = [e for e in entries(corpus)
                if e.get('set') == 'real' and e.get('holdout') and e['ext'] in exts
                and e['size'] <= LIMIT]
        pool.sort(key=order)
        for e in pool[:PER_CONTAINER]:
            key = e['key'] if '/' in e['key'] else '/'.join((STORE_ROOT, corpus, e['key']))
            chosen.append({'class': name, 'sha256': e['sha256'], 'ext': e['ext'],
                           'size': e['size'], 'key': key})
    s3 = common.s3()
    for e in chosen:
        dest = out / f"{e['sha256']}.{e['ext']}"
        if dest.exists() and dest.stat().st_size == e['size']:
            continue
        part = dest.with_suffix(dest.suffix + '.part')
        s3.download_file(common.BUCKET, e['key'], str(part))
        part.rename(dest)
    (out / 'selection.json').write_text(json.dumps(
        [{k: v for k, v in e.items() if k != 'key'} for e in chosen], indent=1) + '\n')
    print(json.dumps({c: sum(1 for e in chosen if e['class'] == c) for c in CLASSES}))


def native_commands():
    out = []
    with open(COMMANDS) as f:
        for line in f:
            r = json.loads(line)
            if r['tool'] == 'ffprobe' and r['label'] == 'native':
                out.append(r['args'])
    return out


# `coreav` in strict-safe mode, so that a command it refuses is reported as
# refused and never timed as the `ffprobe` on PATH.
ENV = {**os.environ, 'COREAV_STRICT_SAFE': '1'}


def timed(argv):
    start = time.perf_counter()
    p = subprocess.run(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=ENV)
    return time.perf_counter() - start, p.returncode


def run(opts):
    d = pathlib.Path(opts.dir)
    selection = json.loads((d / 'selection.json').read_text())
    commands = native_commands()
    results = []
    for n, args in enumerate(commands):
        for e in selection:
            path = str(d / f"{e['sha256']}.{e['ext']}")
            argv = [path if a == 'INPUT' else a for a in args]
            ours = [opts.coreav, 'ffprobe'] + argv
            theirs = ['ffprobe'] + argv
            _, code = timed(ours)
            timed(theirs)
            row = {'command': n, 'class': e['class'], 'file': e['sha256'][:12]}
            if code == 4:
                row['refused'] = True
                results.append(row)
                continue
            a, b = [], []
            for _ in range(opts.rounds):
                a.append(timed(ours)[0])
                b.append(timed(theirs)[0])
            row.update({'coreav': a, 'ffprobe': b,
                        'ratio': statistics.median(a) / statistics.median(b)})
            results.append(row)
        print(f'{n + 1}/{len(commands)}', file=sys.stderr, flush=True)
    pathlib.Path(opts.out).write_text(json.dumps(
        {'commands': [' '.join(c) for c in commands], 'rounds': opts.rounds,
         'results': results}) + '\n')


def interval(ratios, rng):
    if not ratios:
        return None
    meds = []
    for _ in range(10000):
        sample = [rng.choice(ratios) for _ in ratios]
        meds.append(statistics.median(sample))
    meds.sort()
    return [meds[249], meds[9749]]


def report(opts):
    data = json.loads(pathlib.Path(opts.results).read_text())
    rng = random.Random(0)
    rows = []
    groups = {}
    for r in data['results']:
        if 'ratio' not in r:
            continue
        groups.setdefault(('all', 'all'), []).append(r['ratio'])
        groups.setdefault(('class', r['class']), []).append(r['ratio'])
        groups.setdefault(('command', r['command']), []).append(r['ratio'])
        groups.setdefault(('command-class', f"{r['command']}:{r['class']}"), []).append(r['ratio'])
    for (kind, name), ratios in sorted(groups.items(), key=lambda kv: (kv[0][0], str(kv[0][1]))):
        rows.append({'group': kind, 'name': name, 'n': len(ratios),
                     'median': statistics.median(ratios),
                     'ci95': interval(ratios, rng) if kind != 'command-class' else None})
    refused = sorted({data['commands'][r['command']] for r in data['results'] if r.get('refused')})
    missed = [r for r in rows if r['median'] > 1.0]
    out = {'rows': rows, 'refused': refused, 'missed': missed}
    if opts.out:
        pathlib.Path(opts.out).write_text(json.dumps(out, indent=1) + '\n')
    for r in rows:
        if r['group'] in ('all', 'class'):
            print(f"{r['group']:6} {r['name']:9} n={r['n']:5} median={r['median']:.3f} ci95={r['ci95']}")
    worst = max((r for r in rows if r['group'] == 'command'), key=lambda r: r['median'], default=None)
    if worst:
        print(f"slowest command: {data['commands'][worst['name']]} median={worst['median']:.3f}")
    print(f"groups over 1.00: {len(missed)}; commands refused: {len(refused)}")


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='mode', required=True)
    p = sub.add_parser('select')
    p.add_argument('--out', required=True)
    p = sub.add_parser('run')
    p.add_argument('--coreav', required=True)
    p.add_argument('--dir', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--rounds', type=int, default=5)
    p = sub.add_parser('report')
    p.add_argument('--results', required=True)
    p.add_argument('--out')
    opts = parser.parse_args()
    {'select': select, 'run': run, 'report': report}[opts.mode](opts)


if __name__ == '__main__':
    main()
