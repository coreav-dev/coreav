#!/usr/bin/env python3
"""Run the Matroska oracle on corpus v1 and local recordings (docs/design/mkv.md).

`run` streams files one at a time per worker: it downloads a manifest entry
from R2 with the AWS CLI (or takes a local file), checks its SHA-256, captures
the `streams` and `packets` levels through `scripts/oracle-ffprobe.sh` and
`mkv-adapter`, and the MediaInfo record, then deletes the download. The
adapter's exit status, signal and wall time are kept for the robustness bar.
With --keep, downloads stay in --work and a rerun with a changed adapter
captures only the adapter's side again.
Captures and timings stay outside git.

`report` compares the captures with `coreav-oracle compare --profile corpus`,
applies `scripts/oracle_kd_mkv.py`, and writes counts per split: never file
names or values of private files.

R2 credentials come from R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and
R2_ENDPOINT and are passed to the AWS CLI only through its environment; the
bucket comes from R2_BUCKET.
MANIFEST is corpus v1's manifest.

  corpus.py run --out DIR --work DIR [--manifest MANIFEST]
                [--split development|holdout|all] [--set real,edge,hostile]
                [--local DIR] [--jobs N] [--aws aws] [--limit N]
  corpus.py report --out DIR --report REPORT.json [--markdown REPORT.md]
"""
import argparse
import collections
import concurrent.futures
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(ROOT / 'tools' / 'mediainfo-oracle'))
import mediainfo_oracle as mo  # noqa: E402

TIMEOUT = 120
EXTS = ('mkv', 'webm')
LEVELS = ('streams', 'packets')


def r2_environment():
    env = dict(os.environ)
    env.update({'AWS_ACCESS_KEY_ID': os.environ['R2_ACCESS_KEY_ID'],
                'AWS_SECRET_ACCESS_KEY': os.environ['R2_SECRET_ACCESS_KEY'],
                'AWS_DEFAULT_REGION': 'auto'})
    env.pop('AWS_SESSION_TOKEN', None)
    return env, os.environ['R2_ENDPOINT']


def split_of(entry):
    if entry.get('set') != 'real':
        return entry.get('set', 'local')
    return 'holdout' if entry.get('holdout') else 'development'


def select(args):
    entries = []
    if args.manifest:
        files = json.loads(pathlib.Path(args.manifest).read_text())['files']
        sets = args.set.split(',')
        for f in files:
            if f['ext'] not in EXTS or f['set'] not in sets:
                continue
            split = split_of(f)
            if args.split in ('development', 'holdout') and f['set'] == 'real' and split != args.split:
                continue
            entries.append({'name': f"{f['sha256']}.{f['ext']}", 'key': f['key'], 'sha256': f['sha256'],
                            'split': split, 'size': f['size']})
    if args.local:
        for path in sorted(pathlib.Path(args.local).iterdir()):
            if path.suffix.lstrip('.') in EXTS:
                entries.append({'name': path.name, 'path': str(path), 'split': 'mediarecorder',
                                'size': path.stat().st_size})
    entries.sort(key=lambda e: e['size'])
    return entries[:args.limit] if args.limit else entries


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as source:
        for block in iter(lambda: source.read(1 << 20), b''):
            digest.update(block)
    return digest.hexdigest()


def capture(command, name, level, out_path):
    """Runs an adapter; writes its document or a failure record; returns the run record."""
    started = time.monotonic()
    try:
        done = subprocess.run(command, capture_output=True, timeout=TIMEOUT, check=False)
        status, stdout, stderr = done.returncode, done.stdout, done.stderr
    except subprocess.TimeoutExpired:
        status, stdout, stderr = 'timeout', b'', b''
    elapsed = time.monotonic() - started
    document = None
    if status == 0 and stdout.strip():
        try:
            document = json.loads(stdout)
        except json.JSONDecodeError:
            document = None
    if document is None:
        document = {'schema': f'coreav-oracle/{level}/1', 'file': name, 'probe_ok': False}
    out_path.write_text(json.dumps(document) + '\n')
    return {'status': status, 'seconds': round(elapsed, 4), 'panicked': b'panicked' in stderr}


def process(entry, args, env, endpoint):
    out = pathlib.Path(args.out)
    name = entry['name']
    run_path = out / 'runs' / f'{name}.json'
    record = json.loads(run_path.read_text()) if run_path.exists() else None
    adapter = sha256_file(args.adapter)
    if record and record.get('adapter_sha256') == adapter:
        return 'cached'
    media = pathlib.Path(entry['path']) if 'path' in entry else pathlib.Path(args.work) / name
    downloaded = False
    try:
        if record and not media.exists():
            return 'stale-no-media'
        if 'key' in entry and not media.exists():
            done = subprocess.run([args.aws, '--endpoint-url', endpoint, 's3', 'cp', '--only-show-errors',
                                   f"s3://{os.environ['R2_BUCKET']}/{entry['key']}", str(media)],
                                  env=env, capture_output=True, timeout=3600, check=False)
            downloaded = True
            if done.returncode != 0:
                return 'download-failed'
            if sha256_file(media) != entry['sha256']:
                return 'sha256-mismatch'
        if record is None:
            record = {'split': entry['split'], 'size': entry['size'], 'runs': {}}
            for level in LEVELS:
                record['runs'][f'ffprobe.{level}'] = capture(
                    [str(ROOT / 'scripts' / 'oracle-ffprobe.sh'), level, str(media)], name, level,
                    out / 'reference' / f'{name}.{level}.json')
            mediainfo = mo.capture_one(args.mediainfo, media, name)
            (out / 'mediainfo' / f'{name}.mediainfo.json').write_text(json.dumps(mediainfo) + '\n')
        # A new capture makes this file's old comparison stale.
        (out / 'reports' / f'{name}.report.json').unlink(missing_ok=True)
        for level in LEVELS:
            record['runs'][f'coreav.{level}'] = capture(
                [args.adapter, level, str(media)], name, level, out / 'candidate' / f'{name}.{level}.json')
        record['adapter_sha256'] = adapter
        run_path.write_text(json.dumps(record) + '\n')
        return 'captured'
    finally:
        if downloaded and not args.keep:
            media.unlink(missing_ok=True)


def cmd_run(args):
    out = pathlib.Path(args.out)
    for sub in ('reference', 'candidate', 'mediainfo', 'runs'):
        (out / sub).mkdir(parents=True, exist_ok=True)
    pathlib.Path(args.work).mkdir(parents=True, exist_ok=True)
    provenance = out / 'PROVENANCE.json'
    current = {'ffprobe': subprocess.run(['ffprobe', '-version'], capture_output=True, text=True).stdout.split('\n')[0],
               'mediainfo': subprocess.run([args.mediainfo, '--Version'], capture_output=True,
                                           text=True).stdout.strip().split('\n')[-1]}
    if provenance.exists() and json.loads(provenance.read_text()) != current:
        print('corpus.py: captures in --out come from another ffprobe or MediaInfo; use a new --out', file=sys.stderr)
        return 2
    provenance.write_text(json.dumps(current, indent=1) + '\n')
    entries = select(args)
    env, endpoint = r2_environment() if args.manifest else (None, None)
    outcomes = collections.Counter()
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        futures = [pool.submit(process, e, args, env, endpoint) for e in entries]
        for number, future in enumerate(concurrent.futures.as_completed(futures), 1):
            outcomes[future.result()] += 1
            if number % 20 == 0 or number == len(futures):
                print(f'{number}/{len(futures)} {dict(outcomes)}', flush=True)
    return 0 if not (outcomes['download-failed'] or outcomes['sha256-mismatch']) else 2


def robustness(records):
    """Adapter failures that break the robustness bar: crashes, panics, timeouts."""
    bad = collections.Counter()
    for record in records:
        for side, run in record['runs'].items():
            if not side.startswith('coreav.'):
                continue
            if run['status'] == 'timeout':
                bad['timeout'] += 1
            elif run['status'] != 0:
                bad[f'exit-{run["status"]}'] += 1
            if run['panicked']:
                bad['panic'] += 1
    return dict(bad)


def compare_one(out, name, reports):
    """Compares one file (the comparator holds every packet in memory, so
    files are compared one at a time); returns the report path."""
    report = reports / f'{name}.report.json'
    if report.exists():
        return report
    oracle = os.environ.get('COREAV_ORACLE', str(ROOT / 'target' / 'release' / 'coreav-oracle'))
    work = reports / f'{name}.tmp'
    for side in ('reference', 'candidate'):
        (work / side).mkdir(parents=True, exist_ok=True)
        for level in LEVELS:
            target = work / side / f'{name}.{level}.json'
            if not target.exists():
                os.link(out / side / f'{name}.{level}.json', target)
    subprocess.run([oracle, 'compare', '--reference', str(work / 'reference'), '--candidate',
                    str(work / 'candidate'), '--levels', ','.join(LEVELS), '--profile', 'fixtures',
                    '--max-diffs', '1000000', '--report', str(report)], stdout=subprocess.DEVNULL, check=False)
    for path in sorted(work.rglob('*'), reverse=True):
        path.unlink() if path.is_file() else path.rmdir()
    work.rmdir()
    return report


def compare(out, names, workdir):
    """Compares every file, then applies the known-divergence overlay to all
    the reports together; returns the overlay's levels."""
    reports = out / 'reports'
    reports.mkdir(parents=True, exist_ok=True)
    with concurrent.futures.ThreadPoolExecutor(4) as pool:
        paths = list(pool.map(lambda n: compare_one(out, n, reports), names))
    workdir.mkdir(parents=True, exist_ok=True)
    command = [sys.executable, '-I', str(ROOT / 'scripts' / 'oracle_kd_mkv.py'), '--reference',
               str(out / 'reference'), '--candidate', str(out / 'candidate'), '--out',
               str(workdir / 'known-divergences.json')]
    for path in paths:
        command += ['--report', str(path)]
    subprocess.run(command, stdout=subprocess.DEVNULL, check=False)
    overlay = json.loads((workdir / 'known-divergences.json').read_text())
    return {level: groups['in_scope'] for level, groups in overlay['levels'].items()}


def cmd_report(args):
    out = pathlib.Path(args.out)
    records = {}
    for path in sorted((out / 'runs').glob('*.json')):
        records[path.name[:-5]] = json.loads(path.read_text())
    splits = collections.defaultdict(list)
    for name, record in records.items():
        splits[record['split']].append(name)
    groups = {split: names for split, names in splits.items()}
    real = [n for s in ('development', 'holdout', 'mediarecorder') for n in splits.get(s, [])]
    if real:
        groups['real+mediarecorder'] = real
    summary = {}
    for group, names in sorted(groups.items()):
        levels = compare(out, names, out / 'report' / group)
        summary[group] = {'files': len(names), 'levels': levels,
                          'robustness': robustness(records[n] for n in names)}
    pathlib.Path(args.report).write_text(json.dumps(summary, indent=1) + '\n')
    if args.markdown:
        lines = ['| Group | Files | Streams: files agreeing (direct + via KD) | Packets agreeing | '
                 'Crashes, panics, timeouts |', '|---|---|---|---|---|']
        for group, entry in summary.items():
            cells = []
            for level in LEVELS:
                g = entry['levels'][level]
                agree = g['direct'] + g['via_kd']
                percent = 100 * agree / g['items'] if g['items'] else 0
                cells.append(f"{agree}/{g['items']} ({g['direct']} + {g['via_kd']}) {percent:.3f}%")
            lines.append(f"| {group} | {entry['files']} | {cells[0]} | {cells[1]} | "
                         f"{entry['robustness'] or 'none'} |")
        pathlib.Path(args.markdown).write_text('\n'.join(lines) + '\n')
    print(json.dumps({g: {k: v for k, v in e.items() if k != 'levels'} | {
        lv: f"{e['levels'][lv]['direct'] + e['levels'][lv]['via_kd']}/{e['levels'][lv]['items']}"
        for lv in LEVELS} for g, e in summary.items()}, indent=1))
    return 0


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='command', required=True)
    run = sub.add_parser('run')
    run.add_argument('--out', required=True)
    run.add_argument('--work', required=True)
    run.add_argument('--manifest')
    run.add_argument('--split', default='all', choices=['development', 'holdout', 'all'])
    run.add_argument('--set', default='real')
    run.add_argument('--local')
    run.add_argument('--jobs', type=int, default=4)
    run.add_argument('--aws', default='aws')
    run.add_argument('--limit', type=int)
    run.add_argument('--keep', action='store_true',
                     help='keep downloads in --work, so a changed adapter can be rerun without ffprobe')
    run.add_argument('--adapter', default=str(ROOT / 'target' / 'release' / 'mkv-adapter'))
    run.add_argument('--mediainfo', default='mediainfo')
    report = sub.add_parser('report')
    report.add_argument('--out', required=True)
    report.add_argument('--report', required=True)
    report.add_argument('--markdown')
    args = parser.parse_args()
    return cmd_run(args) if args.command == 'run' else cmd_report(args)


if __name__ == '__main__':
    sys.exit(main())
