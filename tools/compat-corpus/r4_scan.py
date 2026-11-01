#!/usr/bin/env python3
"""Which pairs of R4 write to the fallback log (docs/measurements/compat-0.1.0/README.md, "The commit": a run whose
log holds a line is void), without timing anything.

    r4_scan.py --coreav PATH --ffmpeg PATH --ffprobe PATH (--files FILE | --media DIR [DIR...])
               --scratch DIR --out FILE [--records FILE] [--jobs N]

Every pair `speed.py r4` would time (the files in the R4 order, each command on the files R3's rule lets take it)
runs once through `coreav` in strict-safe mode with a fallback log of its own, and once through the pinned `ffmpeg`
or `ffprobe`. OUT names files by family and position, as R4's results do, and holds per command:

- `logged`: the pairs whose log holds a line, by the reason `coreav` logged (it redacts the reason itself): an
  `option` (an option or one of its values), a `property` (of the command or of what the file holds: a codec, a
  stream layout, a container feature), or `other` for a line that is not a strict-safe refusal. `every pair` says
  whether the reason stopped the command on every file it runs on, which a reason of the command does and a
  reason of some files does not, and `reference fails on` which of those files the pinned tool fails as well;
- `failed`: the pairs `coreav` failed with no line in the log (an exit other than 0 and 4), which do not void a
  run but may be bugs;
- `excluded`: the files of the command's families that R3's rule does not let take it, which R4 never runs.

RECORDS gets one line per pair with the file's path and the first line of `coreav`'s standard error: private, as
the files are. Exit 0 when no pair wrote a line, 1 when one did, 3 when there was no pair.
"""
import argparse
import collections
import concurrent.futures
import json
import pathlib
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import release_round  # noqa: E402
import speed  # noqa: E402

def cause(line):
    if line.get('not_run') != 'strict_safe':
        return 'other'
    return 'option' if str(line.get('reason', '')).startswith('-') else 'property'


def log_lines(path):
    out = []
    for text in path.read_text(errors='replace').splitlines() if path.exists() else []:
        if not text.strip():
            continue
        try:
            line = json.loads(text)
        except ValueError:
            line = {}
        out.append(line if isinstance(line, dict) else {})
    return out


def scan_pair(opts, pair, scratch):
    entry, i, command, label, fam, path = pair
    with tempfile.TemporaryDirectory(dir=scratch) as tmp:
        tmp = pathlib.Path(tmp)
        out = tmp / 'out'
        out.mkdir()
        log = tmp / 'fallback.log'
        source = release_round.concat_list(tmp / 'list', path) if entry['job'] == 'join' else path
        argv = release_round.fill(command['args'], source, out / release_round.output_name(entry['job']))
        try:
            done = subprocess.run([opts.coreav, entry['tool']] + argv, stdin=subprocess.DEVNULL,
                                  stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                                  env=release_round.environment(log), timeout=600)
            code, err = done.returncode, done.stderr.decode('utf-8', 'replace')
        except subprocess.TimeoutExpired:
            code, err = None, ''
        speed.clear(out)
        reference = opts.ffmpeg if entry['tool'] == 'ffmpeg' else opts.ffprobe
        ref_code = speed.timed_run([reference] + argv, release_round.environment(), out)[1]
        return {'command': f"{entry['id']}.{i}", 'family': fam, 'file': label, 'path': str(path),
                'coreav': code, 'reference': ref_code,
                'lines': [{'reason': line.get('reason'), 'not_run': line.get('not_run'), 'cause': cause(line)}
                          for line in log_lines(log)],
                'stderr': next((t for t in err.splitlines() if t.strip()), '')[:300]}


def summarize(files, pairs, records):
    labels = collections.defaultdict(list)
    n = collections.Counter()
    for fam, _ in files:
        labels[fam].append(f'{fam}:{n[fam]}')
        n[fam] += 1
    taken = collections.defaultdict(set)
    for entry, i, _, label, _, _ in pairs:
        taken[f"{entry['id']}.{i}"].add(label)
    commands = {}
    for entry in release_round.entries():
        for i, command in enumerate(entry['commands']):
            name = f"{entry['id']}.{i}"
            every = [label for fam in release_round.containers(entry, command) for label in labels[fam]]
            commands[name] = {'tool': entry['tool'], 'class': entry['job'], 'pairs': len(taken[name]),
                              'logged': {}, 'failed': [],
                              'excluded': [label for label in every if label not in taken[name]]}
    causes = collections.Counter()
    for r in records:
        c = commands[r['command']]
        for line in r['lines']:
            key = f"{line['cause']}: {line['reason'] or line['not_run']}"
            group = c['logged'].setdefault(key, {'cause': line['cause'], 'files': [], 'reference fails on': []})
            if r['file'] not in group['files']:
                group['files'].append(r['file'])
                if r['reference'] != 0:
                    group['reference fails on'].append(r['file'])
        causes.update({line['cause'] for line in r['lines']})
        if not r['lines'] and r['coreav'] not in (0, 4):
            c['failed'].append({'file': r['file'], 'exit': r['coreav'], 'reference': r['reference']})
    for c in commands.values():
        for group in c['logged'].values():
            group['every pair'] = len(group['files']) == c['pairs']
    return {'files': dict(n), 'pairs': len(records), 'pairs logged': sum(1 for r in records if r['lines']),
            'pairs by cause': dict(causes),
            'commands logged': sorted(k for k, c in commands.items() if c['logged']), 'commands': commands}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--coreav', required=True)
    ap.add_argument('--ffmpeg', required=True)
    ap.add_argument('--ffprobe', required=True)
    ap.add_argument('--files')
    ap.add_argument('--media', nargs='+', default=[])
    ap.add_argument('--scratch', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--records')
    ap.add_argument('--jobs', type=int, default=2)
    opts = ap.parse_args()
    files = speed.r4_files(opts)
    pairs = speed.r4_pairs(opts, files, release_round.environment())
    if not pairs:
        print('UNMEASURED: no pair to scan')
        return release_round.UNMEASURED
    scratch = pathlib.Path(opts.scratch)
    scratch.mkdir(parents=True, exist_ok=True)
    with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
        records = list(pool.map(lambda p: scan_pair(opts, p, scratch), pairs))
    if opts.records:
        pathlib.Path(opts.records).write_text(''.join(json.dumps(r) + '\n' for r in records))
    summary = summarize(files, pairs, records)
    pathlib.Path(opts.out).write_text(json.dumps(summary, indent=1) + '\n')
    print(json.dumps({k: summary[k] for k in ('files', 'pairs', 'pairs logged', 'pairs by cause', 'commands logged')}))
    return 1 if summary['pairs logged'] else 0


if __name__ == '__main__':
    sys.exit(main())
