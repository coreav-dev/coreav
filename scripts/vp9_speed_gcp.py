#!/usr/bin/env python3
"""Run scripts/vp9_speed.py (the speed run and the corpus oracle) on a Compute Engine `c4-standard-8` with SMT off.

    vp9_speed_gcp.py --selection SEL.json --work DIR --coreav-libvpx PATH --coreav-fallback PATH \\
        --decode-oracle PATH --ffmpeg PATH --ffprobe PATH --gate-sha SHA --out DIR \\
        [--phases speed,oracle] [--cap-usd 6] [--max-hours H] [--dry-run]

SEL.json comes from `vp9_speed.py select`. No job on these VMs is given the R2 credentials: the files go up with the
job, fetched from R2 here first when WORK/files lacks them (R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_ENDPOINT; never
printed). `--gate-sha` names, by SHA-256 prefix, the selected file the S1 gate times: a VP9 file whose FFmpeg S1 command
takes a few hundred milliseconds. `--set`, `--sha` and `--limit` narrow the files, as for `vp9_speed.py`.

The upload holds the two `coreav` builds (`libvpx`: `libvpx,ffmpeg-fallback`; `fallback`: `ffmpeg-fallback` only), the
`decode-oracle` build with `libvpx`,
the pinned `ffmpeg` and `ffprobe`, vp9_speed.py with the scripts it imports, the selection and the files, and a
generated `job.sh`. That installs `libvpx9` and `libdav1d7` from Debian (the builds link them), writes out/host.txt
(the processor, kernel, SMT, the Debian versions of both libraries, the libraries each program loads, the SHA-256 of
every program and script, FFmpeg's version and the decoder it picks for VP9 by default), and runs the phases of
`--phases` in order (default `speed,oracle`; the oracle phase is not timed):

    vp9_speed.py run --coreav libvpx=bin/coreav-libvpx --coreav fallback=bin/coreav-fallback --gate files/GATE ...
    vp9_speed.py oracle --decode-oracle bin/decode-oracle --jobs 2 ...

`scripts/gcp_vm_job.py` runs it on a STANDARD (not preemptible) VM with `--threads-per-core 1` and a hard timeout,
deletes the VM, and writes DIR/spend.json. The hours: a model of the run from the sizes of the uploaded files (the
set-up and upload, the thumbnail commands, the whole-stream decodes, the oracle), times 1.5, capped at what `--cap-usd`
buys; the script refuses to start when the model alone does not fit (narrow the run), and `--max-hours` sets the
hours. The spend of the task so far is `--spent-usd` plus the `usd_bound` of every earlier `spend.json` given with
`--spend-json`; the script refuses when that and `--cap-usd` together pass the per-task limit. The exit status is that
of the phases (0, 1 for a missed bar, 3 for a void run or a host with SMT on), 4 when gcp_vm_job.py failed.
`--dry-run` builds the upload plan and prints it, without touching Compute Engine. Uses only the Python standard
library.
"""
import argparse
import json
import math
import os
import pathlib
import shlex
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import transcode_check as tc  # noqa: E402
import vp9_speed as vs  # noqa: E402

MACHINE = 'c4-standard-8'
USD_PER_HOUR = 0.60
TASK_LIMIT_USD = 10.0
SETUP_HOURS = 0.15
UPLOAD_HOURS_PER_GB = 0.02
FILE_SECONDS = 10
THUMBNAIL_SECONDS_PER_MB = 0.05
DECODE_SECONDS_PER_MB_RUN = 0.08
DECODE_COMMANDS = 3
ORACLE_SECONDS = 10
ORACLE_SECONDS_PER_MB = 0.02
PROGRAMS = ('coreav-libvpx', 'coreav-fallback', 'decode-oracle', 'ffmpeg', 'ffprobe')
SCRIPTS = ('vp9_speed.py', 'thumbnail_oracle.py', 'transcode_check.py')


def job_script(args, gate_name, first_name):
    bins = ' '.join(f'bin/{n}' for n in ('coreav-libvpx', 'coreav-fallback', 'decode-oracle'))
    tools = '--ffmpeg $PWD/bin/ffmpeg --ffprobe $PWD/bin/ffprobe'
    expect = ''.join(f' --expect {shlex.quote(v)}' for v in args.expect)
    lines = [
        'set -u',
        'mkdir -p out',
        'chmod +x bin/*',
        '(sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq libvpx9 libdav1d7) '
        '> out/apt.log 2>&1',
        '{ lscpu; nproc; free -g; uname -a; df -h /dev/shm; echo "smt active: '
        '$(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; dpkg-query -W libvpx9 libdav1d7; '
        f'for b in {bins}; do echo "== $b"; ldd $b; done; '
        'sha256sum bin/* vp9_speed.py thumbnail_oracle.py transcode_check.py; bin/ffmpeg -version | head -1; '
        'bin/ffmpeg -hide_banner -decoders | grep -E " (vp9|libvpx-vp9) "; '
        f'echo "default decoder for VP9:"; bin/ffmpeg -hide_banner -v verbose -i files/{shlex.quote(first_name)} '
        '-frames:v 1 -f null - 2>&1 | grep -E "Stream #0:[0-9]+ -> #0:[0-9]+ \\(vp9"; } > out/host.txt 2>&1',
        f'if ldd {bins} | grep -q "not found"; then echo "missing libraries" >> out/progress.txt; exit 2; fi',
        'export PATH="$PWD/bin:$PATH"',
        'echo "start $(date -u +%T)" >> out/progress.txt',
    ]
    for phase in args.phases:
        if phase == 'speed':
            decode = ('--no-decode' if args.no_decode else
                      f'--decode-oracle $PWD/bin/decode-oracle --decode-rounds {args.decode_rounds}')
            lines.append(f'python3 -I vp9_speed.py run --selection selection.json --files files '
                         f'--coreav libvpx=$PWD/bin/coreav-libvpx --coreav fallback=$PWD/bin/coreav-fallback '
                         f'{tools} --gate files/{shlex.quote(gate_name)} --out out --tmp /dev/shm/vp9-speed '
                         f'{decode}{expect} '
                         f'> out/speed.log 2>&1')
        else:
            lines.append(f'python3 -I vp9_speed.py oracle --selection selection.json --files files '
                         f'--decode-oracle $PWD/bin/decode-oracle {tools} --out out --jobs 2 > out/oracle.log 2>&1')
        lines.append(f'echo "{phase} rc=$? end $(date -u +%T)" >> out/progress.txt')
    return '\n'.join(lines) + '\n'


def estimate_hours(files, args):
    seconds = 0.0
    for f in files:
        mb = f['size'] / 1e6
        if 'speed' in args.phases:
            seconds += FILE_SECONDS + THUMBNAIL_SECONDS_PER_MB * mb
            if not args.no_decode:
                seconds += DECODE_COMMANDS * (1 + args.decode_rounds) * DECODE_SECONDS_PER_MB_RUN * mb
        if 'oracle' in args.phases:
            seconds += ORACLE_SECONDS + ORACLE_SECONDS_PER_MB * mb
    return seconds / 3600 + SETUP_HOURS + UPLOAD_HOURS_PER_GB * sum(f['size'] for f in files) / 1e9


def host_problems(out):
    try:
        text = (pathlib.Path(out) / 'host.txt').read_text()
    except OSError:
        return ['no host.txt']
    problems = []
    if 'smt active: 0' not in text:
        problems.append('SMT is not off')
    if 'vp9 (native)' not in text:
        problems.append("FFmpeg's default VP9 decoder is not the native one")
    return problems


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--selection', required=True)
    ap.add_argument('--work', required=True,
                    help='the directory whose files/ holds the corpus files (fetched from R2 when missing)')
    ap.add_argument('--coreav-libvpx', required=True)
    ap.add_argument('--coreav-fallback', required=True)
    ap.add_argument('--decode-oracle', required=True)
    ap.add_argument('--ffmpeg', required=True)
    ap.add_argument('--ffprobe', required=True)
    ap.add_argument('--gate-sha', required=True, help='SHA-256 prefix of the selected file the S1 gate times')
    ap.add_argument('--out', required=True)
    ap.add_argument('--phases', default='speed,oracle')
    ap.add_argument('--set', choices=('all', 'holdout', 'dev'), default='all')
    ap.add_argument('--sha', action='append', default=[])
    ap.add_argument('--limit', type=int, default=0)
    ap.add_argument('--no-decode', action='store_true')
    ap.add_argument('--decode-rounds', type=int, default=3)
    ap.add_argument('--expect', action='append', default=[], metavar='NAME=BACKEND',
                    help="passed to vp9_speed.py: for a check of the plumbing with one build, never for a claim")
    ap.add_argument('--judge', default='libvpx')
    ap.add_argument('--judge-set', choices=('all', 'dev', 'held-out'), default='all')
    ap.add_argument('--bar', type=float, default=vs.BAR)
    ap.add_argument('--cap-usd', type=float, default=6.0)
    ap.add_argument('--spent-usd', type=float, default=0.0, help='spend of this task so far, outside --spend-json')
    ap.add_argument('--spend-json', action='append', default=[], metavar='FILE',
                    help="an earlier run's spend.json of this task (repeatable)")
    ap.add_argument('--max-hours', type=float)
    ap.add_argument('--zone', default='us-central1-a')
    ap.add_argument('--upload-dir', help='build the upload here and keep it')
    ap.add_argument('--dry-run', action='store_true')
    a = ap.parse_args()
    a.phases = [p for p in a.phases.split(',') if p]
    if not a.phases or not set(a.phases) <= {'speed', 'oracle'}:
        ap.error('--phases: speed, oracle, or both, in the order to run')
    if not 0 < a.cap_usd <= TASK_LIMIT_USD or a.spent_usd < 0:
        ap.error(f'--cap-usd is above 0 and at most {TASK_LIMIT_USD} (the per-task spend limit)')
    spent = a.spent_usd
    for f in a.spend_json:
        try:
            spent += float(json.loads(pathlib.Path(f).read_text())['usd_bound'])
        except (OSError, ValueError, KeyError, TypeError):
            ap.error(f'{f} is not a spend.json with usd_bound')
    if spent + a.cap_usd > TASK_LIMIT_USD:
        ap.error(f'{spent:.2f} USD spent so far and a cap of {a.cap_usd:.2f} USD pass the per-task limit of '
                 f'{TASK_LIMIT_USD} USD: lower --cap-usd')
    if not a.dry_run and os.path.isdir(a.out) and os.listdir(a.out):
        ap.error('--out is not empty: a run that is run again takes a new directory')
    sel = json.loads(pathlib.Path(a.selection).read_text())
    gate = [f for f in sel['files'] if f['sha256'].startswith(a.gate_sha)]
    if len(gate) != 1:
        ap.error('--gate-sha names no selected file, or more than one')
    a.selection_args = argparse.Namespace(selection=a.selection, set=a.set, sha=a.sha, limit=a.limit)
    files = vs.queue_of(a.selection_args)
    if gate[0] not in files:
        files = sorted(files + gate, key=lambda f: f['sha256'])
    estimate = estimate_hours(files, a)
    ceiling = a.cap_usd / USD_PER_HOUR
    if estimate > ceiling:
        ap.error(f'the estimate, {estimate:.1f} h, is above {ceiling:.1f} h ({a.cap_usd:.2f} USD at '
                 f'{USD_PER_HOUR:.2f} USD an hour): narrow the run')
    hours = a.max_hours or min(ceiling, math.ceil(estimate * 1.5 * 4) / 4)
    if hours * USD_PER_HOUR > a.cap_usd:
        ap.error('--max-hours times 0.60 USD an hour is above --cap-usd')
    up = pathlib.Path(a.upload_dir or tempfile.mkdtemp(prefix='vp9-speed-gcp.'))
    try:
        (up / 'bin').mkdir(parents=True, exist_ok=True)
        (up / 'files').mkdir(exist_ok=True)
        for name, path in zip(PROGRAMS, (a.coreav_libvpx, a.coreav_fallback, a.decode_oracle, a.ffmpeg, a.ffprobe)):
            shutil.copy(path, up / 'bin' / name)
        for name in SCRIPTS:
            shutil.copy(HERE / name, up / name)
        for f in files:
            name = f'{f["sha256"]}.{f["ext"]}'
            src = pathlib.Path(a.work) / 'files' / name
            if not src.exists() or tc.sha256_file(src) != f['sha256']:
                if a.dry_run:
                    print(f'dry run: {f["sha256"][:12]} is not in WORK/files and would be fetched')
                    continue
                src.parent.mkdir(parents=True, exist_ok=True)
                if not tc.download(f, src):
                    raise SystemExit(f'could not fetch {f["sha256"][:12]}')
            try:
                os.link(src, up / 'files' / name)
            except OSError:
                shutil.copy(src, up / 'files' / name)
        tc._write(up / 'selection.json', dict(sel, files=files))
        gate_name = f'{gate[0]["sha256"]}.{gate[0]["ext"]}'
        (up / 'job.sh').write_text(job_script(a, gate_name, gate_name))
        size = sum(p.stat().st_size for p in up.rglob('*') if p.is_file())
        disk = max(40, math.ceil(size / 1e9 * 1.2) + 30)
        cmd = [sys.executable, str(HERE / 'gcp_vm_job.py'), '--upload', str(up), '--job', 'job.sh',
               '--download', a.out, '--zone', a.zone, '--machine', MACHINE, '--provisioning', 'standard',
               '--threads-per-core', '1', '--usd-per-hour', str(USD_PER_HOUR), '--max-hours', str(hours),
               '--cap-usd', str(a.cap_usd), '--disk-gb', str(disk)]
        summary = {'files': len(files), 'held_out': sum(1 for f in files if f['holdout']), 'upload_bytes': size,
                   'phases': a.phases, 'estimate_hours': round(estimate, 2), 'max_hours': hours, 'disk_gb': disk}
        print(json.dumps(summary, indent=1), flush=True)
        os.makedirs(a.out, exist_ok=True)
        tc._write(pathlib.Path(a.out) / 'plan.json', summary)
        shutil.copy(up / 'job.sh', pathlib.Path(a.out) / 'job.sh')
        if a.dry_run:
            print('dry run:', ' '.join(shlex.quote(c) for c in cmd))
            return 0
        rc = subprocess.run(cmd).returncode
        if rc != 0:
            print(f'gcp_vm_job.py exited {rc}', flush=True)
            return 4
        problems = host_problems(a.out)
        for p in problems:
            print(f'host problem: {p}')
        rep = vs.summarize(a.out)
        print(vs.report(rep))
        print(json.dumps({'void': rep['void'], 'reasons': rep['reasons']}))
        if problems:
            return 3
        return vs.status(rep, a, tuple(a.phases))
    finally:
        if not a.upload_dir:
            shutil.rmtree(up, ignore_errors=True)


if __name__ == '__main__':
    sys.exit(main())
