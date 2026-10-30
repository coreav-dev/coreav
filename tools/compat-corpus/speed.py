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

    speed.py r4 --arm r4|aa|null --coreav PATH --ffmpeg PATH --ffprobe PATH
                (--files FILES.json | --media DIR [DIR...]) --scratch DIR
                --out RESULTS.json [--rounds 5] [--cpus 4-7]
        R4 of the 0.1.0 round (docs/design/compat-cli.md, "The 0.1.0 bars"):
        every command of commands-0.1.0, `ffprobe` and `ffmpeg` alike, on
        every file of its families that can take it (R3's rule, from the
        pinned ffprobe's stream listing), in the order of the set and of the
        files. A pair is one untimed run of each side, then ROUNDS rounds
        alternating them, wall time of the whole process, one process at a
        time on the CPUs of --cpus; an `ffmpeg` command writes into
        SCRATCH, which is emptied after every run. The arm sets the sides:
        `r4` CoreAV against FFmpeg, `aa` FFmpeg against itself, `null`
        CoreAV against itself (the gate). FILES.json lists the files in
        their order as {"family", "path"}; RESULTS.json names a file by its
        family and position only.

    speed.py r4-report --results RESULTS.json [--out FILE] [--gate]
        Per command class (the `Job` column), in each family and over all
        of them: the median ratio, its bootstrap 95% interval and the share
        of pairs whose ratio is below 1.00; per command, the median alone.
        Without --gate, a class meets R4 when its median is at most 1.00;
        with --gate (the `aa` and `null` arms), the host passes when every
        class's median over the families is within 0.98 to 1.02 and its
        interval holds 1.00. Exit 1 when a class misses or the gate fails,
        2 when the run is void (its fallback log).

Every run has no gconv modules (release_round.NO_GCONV). Python 3 standard
library only, besides boto3 for `select`.
"""
import argparse
import collections
import hashlib
import json
import os
import pathlib
import random
import shutil
import statistics
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import release_round  # noqa: E402

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
ENV = release_round.environment()


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


# The sides of each arm of the 0.1.0 round: the program on the left of a
# ratio, and on the right.
ARMS = {'r4': ('coreav', 'ffmpeg'), 'aa': ('ffmpeg', 'ffmpeg'), 'null': ('coreav', 'coreav')}
GATE = (0.98, 1.02)


def cpu_list(text):
    """`4-7` or `4,5,6,7` as a set of CPU numbers."""
    out = set()
    for part in text.split(','):
        lo, _, hi = part.partition('-')
        out.update(range(int(lo), int(hi or lo) + 1))
    return out


def r4_files(opts):
    if opts.files:
        return [(e['family'], pathlib.Path(e['path'])) for e in json.loads(pathlib.Path(opts.files).read_text())]
    out = []
    for d in map(pathlib.Path, opts.media):
        for p in sorted(d.iterdir()) if d.is_dir() else [d]:
            if release_round.family(p):
                out.append((release_round.family(p), p))
    return out


def listed_streams(ffprobe, path, env):
    try:
        done = subprocess.run([ffprobe, '-v', 'error', '-of', 'json', '-show_streams', str(path)],
                              stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=600)
        return json.loads(done.stdout)['streams'] if done.returncode == 0 else None
    except (subprocess.TimeoutExpired, ValueError, KeyError):
        return None


def r4_pairs(opts, files, env):
    """(entry, index, command, file label, family, path) for every pair the
    arm times, in the order of the set and of the files."""
    labels, n = [], collections.Counter()
    for fam, path in files:
        labels.append(f'{fam}:{n[fam]}')
        n[fam] += 1
    listings = {}
    out = []
    for entry, i, command in [(e, i, c) for e in release_round.entries() for i, c in enumerate(e['commands'])]:
        for label, (fam, path) in zip(labels, files):
            if fam not in release_round.containers(entry, command):
                continue
            if entry['tool'] == 'ffmpeg':
                if path not in listings:
                    listings[path] = listed_streams(opts.ffprobe, path, env)
                if listings[path] is None or not release_round.can_take(entry, command, fam, listings[path]):
                    continue
            out.append((entry, i, command, label, fam, path))
    return out


def clear(directory):
    for p in directory.iterdir():
        shutil.rmtree(p) if p.is_dir() else p.unlink()


def timed_run(argv, env, scratch):
    """One process, wall time from start to exit; SCRATCH is emptied after
    it, so every run writes to an empty directory."""
    start = time.perf_counter()
    try:
        code = subprocess.run(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                              stderr=subprocess.DEVNULL, env=env, timeout=600).returncode
    except subprocess.TimeoutExpired:
        code = None
    elapsed = time.perf_counter() - start
    clear(scratch)
    return elapsed, code


def r4(opts):
    os.sched_setaffinity(0, cpu_list(opts.cpus))
    root = pathlib.Path(opts.scratch)
    out_dir, lists = root / 'out', root / 'lists'
    out_dir.mkdir(parents=True, exist_ok=True)
    lists.mkdir(parents=True, exist_ok=True)
    clear(out_dir)
    log = root / 'fallback.log'
    if log.exists():
        log.unlink()
    env = release_round.environment(log)
    files = r4_files(opts)
    pairs = r4_pairs(opts, files, env)
    left, right = ARMS[opts.arm]
    results = []
    for k, (entry, i, command, label, fam, path) in enumerate(pairs):
        source = path
        if entry['job'] == 'join':
            source = release_round.concat_list(lists / label.replace(':', '-'), path)
        argv = release_round.fill(command['args'], source, out_dir / release_round.output_name(entry['job']))
        program = {'coreav': [opts.coreav, entry['tool']],
                   'ffmpeg': [opts.ffmpeg if entry['tool'] == 'ffmpeg' else opts.ffprobe]}
        a, b = program[left] + argv, program[right] + argv
        row = {'id': entry['id'], 'command': i, 'class': entry['job'], 'family': fam, 'file': label}
        warm = (timed_run(a, env, out_dir)[1], timed_run(b, env, out_dir)[1])
        if warm != (0, 0):
            row['exit'] = list(warm)
            # Refused: every CoreAV side refused (exit 4) where every FFmpeg side ran.
            row['refused'] = 'coreav' in (left, right) and all(
                code == (4 if side == 'coreav' else 0) for side, code in zip((left, right), warm))
            results.append(row)
            continue
        times, codes = ([], []), set()
        for _ in range(opts.rounds):
            for side, argv_ in ((0, a), (1, b)):
                t, code = timed_run(argv_, env, out_dir)
                times[side].append(t)
                codes.add(code)
        row.update(a=times[0], b=times[1])
        if codes == {0}:
            row['ratio'] = statistics.median(times[0]) / statistics.median(times[1])
        else:
            row['exit'] = sorted(c if c is not None else -1 for c in codes)
        results.append(row)
        if (k + 1) % 100 == 0 or k + 1 == len(pairs):
            print(f'{k + 1}/{len(pairs)}', file=sys.stderr, flush=True)
    pathlib.Path(opts.out).write_text(json.dumps(
        {'set': '0.1.0', 'arm': opts.arm, 'sides': [left, right], 'rounds': opts.rounds, 'cpus': opts.cpus,
         'files': dict(collections.Counter(f for f, _ in files)),
         'fallback_log': release_round.fallback_verdict(log), 'results': results}) + '\n')
    return 0


def bootstrap(ratios, rng):
    """The median's bootstrap 95% interval: 10,000 resamples of the pairs."""
    meds = sorted(statistics.median(rng.choices(ratios, k=len(ratios))) for _ in range(10000))
    return [meds[249], meds[9749]]


def r4_summary(data, gate):
    rng = random.Random(0)
    groups = collections.defaultdict(list)
    commands = collections.defaultdict(list)
    for r in data['results']:
        if 'ratio' not in r:
            continue
        groups[(r['class'], r['family'])].append(r['ratio'])
        groups[(r['class'], 'all')].append(r['ratio'])
        commands[f"{r['id']}.{r['command']}"].append(r['ratio'])
    rows = []
    for (cls, fam), ratios in sorted(groups.items()):
        median, ci = statistics.median(ratios), bootstrap(ratios, rng)
        row = {'class': cls, 'family': fam, 'n': len(ratios), 'median': median, 'ci95': ci,
               'faster_share': sum(x < 1.0 for x in ratios) / len(ratios)}
        if gate:
            row['passes'] = GATE[0] <= median <= GATE[1] and ci[0] <= 1.0 <= ci[1] if fam == 'all' else None
        else:
            row.update(meets=median <= 1.0, faster=ci[1] < 1.0)
        rows.append(row)
    per_command = [{'command': c, 'n': len(v), 'median': statistics.median(v)} for c, v in sorted(commands.items())]
    no_ratio = collections.Counter(
        'refused' if r.get('refused') else 'failed' for r in data['results'] if 'ratio' not in r)
    failing = [r for r in rows if r.get('passes') is False or r.get('meets') is False]
    return {'arm': data['arm'], 'sides': data['sides'], 'files': data['files'], 'gate': gate,
            'fallback_log': data['fallback_log'], 'pairs': len(data['results']), 'no_ratio': dict(no_ratio),
            'classes': rows, 'commands': per_command,
            'failing': [f"{r['class']} {r['family']}" for r in failing]}


def r4_report(opts):
    summary = r4_summary(json.loads(pathlib.Path(opts.results).read_text()), opts.gate)
    if opts.out:
        pathlib.Path(opts.out).write_text(json.dumps(summary, indent=1) + '\n')
    for r in summary['classes']:
        if opts.gate:
            verdict = {True: 'pass', False: 'FAIL', None: '(not gated)'}[r['passes']]
        else:
            verdict = 'meets' if r['meets'] else 'MISSES'
        print(f"{r['class']:14} {r['family']:13} n={r['n']:5} median={r['median']:.3f} "
              f"ci95=[{r['ci95'][0]:.3f}, {r['ci95'][1]:.3f}] faster={r['faster_share']:.0%} {verdict}")
    print(json.dumps({k: summary[k] for k in ('arm', 'pairs', 'no_ratio', 'failing', 'fallback_log')}))
    if summary['fallback_log']['void']:
        print('VOID: the fallback log holds a line that is not a strict-safe refusal')
        return 2
    return 1 if summary['failing'] else 0


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
    p = sub.add_parser('r4')
    p.add_argument('--arm', choices=sorted(ARMS), required=True)
    p.add_argument('--coreav', required=True)
    p.add_argument('--ffmpeg', required=True)
    p.add_argument('--ffprobe', required=True)
    p.add_argument('--files')
    p.add_argument('--media', nargs='+', default=[])
    p.add_argument('--scratch', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--rounds', type=int, default=5)
    p.add_argument('--cpus', default='4-7', help='the CPUs every timed process runs on')
    p = sub.add_parser('r4-report')
    p.add_argument('--results', required=True)
    p.add_argument('--out')
    p.add_argument('--gate', action='store_true')
    opts = parser.parse_args()
    return {'select': select, 'run': run, 'report': report, 'r4': r4, 'r4-report': r4_report}[opts.mode](opts)


if __name__ == '__main__':
    sys.exit(main())
