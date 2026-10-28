#!/usr/bin/env python3
"""Run the three black boxes on corpus v1 and write an aggregate report.

`run` streams the R2 corpus: for each manifest entry it downloads the file
with the AWS CLI, captures the ffprobe and CoreAV stream records through the
existing adapters (scripts/oracle-ffprobe.sh, tools/isobmff-adapter) and the
MediaInfo record, then deletes the media. Captures stay outside git.
Credentials come from an environment file (`KEY=value` lines with
AccessKeyID, SecretAccessKey and Endpoint); they are passed to the AWS CLI
through its environment only and never printed.

`report` aggregates the captures by split and by container (the manifest
entry's file extension). It publishes counts only, never file names or
values of private files.

  corpus.py run --manifest M --captures DIR --work DIR --r2-env FILE
                [--split tuning|holdout|all] [--set real,edge] [--jobs N]
                [--aws aws] [--bucket BUCKET] [--limit N]
  corpus.py report --manifest M --captures DIR --out REPORT.json --markdown REPORT.md
                [--split tuning|holdout|all]
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

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
import mediainfo_oracle as mo  # noqa: E402

TIMEOUT = 120


def split_of(entry):
    if entry.get('set') != 'real':
        return 'edge'
    return 'holdout' if entry.get('holdout') else 'tuning'


def select(manifest, split, sets):
    files = json.loads(pathlib.Path(manifest).read_text())['files']
    chosen = [f for f in files if f.get('set') in sets]
    if split == 'tuning':
        chosen = [f for f in chosen if not f.get('holdout')]
    elif split == 'holdout':
        chosen = [f for f in chosen if f.get('holdout')]
    return chosen


def capture_paths(captures, entry):
    # Captures made from a manifest whose entries have a `category` sit one
    # directory deeper, under it.
    base = pathlib.Path(captures) / entry['set'] / entry.get('category', '') / entry['sha256']
    return {side: base.with_name(f"{entry['sha256']}.{side}.json") for side in ('ffprobe', 'coreav', 'mediainfo')}


def failed(name):
    return {'schema': 'coreav-oracle/streams/1', 'file': name, 'probe_ok': False}


def oracle_adapter(command, name):
    try:
        done = subprocess.run(command, capture_output=True, timeout=TIMEOUT, check=False)
    except subprocess.TimeoutExpired:
        return failed(name)
    if done.returncode != 0 or not done.stdout.strip():
        return failed(name)
    try:
        return json.loads(done.stdout)
    except json.JSONDecodeError:
        return failed(name)


def r2_environment(path):
    values = {}
    for line in pathlib.Path(path).read_text().splitlines():
        if '=' in line and not line.lstrip().startswith('#'):
            key, value = line.split('=', 1)
            values[key.strip()] = value.strip().strip('"')
    env = dict(os.environ)
    env.update({'AWS_ACCESS_KEY_ID': values['AccessKeyID'], 'AWS_SECRET_ACCESS_KEY': values['SecretAccessKey'],
                'AWS_DEFAULT_REGION': 'auto'})
    env.pop('AWS_SESSION_TOKEN', None)
    return env, values['Endpoint']


def process(entry, args, env, endpoint):
    paths = capture_paths(args.captures, entry)
    if all(p.exists() for p in paths.values()):
        return 'cached'
    name = f"{entry['sha256']}.{entry['ext']}"
    media = pathlib.Path(args.work) / name
    try:
        done = subprocess.run([args.aws, '--endpoint-url', endpoint, 's3', 'cp', '--only-show-errors',
                               f"s3://{args.bucket}/{entry['key']}", str(media)],
                              env=env, capture_output=True, timeout=3600, check=False)
        if done.returncode != 0:
            return 'download-failed'
        records = {
            'ffprobe': oracle_adapter([str(ROOT / 'scripts' / 'oracle-ffprobe.sh'), 'streams', str(media)], name),
            'coreav': oracle_adapter([args.adapter, 'streams', str(media)], name),
            'mediainfo': mo.capture_one(args.mediainfo, media, name),
        }
        paths['ffprobe'].parent.mkdir(parents=True, exist_ok=True)
        for side, record in records.items():
            paths[side].write_text(json.dumps(record, sort_keys=True) + '\n')
        return 'captured'
    finally:
        media.unlink(missing_ok=True)


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as source:
        for block in iter(lambda: source.read(1 << 20), b''):
            digest.update(block)
    return digest.hexdigest()


def first_line(command):
    try:
        done = subprocess.run(command, capture_output=True, text=True, timeout=60, check=False)
    except (OSError, subprocess.TimeoutExpired):
        return None
    lines = [line for line in done.stdout.splitlines() if line.strip()]
    return lines[-1] if command[-1] == '--Version' and lines else (lines[0] if lines else None)


def provenance(args):
    """What produced the captures. Binaries and versions must match to reuse captures."""
    git = subprocess.run(['git', '-C', str(ROOT), 'describe', '--always', '--dirty', '--abbrev=12'],
                         capture_output=True, text=True, check=False).stdout.strip() or None
    ffprobe = os.environ.get('FFPROBE', 'ffprobe')
    return {
        'binaries': {
            'mediainfo': first_line([args.mediainfo, '--Version']),
            'ffprobe': first_line([ffprobe, '-version']),
            'isobmff_adapter_sha256': sha256_file(args.adapter),
            'coreav_oracle_sha256': sha256_file(os.environ.get(
                'COREAV_ORACLE', str(ROOT / 'target' / 'release' / 'coreav-oracle'))),
        },
        'tool': {
            'git': git,
            'mediainfo_oracle_py_sha256': sha256_file(HERE / 'mediainfo_oracle.py'),
            'corpus_py_sha256': sha256_file(HERE / 'corpus.py'),
        },
    }


def check_provenance(captures, current):
    """Refuse to mix captures from different binaries; warn when only the tool changed."""
    path = pathlib.Path(captures) / 'PROVENANCE.json'
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(current, indent=1) + '\n')
        return True
    recorded = json.loads(path.read_text())
    if recorded.get('binaries') != current['binaries']:
        print(f'corpus.py: {captures} holds captures from other binaries; use a new --captures directory',
              file=sys.stderr)
        return False
    if recorded.get('tool') != current['tool']:
        print('corpus.py: warning: the tool changed since these captures; the recorded provenance is kept',
              file=sys.stderr)
    return True


def cmd_run(args):
    entries = select(args.manifest, args.split, args.set.split(','))
    if args.limit:
        entries = entries[:args.limit]
    if not check_provenance(args.captures, provenance(args)):
        return 2
    pathlib.Path(args.work).mkdir(parents=True, exist_ok=True)
    if not args.bucket:
        raise SystemExit('corpus.py run: --bucket or R2_BUCKET names the corpus bucket')
    env, endpoint = r2_environment(args.r2_env)
    outcomes = collections.Counter()
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        futures = [pool.submit(process, e, args, env, endpoint) for e in entries]
        for number, future in enumerate(concurrent.futures.as_completed(futures), 1):
            outcomes[future.result()] += 1
            if number % 50 == 0 or number == len(futures):
                print(f'{number}/{len(futures)} {dict(outcomes)}', flush=True)
    return 0 if outcomes['download-failed'] == 0 else 2


def cmd_report(args):
    entries = select(args.manifest, args.split, args.set.split(','))
    groups = collections.defaultdict(list)
    missing = 0
    for entry in entries:
        paths = capture_paths(args.captures, entry)
        if not all(p.exists() for p in paths.values()):
            missing += 1
            continue
        split = split_of(entry)
        groups[(split, None)].append(entry)
        groups[(split, entry.get('ext') or '?')].append(entry)

    def summary_of(members):
        def load(entry):
            paths = capture_paths(args.captures, entry)
            return tuple(mo.load_json(paths[side]) for side in ('ffprobe', 'coreav', 'mediainfo'))
        items, notes = mo.three_way(members, load)
        return mo.summarize(items, notes)

    origin = pathlib.Path(args.captures) / 'PROVENANCE.json'
    recorded = json.loads(origin.read_text()) if origin.exists() else None
    report = {'schema': 'coreav-mediainfo/corpus/1', 'gating': False, 'split': args.split,
              'capture_provenance': recorded, 'manifest_files_selected': len(entries),
              'missing_captures': missing, 'groups': {}}
    text = [f'# {args.title}', '',
            f'Selected manifest entries: {len(entries)}; captured: {len(entries) - missing}; '
            f'missing: {missing}.', '',
            'Capture provenance: `' + json.dumps(recorded, sort_keys=True) + '`.', '']
    order = {'tuning': 0, 'holdout': 1, 'edge': 2}
    for (split, container) in sorted(groups, key=lambda k: (order[k[0]], k[1] or '')):
        summary = summary_of(groups[(split, container)])
        report['groups'][f'{split}/{container or "all"}'] = summary
        if container is None:
            text.append(mo.markdown(summary, f'Split `{split}`, all files').replace('\n# ', '\n## ', 1)
                        .replace('# Split', '## Split', 1).replace('\n## ', '\n### '))
    text += ['## Per container: CoreAV and ffprobe gating disagreements by MediaInfo reading', '',
             '| Split | Container | Files | ' + ' | '.join(label for _, label in mo.TRIAGE_COLUMNS) + ' |',
             '|---|---|---|' + '---|' * len(mo.TRIAGE_COLUMNS)]
    for key, summary in report['groups'].items():
        split, container = key.split('/')
        if container == 'all':
            continue
        totals = collections.Counter()
        for counts in summary['triage'].values():
            totals.update(counts)
        text.append(f"| {split} | {container} | {summary['files']} | "
                    + ' | '.join(str(totals.get(c, 0)) for c, _ in mo.TRIAGE_COLUMNS) + ' |')
    pathlib.Path(args.out).write_text(json.dumps(report, indent=1) + '\n')
    pathlib.Path(args.markdown).write_text('\n'.join(text) + '\n')
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n', 1)[0])
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('run')
    p.add_argument('--manifest', required=True)
    p.add_argument('--captures', required=True)
    p.add_argument('--work', required=True)
    p.add_argument('--r2-env', required=True)
    p.add_argument('--split', choices=['tuning', 'holdout', 'all'], default='tuning')
    p.add_argument('--set', default='real,edge')
    p.add_argument('--jobs', type=int, default=4)
    p.add_argument('--limit', type=int, default=0)
    p.add_argument('--aws', default='aws')
    p.add_argument('--bucket', default=os.environ.get('R2_BUCKET'),
                   help='the corpus bucket (default: R2_BUCKET)')
    p.add_argument('--mediainfo', default='mediainfo')
    p.add_argument('--adapter', default=str(ROOT / 'target' / 'release' / 'isobmff-adapter'))
    p.set_defaults(run=cmd_run)
    p = sub.add_parser('report')
    p.add_argument('--manifest', required=True)
    p.add_argument('--captures', required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--markdown', required=True)
    p.add_argument('--split', choices=['tuning', 'holdout', 'all'], default='tuning')
    p.add_argument('--set', default='real,edge')
    p.add_argument('--title', default='Three-way stream report on corpus v1: CoreAV, ffprobe, MediaInfo')
    p.set_defaults(run=cmd_report)
    args = parser.parse_args(argv)
    return args.run(args)


if __name__ == '__main__':
    sys.exit(main())
