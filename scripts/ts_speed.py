#!/usr/bin/env python3
"""Time the packet listing of MPEG-TS files, CoreAV against libavformat, on one core (docs/design/mpegts.md, "Speed bars,
round 2"). Runs on the measuring host; scripts/ts_speed_gcp.py runs it on a Compute Engine VM of the bar's class.

    ts_speed.py --coreav BIN --null BIN --lav BIN --report OUT.json [--core 2] [--rounds 7] [--tmp /dev/shm]
                [--coreav-cli BIN --ffprobe BIN] FILE...

The engines are three in-process arms, each a program that prints one JSON line {"ok", "ns", ...} with the time it
measured around the operation: CoreAV (tools/ts-speed, `packets FILE`), the null arm (a second, byte-identical copy of
CoreAV's binary) and libavformat (tools/ts-speed/lav_ts.c, `packets FILE`). With --coreav-cli and --ffprobe, two more
arms time, as information and as whole commands, `coreav probe FILE` and `ffprobe -v error -of json -show_format
-show_streams FILE`. A run whose process exits with a status other than 0 is recorded as failed, whatever its JSON
line says.

One file at a time: it is copied to --tmp (a tmpfs) and read once, then each arm runs once as a warm-up and then
--rounds times, the order of the arms rotated by one each round; every run is pinned to --core with taskset. After
each file, and once before the first, the frequency of the core is sampled (`perf stat -e cycles,task-clock` over
half a second of a busy loop on the core, cycles over task-clock: a run of a few milliseconds is mostly the start of a
process, and its ratio reads low on a core that holds its frequency). The report holds every run's time and output and every sample; it is
written after each file, so a run cut short keeps the files it finished. scripts/ts_speed_bars.py judges it.
"""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time

LISTING = ('coreav', 'null', 'lav')
COMMANDS = ('probe-coreav', 'probe-ffprobe')


def pinned(core, argv):
    return ['taskset', '-c', str(core)] + argv


def run_arm(arm, args, path):
    """One run of an arm: its JSON line (the in-process arms) or the command's wall time (the command arms)."""
    if arm in COMMANDS:
        argv = ([args.coreav_cli, 'probe', path] if arm == 'probe-coreav' else
                [args.ffprobe, '-v', 'error', '-of', 'json', '-show_format', '-show_streams', path])
        start = time.perf_counter_ns()
        r = subprocess.run(pinned(args.core, argv), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        return {'ok': r.returncode == 0, 'ns': time.perf_counter_ns() - start}
    binary = {'coreav': args.coreav, 'null': args.null, 'lav': args.lav}[arm]
    r = subprocess.run(pinned(args.core, [binary, 'packets', path]), capture_output=True, text=True)
    try:
        line = json.loads(r.stdout.strip().splitlines()[-1])
    except (IndexError, ValueError):
        return {'ok': False, 'ns': None, 'error': f'exit {r.returncode}, no JSON line'}
    result = {k: line.get(k) for k in ('ok', 'ns', 'count', 'bytes', 'error') if k in line}
    if r.returncode != 0:
        result['ok'] = False
        result['error'] = f'exit {r.returncode}' + (f': {result["error"]}' if result.get('error') else '')
    return result


SPIN = 'import time\nstart = time.perf_counter()\nwhile time.perf_counter() - start < 0.5:\n    pass\n'


def frequency(args, label):
    """A frequency sample of the timing core: cycles over task-clock of half a second of a busy loop on it."""
    sample = {'label': label, 'time': time.strftime('%H:%M:%SZ', time.gmtime())}
    try:
        r = subprocess.run(['perf', 'stat', '-x,', '-e', 'cycles,task-clock', '--'] +
                           pinned(args.core, [sys.executable, '-c', SPIN]),
                           capture_output=True, text=True, check=True)
        counts = {}
        for line in r.stderr.splitlines():
            fields = line.split(',')
            if len(fields) >= 3 and fields[0].replace('.', '', 1).isdigit():
                counts[fields[2].split(':')[0]] = float(fields[0])
        sample['ghz'] = counts['cycles'] / (counts['task-clock'] * 1e6)
    except (OSError, subprocess.CalledProcessError, KeyError, ZeroDivisionError) as e:
        sample['error'] = type(e).__name__
    return sample


def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--coreav', required=True)
    ap.add_argument('--null', required=True)
    ap.add_argument('--lav', required=True)
    ap.add_argument('--coreav-cli')
    ap.add_argument('--ffprobe')
    ap.add_argument('--report', required=True)
    ap.add_argument('--core', type=int, default=2)
    ap.add_argument('--rounds', type=int, default=7)
    ap.add_argument('--tmp', default='/dev/shm')
    ap.add_argument('files', nargs='+')
    args = ap.parse_args()
    if (args.coreav_cli is None) != (args.ffprobe is None):
        ap.error('--coreav-cli and --ffprobe go together')
    if args.rounds < 1:
        ap.error('--rounds is at least 1')
    if sha256(args.coreav) != sha256(args.null):
        ap.error('the null arm must be a byte-identical copy of the CoreAV binary')
    arms = LISTING + (COMMANDS if args.ffprobe else ())
    report = {'core': args.core, 'rounds': args.rounds, 'arms': list(arms), 'files': [], 'freq': []}

    def save():
        with open(args.report + '.part', 'w') as f:
            json.dump(report, f, indent=1)
        os.replace(args.report + '.part', args.report)

    work = os.path.join(args.tmp, 'ts-speed')
    os.makedirs(work, exist_ok=True)
    for i, src in enumerate(args.files):
        path = os.path.join(work, os.path.basename(src))
        shutil.copyfile(src, path)
        with open(path, 'rb') as f:
            while f.read(1 << 20):
                pass
        if i == 0:
            report['freq'].append(frequency(args, 'start'))
        entry = {'name': os.path.basename(src), 'bytes': os.path.getsize(path), 'sha256': sha256(path),
                 'warm': {arm: run_arm(arm, args, path) for arm in arms}, 'runs': {arm: [] for arm in arms}}
        for r in range(args.rounds):
            for arm in arms[r % len(arms):] + arms[:r % len(arms)]:
                entry['runs'][arm].append(run_arm(arm, args, path))
        report['files'].append(entry)
        report['freq'].append(frequency(args, f'after {entry["name"]}'))
        os.remove(path)
        save()
        print(f'{i + 1}/{len(args.files)} {entry["name"]}', flush=True)
    save()
    return 0


if __name__ == '__main__':
    sys.exit(main())
