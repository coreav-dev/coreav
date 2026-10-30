#!/usr/bin/env python3
"""Run the packet-listing speed round of docs/design/mpegts.md ("Speed bars, round 2") on a Compute Engine VM of the
one-thread class at a fixed frequency (docs/design/h264.md), and write its verdict.

    ts_speed_gcp.py --out DIR --coreav BIN --lav-dir DIR [--coreav-cli BIN] [--rounds 7] [--cap-usd 2] FILE...
    ts_speed_gcp.py --collect DIR

Builds one upload directory (CoreAV's `ts-speed` binary and its byte-identical copy for the null arm, the directory
tools/ts-speed/build-lav.sh writes, `scripts/ts_speed.py`, the files and a generated `job.sh`) and runs
`scripts/gcp_vm_job.py` with a `c4-standard-8`, one thread a core, `turboMode=ALL_CORE_MAX`, STANDARD provisioning
and the spend bound checked against --cap-usd before the VM is made. On the VM, `job.sh` records the host
(`host.txt`: lscpu, the SMT state, the library identity, the sha256 of every binary), installs perf for the frequency
samples, and runs `ts_speed.py` on core 2 into `report.json`. With --coreav-cli, the command arms time that CLI's
`probe` and the `ffprobe` of the same FFmpeg build (DIR/bin/ffprobe) as information.

DIR gets the VM's output, `run.json` (the files and the options) and `verdict.json`. The run is void, a host fault that
lets it be run again on a new VM into a new DIR, when the VM does not report `turboMode` ALL_CORE_MAX or a Xeon 8581C,
does not have 4 vCPUs with SMT off, a frequency sample is missing or more than 3% from 3.1 GHz, the timing ended early
or left a file out, or the null arm's times are outside their band (scripts/ts_speed_bars.py). The run failed, and
stands as it is, when `ts_speed_bars.py` refuses the report of the files it timed: a run of an engine failed or has no
time, CoreAV and its null copy list different access units, or a file is over the large class. Those are the
engines' outcome, not the host's, so a failure is never void, whatever host fault is recorded beside it. The exit
status is 0 for a run that is not void, 3 for a void run, 5 for a failed run and 4 when `gcp_vm_job.py` failed (no
verdict; a reason outside the VM). `--dry-run` builds the
upload and prints what would run. When this script stops before the end, `--collect DIR` finishes the run through
`gcp_vm_job.py --collect DIR.gcpjob` and writes the verdict from `run.json`; it is safe to run again. DIR.gcpjob names
the cloud project: keep it out of published results. The bars themselves are judged by `ts_speed_bars.py
DIR/report.json`.
"""
import argparse
import json
import math
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import ts_speed_bars  # noqa: E402

MACHINE = 'c4-standard-8'
USD_PER_HOUR = 0.60
VCPUS = 4
CORE = 2
CPU_MATCH = '8581C'
FREQ = 3.1
FREQ_TOL = 0.03
CAP_LIMIT = 10.0


def job_script(run):
    command = ['python3', 'ts_speed.py', '--coreav', './ts-speed', '--null', './ts-speed-null', '--lav', './lav/lav_ts',
               '--core', str(CORE), '--rounds', str(run['rounds']), '--tmp', '/dev/shm', '--report', 'out/report.json']
    if run['commands']:
        command += ['--coreav-cli', './coreav', '--ffprobe', './lav/bin/ffprobe']
    command += [f'files/{name}' for name in run['files']]
    return '\n'.join([
        'set -u', 'mkdir -p out', 'chmod +x ts-speed ts-speed-null lav/lav_ts' + (' coreav lav/bin/ffprobe' if run['commands'] else ''),
        '{ lscpu; nproc; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version;'
        ' lav/lav_ts --identity; sha256sum ts-speed ts-speed-null lav/lav_ts lav/lib/* ts_speed.py'
        + (' coreav lav/bin/ffprobe' if run['commands'] else '') + '; } > out/host.txt 2>&1',
        '{ sudo apt-get -q update && sudo DEBIAN_FRONTEND=noninteractive apt-get -q -y install linux-perf'
        ' && sudo sysctl -q kernel.perf_event_paranoid=1; } > out/perf-install.log 2>&1',
        'echo "start $(date -u +%T)" > out/progress.txt',
        ' '.join(shlex.quote(c) for c in command) + ' > out/ts_speed.log 2>&1',
        'echo "rc=$? $(date -u +%T)" >> out/progress.txt', ''])


def problems(out, run):
    """Why the run is void, from what the VM recorded."""
    found = []
    try:
        with open(os.path.join(out, 'instance.json')) as f:
            features = json.load(f).get('advanced_machine_features') or {}
        if features.get('turboMode') != 'ALL_CORE_MAX':
            found.append('the VM does not report turboMode ALL_CORE_MAX')
    except (OSError, ValueError, AttributeError):
        found.append('no instance.json: the turbo mode of the VM is not on record')
    try:
        with open(os.path.join(out, 'host.txt')) as f:
            host = f.read()
        cpus = re.search(r'^CPU\(s\):\s+(\d+)', host, re.M)
        model = re.search(r'^Model name:\s+(.*)$', host, re.M)
        smt = re.search(r'^smt active: (\S+)', host, re.M)
        if not model or CPU_MATCH not in model.group(1):
            found.append(f'the CPU is {model.group(1).strip() if model else "not on record"}, not a Xeon {CPU_MATCH}')
        if not cpus or int(cpus.group(1)) != VCPUS:
            found.append(f'the VM has {cpus.group(1) if cpus else "an unknown number of"} vCPUs, not {VCPUS}')
        if not smt or smt.group(1) != '0':
            found.append('SMT is not off')
    except OSError:
        found.append('no host.txt')
    try:
        with open(os.path.join(out, 'progress.txt')) as f:
            if not re.search(r'^rc=0 ', f.read(), re.M):
                found.append('the timing did not end with status 0')
    except OSError:
        found.append('no progress.txt')
    try:
        with open(os.path.join(out, 'report.json')) as f:
            report = json.load(f)
    except (OSError, ValueError):
        return found + ['no report.json']
    timed = [f.get('name') for f in report.get('files', [])]
    if timed != run['files']:
        found.append(f'the report has {len(timed)} of the {len(run["files"])} files, or not in their order')
    labels = [s.get('label') for s in report.get('freq', [])]
    for label in ['start'] + [f'after {name}' for name in run['files']]:
        if label not in labels:
            found.append(f'no frequency sample {label!r}')
    for s in report.get('freq', []):
        ghz = s.get('ghz')
        if not isinstance(ghz, (int, float)) or not math.isfinite(ghz) or abs(ghz / FREQ - 1) > FREQ_TOL:
            found.append(f'frequency sample {s.get("label")!r}: {ghz if ghz is not None else s.get("error")}, '
                         f'not {FREQ} GHz within {FREQ_TOL:.0%}')
    if report.get('files'):
        try:
            summary = ts_speed_bars.analyze(report)
            if summary['void']:
                found.append(f'the null arm is {summary["null"]["median"]:.4f}, outside {ts_speed_bars.NULL_BAND}')
        except ts_speed_bars.Refused:
            pass
    return found


def failures(out):
    """Why the run failed, from what the engines did on the files it timed: a refused report (an engine's run failed
    or has no time, CoreAV and its null copy list different access units, a file is over the large class). These are
    not host faults, so they do not void the run: it stands, as a failure."""
    try:
        with open(os.path.join(out, 'report.json')) as f:
            report = json.load(f)
    except (OSError, ValueError):
        return []
    if not report.get('files'):
        return []
    try:
        ts_speed_bars.analyze(report)
    except ts_speed_bars.Refused as e:
        return [f'the report is refused: {e}']
    return []


def judge(out, rc, run):
    if rc != 0:
        print(f'gcp_vm_job.py exited {rc}: no verdict', flush=True)
        return 4
    failed = failures(out)
    reasons = problems(out, run)
    with open(os.path.join(out, 'verdict.json'), 'w') as f:
        json.dump({'failed': bool(failed), 'failures': failed, 'void': bool(reasons) and not failed,
                   'reasons': reasons}, f, indent=1)
    for failure in failed:
        print('failed:', failure, flush=True)
    for reason in reasons:
        print('host fault:' if not failed else 'host fault (the run failed all the same):', reason, flush=True)
    return 5 if failed else 3 if reasons else 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--out')
    ap.add_argument('--collect', metavar='DIR')
    ap.add_argument('--coreav')
    ap.add_argument('--lav-dir')
    ap.add_argument('--coreav-cli')
    ap.add_argument('--rounds', type=int, default=7)
    ap.add_argument('--zone', default='us-central1-a')
    ap.add_argument('--max-hours', type=float, default=2.0)
    ap.add_argument('--cap-usd', type=float, default=2.0)
    ap.add_argument('--dry-run', action='store_true')
    ap.add_argument('files', nargs='*')
    args = ap.parse_args()
    if args.collect:
        with open(os.path.join(args.collect, 'run.json')) as f:
            run = json.load(f)
        rc = subprocess.run([sys.executable, os.path.join(HERE, 'gcp_vm_job.py'), '--collect',
                             args.collect.rstrip('/') + '.gcpjob']).returncode
        return judge(args.collect, rc, run)
    if not (args.out and args.coreav and args.lav_dir and args.files):
        ap.error('--out, --coreav, --lav-dir and at least one file are required')
    if os.path.exists(args.out):
        ap.error(f'{args.out} exists: every run takes a new directory')
    if args.cap_usd > CAP_LIMIT:
        ap.error(f'--cap-usd is at most {CAP_LIMIT:.0f} (the per-task spend limit)')
    if args.max_hours * USD_PER_HOUR > args.cap_usd:
        ap.error(f'--max-hours times {USD_PER_HOUR:.2f} USD an hour is above --cap-usd')
    names = [os.path.basename(p) for p in args.files]
    if len(set(names)) != len(names):
        ap.error('two files have the same name')
    run = {'files': names, 'rounds': args.rounds, 'commands': bool(args.coreav_cli)}
    up = tempfile.mkdtemp(prefix='ts-speed-upload-')
    try:
        shutil.copy(args.coreav, os.path.join(up, 'ts-speed'))
        shutil.copy(args.coreav, os.path.join(up, 'ts-speed-null'))
        shutil.copytree(args.lav_dir, os.path.join(up, 'lav'), symlinks=True)
        if args.coreav_cli:
            shutil.copy(args.coreav_cli, os.path.join(up, 'coreav'))
        shutil.copy(os.path.join(HERE, 'ts_speed.py'), os.path.join(up, 'ts_speed.py'))
        os.makedirs(os.path.join(up, 'files'))
        for path in args.files:
            shutil.copy(path, os.path.join(up, 'files', os.path.basename(path)))
        with open(os.path.join(up, 'job.sh'), 'w') as f:
            f.write(job_script(run))
        size = sum(os.path.getsize(os.path.join(r, n)) for r, _, ns in os.walk(up) for n in ns)
        cmd = [sys.executable, os.path.join(HERE, 'gcp_vm_job.py'), '--upload', up, '--job', 'job.sh',
               '--download', args.out, '--state', args.out.rstrip('/') + '.gcpjob', '--zone', args.zone,
               '--machine', MACHINE, '--provisioning', 'standard', '--threads-per-core', '1', '--turbo-mode',
               'ALL_CORE_MAX', '--usd-per-hour', str(USD_PER_HOUR), '--max-hours', str(args.max_hours),
               '--cap-usd', str(args.cap_usd)]
        print(f'upload {size / 1e6:.0f} MB, {len(names)} files, {args.rounds} rounds; spend bound '
              f'{args.max_hours * USD_PER_HOUR:.2f} USD of {args.cap_usd:.2f}', flush=True)
        os.makedirs(args.out)
        shutil.copy(os.path.join(up, 'job.sh'), os.path.join(args.out, 'job.sh'))
        with open(os.path.join(args.out, 'run.json'), 'w') as f:
            json.dump(run, f, indent=1)
        if args.dry_run:
            print('dry run:', ' '.join(shlex.quote(c) for c in cmd))
            return 0
        return judge(args.out, subprocess.run(cmd).returncode, run)
    finally:
        shutil.rmtree(up, ignore_errors=True)


if __name__ == '__main__':
    sys.exit(main())
