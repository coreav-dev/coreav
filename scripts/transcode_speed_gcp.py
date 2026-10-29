#!/usr/bin/env python3
"""Run bar B5 of `coreav transcode` (scripts/transcode_speed.py) on a Compute Engine `c4-standard-8`
(docs/design/transcode.md, "Bars", B5).

    transcode_speed_gcp.py --selection SEL.json --work DIR --coreav PATH --x264 PATH --ffmpeg PATH \\
        --ffprobe PATH --gate FILE --out DIR [--cap-usd 6] [--max-hours H] [--classes ...] [--dry-run]

SEL.json comes from `transcode_speed.py select`. No earlier job on these VMs was given the R2 credentials, so
none is given here: the files to time are uploaded with the job instead. Each class's selected files go up, and
its reserve for the top-up (every other eligible file, ascending pixel-frames): all of it for a class whose
selection has fewer than 5 files, so that "until 5 are timed or none is left" can always be honoured there, and
the first 5 for the other classes, which need the top-up only when their own files are refused or fail
(`--reserve N` sets another count for every class). The uploaded selection says how many reserve files stayed
behind; when a class ends with fewer than 5 timed files while some did, the summary says so
(`topup_not_honoured`) and the run must be completed with those files. Files missing from WORK/files are
fetched from R2 here first (R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_ENDPOINT; never printed).

The upload holds the `coreav` build (`native-h264`, `dav1d`, `ffmpeg-fallback`, and `libvpx` for `--classes vp9`),
the pinned `ffmpeg` and `ffprobe`, the x264 program, scripts/transcode_speed.py and scripts/transcode_check.py, the files, the gate file
and a generated `job.sh`, which installs libdav1d and libvpx from Debian (the `dav1d` and `libvpx` builds link
them), records the host, and runs `transcode_speed.py run` with the two arms of the note:

    --arm ffmpeg="--encoder ffmpeg --ffmpeg bin/ffmpeg" --arm x264="--encoder x264 --x264 bin/x264"

`scripts/gcp_vm_job.py` runs it on a STANDARD (not preemptible) c4-standard-8 with a hard timeout, deletes the
VM, and writes DIR/spend.json. The hours: the note's estimate (1e-7 s a pixel-frame for ten `medium` encodes)
of the uploaded files plus half an hour, times 1.5, capped at `--cap-usd` / 0.60 USD an hour; the script
refuses to start when the estimate alone does not fit (split the run with --classes). The spend of the task so
far is `--spent-usd` plus the `usd_bound` of every earlier `spend.json` given with `--spend-json` (the runs of
this task); the script refuses when that and `--cap-usd` together pass 10 USD, the per-task limit. The exit status is that of `transcode_speed.py run` (0, 1 for a missed bar, 3 for a void run), 4 when
gcp_vm_job.py failed. `--threads-per-core 1` turns SMT off on the VM (unset: the machine type's default). `--dry-run`
builds the upload and prints the plan, without touching Compute Engine.
Uses only the Python standard library.
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
import transcode_speed as ts  # noqa: E402

MACHINE = 'c4-standard-8'
USD_PER_HOUR = 0.60
TASK_LIMIT_USD = 10.0
# Reserve files uploaded for a class whose selection is not short: enough for that many of its selected files
# to be refused or to fail.
RESERVE_MARGIN = 5


def job_script(gate_name, classes):
    arms = (f'--arm "ffmpeg=--encoder ffmpeg --ffmpeg $PWD/bin/ffmpeg" '
            f'--arm "x264=--encoder x264 --x264 $PWD/bin/x264"')
    return '\n'.join([
        'set -u',
        'mkdir -p out',
        'chmod +x bin/*',
        '(sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq libdav1d7 libvpx9) '
        '> out/apt.log 2>&1',
        '{ lscpu; nproc; free -g; uname -a; df -h /dev/shm; echo "smt active: '
        '$(cat /sys/devices/system/cpu/smt/active 2>&1)"; python3 --version; ldd bin/coreav; '
        'sha256sum bin/* transcode_speed.py transcode_check.py; bin/ffmpeg -version | head -1; } '
        '> out/host.txt 2>&1',
        'if ldd bin/coreav | grep -q "not found"; then echo "missing libraries" >> out/progress.txt; exit 2; fi',
        'export PATH="$PWD/bin:$PATH"',
        'echo "start $(date -u +%T)" >> out/progress.txt',
        f'python3 -I transcode_speed.py run --selection selection.json --files files --coreav bin/coreav '
        f'--ffmpeg bin/ffmpeg --ffprobe bin/ffprobe {arms} --gate {shlex.quote(gate_name)} --out out '
        f'--tmp /dev/shm/transcode-speed --classes {",".join(classes)} > out/speed.log 2>&1',
        'echo "rc=$? end $(date -u +%T)" >> out/progress.txt',
    ]) + '\n'


def plan(sel, classes, reserve):
    """The uploaded selection: every selected file, and the first files of each reserve."""
    up = {'params': dict(sel['params'], uploaded=True), 'classes': {}}
    for cls in classes:
        c = sel['classes'].get(cls)
        if not c:
            continue
        if reserve is not None:
            n = reserve
        elif len(c['files']) < ts.MIN_TIMED:
            n = len(c['reserve'])
        else:
            n = RESERVE_MARGIN
        up['classes'][cls] = dict(c, reserve=c['reserve'][:n], reserve_not_uploaded=max(0, len(c['reserve']) - n))
    return up


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--selection', required=True)
    ap.add_argument('--work', required=True, help='the work directory of `transcode_speed.py select` (WORK/files)')
    ap.add_argument('--coreav', required=True)
    ap.add_argument('--x264', required=True)
    ap.add_argument('--ffmpeg', required=True)
    ap.add_argument('--ffprobe', required=True)
    ap.add_argument('--gate', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--classes', default=','.join(tc.CLASSES))
    ap.add_argument('--reserve', type=int, help='reserve files uploaded a class (default: 5 - selected, plus 3)')
    ap.add_argument('--cap-usd', type=float, default=6.0)
    ap.add_argument('--spent-usd', type=float, default=0.0, help='spend of this task so far, outside --spend-json')
    ap.add_argument('--spend-json', action='append', default=[], metavar='FILE',
                    help="an earlier run's spend.json of this task (repeatable)")
    ap.add_argument('--max-hours', type=float)
    ap.add_argument('--threads-per-core', type=int, choices=[1, 2],
                    help='passed to gcp_vm_job.py: 1 turns SMT off (unset: the machine type\'s default)')
    ap.add_argument('--zone', default='us-central1-a')
    ap.add_argument('--upload-dir', help='build the upload here and keep it')
    ap.add_argument('--dry-run', action='store_true')
    a = ap.parse_args()
    classes = [c for c in a.classes.split(',') if c]
    if not set(classes) <= set(tc.CLASSES):
        ap.error(f'--classes: {tc.CLASSES}')
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
    sel = plan(json.loads(pathlib.Path(a.selection).read_text()), classes, a.reserve)
    files = [f for c in sel['classes'].values() for f in c['files'] + c['reserve']]
    # The time: the selected files and the reserve files the top-up takes when nothing is refused.
    timed = [f for c in sel['classes'].values()
             for f in c['files'] + c['reserve'][:max(0, ts.MIN_TIMED - len(c['files']))]]
    pixel_frames = sum(f['pixel_frames'] for f in timed)
    estimate = pixel_frames * ts.SECONDS_PER_PIXEL_FRAME / 3600 + 0.5
    ceiling = a.cap_usd / USD_PER_HOUR
    if estimate > ceiling:
        ap.error(f'the estimate, {estimate:.1f} h, is above {ceiling:.1f} h ({a.cap_usd:.2f} USD at '
                 f'{USD_PER_HOUR:.2f} USD an hour): split the run with --classes')
    hours = a.max_hours or min(ceiling, math.ceil(estimate * 1.5 * 4) / 4)
    if hours * USD_PER_HOUR > a.cap_usd:
        ap.error('--max-hours times 0.60 USD an hour is above --cap-usd')
    up = pathlib.Path(a.upload_dir or tempfile.mkdtemp(prefix='transcode-speed-gcp.'))
    try:
        (up / 'bin').mkdir(parents=True, exist_ok=True)
        (up / 'files').mkdir(exist_ok=True)
        for name, path in (('coreav', a.coreav), ('ffmpeg', a.ffmpeg), ('ffprobe', a.ffprobe), ('x264', a.x264)):
            shutil.copy(path, up / 'bin' / name)
        for name in ('transcode_speed.py', 'transcode_check.py'):
            shutil.copy(HERE / name, up / name)
        gate_name = 'gate' + pathlib.Path(a.gate).suffix
        shutil.copy(a.gate, up / gate_name)
        tc.ENV = tc.child_env(a.ffmpeg)
        for f in files:
            src = pathlib.Path(a.work) / 'files' / f'{f["sha256"]}.{f["ext"]}'
            dst = up / 'files' / src.name
            if not src.exists() or tc.sha256_file(src) != f['sha256']:
                if a.dry_run:
                    print(f'dry run: {f["sha256"][:12]} is not in WORK/files and would be fetched')
                    continue
                src.parent.mkdir(parents=True, exist_ok=True)
                if not tc.download(f, src):
                    raise SystemExit(f'could not fetch {f["sha256"][:12]}')
            try:
                os.link(src, dst)
            except OSError:
                shutil.copy(src, dst)
        tc._write(up / 'selection.json', sel)
        (up / 'job.sh').write_text(job_script(gate_name, classes))
        size = sum(p.stat().st_size for p in up.rglob('*') if p.is_file())
        disk = max(40, math.ceil(size / 1e9 * 1.2) + 30)
        cmd = [sys.executable, str(HERE / 'gcp_vm_job.py'), '--upload', str(up), '--job', 'job.sh',
               '--download', a.out, '--zone', a.zone, '--machine', MACHINE, '--provisioning', 'standard',
               '--usd-per-hour', str(USD_PER_HOUR), '--max-hours', str(hours), '--cap-usd', str(a.cap_usd),
               '--disk-gb', str(disk)]
        if a.threads_per_core:
            cmd += ['--threads-per-core', str(a.threads_per_core)]
        summary = {'files': len(files), 'upload_bytes': size, 'pixel_frames': pixel_frames,
                   'estimate_hours': round(estimate, 2), 'max_hours': hours,
                   'spend_bound_usd': round(hours * USD_PER_HOUR, 2), 'cap_usd': a.cap_usd, 'disk_gb': disk,
                   'spent_before_usd': round(spent, 2),
                   'per_class': {c: {'files': len(v['files']), 'reserve': len(v['reserve']),
                                     'reserve_not_uploaded': v['reserve_not_uploaded']}
                                 for c, v in sel['classes'].items()}}
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
        if not (pathlib.Path(a.out) / 'files.jsonl').exists():
            print('no results came back', flush=True)
            return 4
        rep = ts.summarize(a.out, sel, classes)
        print(ts.table(rep))
        print(json.dumps({'void': rep['void'], 'reasons': rep['reasons']}))
        return ts.status(rep)
    finally:
        if not a.upload_dir:
            shutil.rmtree(up, ignore_errors=True)


if __name__ == '__main__':
    sys.exit(main())
