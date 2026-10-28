#!/usr/bin/env python3
"""Run the Matroska-to-MP4 remux bars on corpus v1 and local recordings
(docs/design/mkv-remux.md).

`run` takes files one at a time per worker: it downloads a manifest entry
from R2 with the AWS CLI (or takes a local file), checks its SHA-256, runs
`coreav remux` with the library defaults, checks the output with
`scripts/mkv_remux_check.py` (ffprobe, ffmpeg, MediaInfo), then times
`ffmpeg -c copy -movflags +faststart` on the same streams, and deletes the
outputs and the download. Exit status, wall time and peak memory of both
sides are kept. Records stay outside git.

`report` writes counts per split against the bars: never file names or
values of private files.

R2 credentials come from R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and
R2_ENDPOINT and are passed to the AWS CLI only through its environment; the
bucket comes from R2_BUCKET.
MANIFEST is corpus v1's manifest.

  remux_corpus.py run --out DIR --work DIR [--manifest MANIFEST]
                      [--split development|holdout|all] [--local DIR]
                      [--jobs N] [--disk-gb N] [--limit N] [--only NAMES]
  remux_corpus.py report --out DIR --report REPORT.json [--markdown REPORT.md]
"""
import argparse
import collections
import concurrent.futures
import json
import os
import pathlib
import statistics
import subprocess
import sys
import tempfile
import threading
import time

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / 'scripts'))
import corpus  # noqa: E402
import mkv_remux_check as checker  # noqa: E402

TIMEOUT = 3600


class Disk:
    """Bounds the bytes of downloads and outputs held at once."""

    def __init__(self, budget):
        self.budget, self.used = budget, 0
        self.cond = threading.Condition()

    def take(self, size):
        with self.cond:
            self.cond.wait_for(lambda: self.used == 0 or self.used + size <= self.budget)
            self.used += size

    def give(self, size):
        with self.cond:
            self.used -= size
            self.cond.notify_all()


# A child forked from this Python process inherits its peak RSS in
# ru_maxrss (Linux keeps the larger of the old and new address space's
# peaks across fork and exec), so commands run under this small helper,
# whose own address space is tiny, and it reports their ru_maxrss.
PEAK_C = r"""
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc < 3) return 125;
    pid_t pid = fork();
    if (pid < 0) return 125;
    if (pid == 0) { execvp(argv[2], argv + 2); _exit(127); }
    int status; struct rusage usage;
    if (wait4(pid, &status, 0, &usage) < 0) return 125;
    FILE *f = fopen(argv[1], "w");
    if (f) { fprintf(f, "%ld\n", usage.ru_maxrss); fclose(f); }
    if (WIFSIGNALED(status)) { signal(WTERMSIG(status), SIG_DFL); raise(WTERMSIG(status)); }
    return WEXITSTATUS(status);
}
"""


def peak_helper(work):
    binary = pathlib.Path(work) / 'peak'
    if not binary.exists():
        source = pathlib.Path(work) / 'peak.c'
        source.write_text(PEAK_C)
        subprocess.run(['cc', '-O2', '-o', str(binary), str(source)], check=True)
    return binary


def run_rusage(command, helper):
    """Runs a command; returns its status, wall time and peak RSS, and its
    output streams."""
    started = time.monotonic()
    with open(os.devnull, 'rb') as stdin, tempfile.TemporaryFile() as out, \
            tempfile.TemporaryFile() as err, tempfile.NamedTemporaryFile() as peak:
        process = subprocess.Popen([str(helper), peak.name, *command], stdin=stdin, stdout=out, stderr=err,
                                   start_new_session=True)
        try:
            process.wait(TIMEOUT)
            timed_out = False
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, 9)
            process.wait()
            timed_out = True
        out.seek(0)
        err.seek(0)
        stdout, stderr = out.read(), err.read()
        text = pathlib.Path(peak.name).read_text().strip()
    return {'status': 'timeout' if timed_out else process.returncode,
            'seconds': round(time.monotonic() - started, 4),
            'max_rss_kib': int(text) if text else None}, stdout, stderr


def process(entry, args, env, endpoint, disk):
    out = pathlib.Path(args.out)
    name = entry['name']
    run_path = out / 'runs' / f'{name}.json'
    # A record is reused only for the same coreav binary and checker.
    coreav = corpus.sha256_file(args.coreav)
    check_script = corpus.sha256_file(pathlib.Path(checker.__file__))
    if run_path.exists():
        old = json.loads(run_path.read_text())
        if old.get('coreav_sha256') == coreav and old.get('checker_sha256') == check_script:
            return 'cached'
    media = pathlib.Path(entry['path']) if 'path' in entry else pathlib.Path(args.work) / name
    output = pathlib.Path(args.work) / f'{name}.coreav.mp4'
    reference = pathlib.Path(args.work) / f'{name}.ffmpeg.mp4'
    hold = entry['size'] * (3 if 'key' in entry else 2)
    disk.take(hold)
    downloaded = False
    try:
        if 'key' in entry and not media.exists():
            downloaded = True
            try:
                done = subprocess.run([args.aws, '--endpoint-url', endpoint, 's3', 'cp', '--only-show-errors',
                                       f"s3://{os.environ['R2_BUCKET']}/{entry['key']}", str(media)],
                                      env=env, capture_output=True, timeout=3600, check=False)
            except subprocess.TimeoutExpired:
                return 'download-failed'
            if done.returncode != 0:
                return 'download-failed'
        # Every manifest file is checked, a download kept from an earlier run
        # included.
        if 'key' in entry and corpus.sha256_file(media) != entry['sha256']:
            media.unlink(missing_ok=True)
            return 'sha256-mismatch'
        record = {'split': entry['split'], 'size': entry['size'], 'coreav_sha256': coreav,
                  'checker_sha256': check_script}
        run, stdout, stderr = run_rusage([args.coreav, 'remux', '--force', str(media), str(output)], args.peak)
        run['panicked'] = b'panicked' in stderr
        record['coreav'] = run
        document = None
        try:
            document = json.loads(stdout if run['status'] == 0 else stderr)
        except json.JSONDecodeError:
            pass
        if run['status'] == 0 and document is not None:
            record['status'] = 'remuxed'
            record['streams'] = [{'codec': s['codec'], 'kind': s['kind'], 'samples': s['samples']}
                                 for s in document['streams']]
            record['dropped'] = [d['message'] or d['reason'] for d in document['dropped_streams']]
            record['dropped_additions'] = len(document['dropped_boxes']) + document['dropped_boxes_not_listed']
            record['truncated'] = document['source'].get('truncated_at') is not None
            record['output_bytes'] = document['output']['bytes']
            result = checker.check(media, output, document, args.mediainfo)
            record['check'] = {'ok': result['ok'], 'packets_ok': result['packets_ok'],
                               'packets_total': result['packets_total'],
                               'fields': sorted({p['field'] for p in result['problems']}),
                               'problems': result['problems'][:5]}
            output.unlink(missing_ok=True)
            # The same streams through FFmpeg, for time and memory only.
            command = ['ffmpeg', '-nostdin', '-v', 'error', '-y', '-i', str(media)]
            for stream in document['streams']:
                command += ['-map', f"0:{stream['stream_index']}"]
            command += ['-c', 'copy', '-movflags', '+faststart', str(reference)]
            ffmpeg, _, _ = run_rusage(command, args.peak)
            record['ffmpeg'] = ffmpeg
        elif run['status'] == 1 and document is not None and document.get('kind') == 'unsupported_feature':
            # Nothing this remux copies (counted apart); any other error is
            # a failure and counts against the bars.
            record['status'] = 'refused'
            record['error'] = {'kind': document.get('kind'), 'message': document.get('message')}
        else:
            record['status'] = 'failed'
            record['error'] = {'kind': (document or {}).get('kind'),
                               'stderr': stderr[-400:].decode('utf-8', 'replace')}
        run_path.write_text(json.dumps(record) + '\n')
        return record['status']
    finally:
        output.unlink(missing_ok=True)
        reference.unlink(missing_ok=True)
        if downloaded and not args.keep:
            media.unlink(missing_ok=True)
        disk.give(hold)


def cmd_run(args):
    out = pathlib.Path(args.out)
    (out / 'runs').mkdir(parents=True, exist_ok=True)
    pathlib.Path(args.work).mkdir(parents=True, exist_ok=True)
    args.peak = peak_helper(args.work)
    args.set = 'real'
    entries = corpus.select(args)
    if args.only:
        # Re-check only the files a change affects: one name per line.
        names = set(pathlib.Path(args.only).read_text().split())
        entries = [e for e in entries if e['name'] in names]
        if args.manifest and args.local is None:
            missing = names - {e['name'] for e in entries}
            local = {n for n in missing if not n.endswith(('.mkv', '.webm')) or len(n.split('.')[0]) != 64}
            if missing - local:
                print(f'remux_corpus.py: --only names not in the selection: {sorted(missing - local)}',
                      file=sys.stderr)
                return 2
    env, endpoint = corpus.r2_environment() if args.manifest else (None, None)
    disk = Disk(args.disk_gb * 1e9)
    outcomes = collections.Counter()
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        futures = [pool.submit(process, e, args, env, endpoint, disk) for e in entries]
        for number, future in enumerate(concurrent.futures.as_completed(futures), 1):
            outcomes[future.result()] += 1
            if number % 10 == 0 or number == len(futures):
                print(f'{number}/{len(futures)} {dict(outcomes)}', flush=True)
    return 0 if not (outcomes['download-failed'] or outcomes['sha256-mismatch']) else 2


def summarize(records):
    remuxed = [r for r in records if r['status'] == 'remuxed']
    passing = sum(r['check']['ok'] for r in remuxed)
    packets = [sum(r['check']['packets_ok'] for r in remuxed), sum(r['check']['packets_total'] for r in remuxed)]
    readable = sum(not ({'layout', 'output_probe', 'mediainfo'} & set(r['check']['fields'])) for r in remuxed)
    robustness = collections.Counter()
    for r in records:
        run = r['coreav']
        if run['status'] == 'timeout':
            robustness['timeout'] += 1
        elif run['status'] not in (0, 1):
            robustness[f"exit-{run['status']}"] += 1
        if run['panicked']:
            robustness['panic'] += 1
    timed = [r for r in remuxed if r.get('ffmpeg', {}).get('status') == 0]
    speed = {}
    if timed:
        speed = {
            'files': len(timed),
            'coreav_total_s': round(sum(r['coreav']['seconds'] for r in timed), 2),
            'ffmpeg_total_s': round(sum(r['ffmpeg']['seconds'] for r in timed), 2),
            'median_speedup': round(statistics.median(r['ffmpeg']['seconds'] / max(r['coreav']['seconds'], 1e-6)
                                                      for r in timed), 2),
            'coreav_median_peak_mib': round(statistics.median(r['coreav']['max_rss_kib'] for r in timed) / 1024, 1),
            'ffmpeg_median_peak_mib': round(statistics.median(r['ffmpeg']['max_rss_kib'] for r in timed) / 1024, 1),
            'coreav_max_peak_mib': round(max(r['coreav']['max_rss_kib'] for r in timed) / 1024, 1),
            'ffmpeg_max_peak_mib': round(max(r['ffmpeg']['max_rss_kib'] for r in timed) / 1024, 1),
        }
    return {
        'files': len(records),
        'remuxed': len(remuxed),
        'passing': passing,
        'files_percent': round(100 * passing / len(remuxed), 3) if remuxed else None,
        'packets': packets,
        'packets_percent': round(100 * packets[0] / packets[1], 4) if packets[1] else None,
        'readable_layout': readable,
        'refused': dict(collections.Counter(r['error']['message'] for r in records if r['status'] == 'refused')),
        'failed': sum(r['status'] == 'failed' for r in records),
        'failing_checks': dict(collections.Counter(f for r in remuxed for f in r['check']['fields'])),
        'dropped_streams': dict(collections.Counter(d for r in remuxed for d in r['dropped'])),
        'files_with_dropped_additions': sum(r['dropped_additions'] > 0 for r in remuxed),
        'truncated_sources': sum(r['truncated'] for r in remuxed),
        'kept_codecs': dict(collections.Counter(s['codec'] for r in remuxed for s in r['streams'])),
        'robustness': dict(robustness),
        'speed': speed,
    }


def cmd_report(args):
    out = pathlib.Path(args.out)
    records = [json.loads(p.read_text()) for p in sorted((out / 'runs').glob('*.json'))]
    splits = collections.defaultdict(list)
    for record in records:
        splits[record['split']].append(record)
    groups = dict(splits)
    groups['all'] = [r for s in ('development', 'holdout', 'mediarecorder') for r in splits.get(s, [])]
    summary = {group: summarize(rs) for group, rs in sorted(groups.items())}
    pathlib.Path(args.report).write_text(json.dumps(summary, indent=1) + '\n')
    if args.markdown:
        lines = ['| Group | Files | Remuxed | Refused | Files passing | Packets agreeing | Layout and readable | '
                 'Crashes, panics, timeouts |', '|---|---|---|---|---|---|---|---|']
        for group, s in summary.items():
            refused = sum(s['refused'].values())
            lines.append(f"| {group} | {s['files']} | {s['remuxed']} | {refused} | {s['passing']}/{s['remuxed']} "
                         f"({s['files_percent']}%) | {s['packets'][0]}/{s['packets'][1]} ({s['packets_percent']}%) | "
                         f"{s['readable_layout']}/{s['remuxed']} | {s['robustness'] or 'none'} |")
        pathlib.Path(args.markdown).write_text('\n'.join(lines) + '\n')
    print(json.dumps(summary, indent=1))
    return 0


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='command', required=True)
    run = sub.add_parser('run')
    run.add_argument('--out', required=True)
    run.add_argument('--work', required=True)
    run.add_argument('--manifest')
    run.add_argument('--split', default='all', choices=['development', 'holdout', 'all'])
    run.add_argument('--local')
    run.add_argument('--jobs', type=int, default=3)
    run.add_argument('--disk-gb', type=float, default=12)
    run.add_argument('--aws', default='aws')
    run.add_argument('--limit', type=int)
    run.add_argument('--only', help='a file of names (one per line) to run, for a re-check of affected files')
    run.add_argument('--keep', action='store_true', help='keep downloads in --work')
    run.add_argument('--coreav', default=str(ROOT / 'target' / 'release' / 'coreav'))
    run.add_argument('--mediainfo', default='mediainfo')
    report = sub.add_parser('report')
    report.add_argument('--out', required=True)
    report.add_argument('--report', required=True)
    report.add_argument('--markdown')
    args = parser.parse_args()
    return cmd_run(args) if args.command == 'run' else cmd_report(args)


if __name__ == '__main__':
    sys.exit(main())
