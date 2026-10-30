#!/usr/bin/env python3
"""Run the R4 host of the 0.1.0 round (docs/measurements/compat-0.1.0/README.md, "Order", steps 2 and 3) on one
Compute Engine VM through scripts/gcp_vm_job.py: the gate on spent files, then R4 on the draw, on the same VM.

    compat_r4_gcp.py --out DIR --coreav PATH --ffmpeg PATH --ffprobe PATH [--rehearsal] [--per-family N]
                     [--rounds 5] [--max-hours 7] [--cap-usd 10] [--machine c4-standard-16 --usd-per-hour 1.20]
                     [--cpus 4-7] [--local] [--dry-run]
    compat_r4_gcp.py --collect DIR

Builds one upload directory: the three binaries, the runner (tools/compat-corpus/speed.py and what it imports, the
set, round_files.py and the corpus store's client), the two file lists (round_files.py `r4-gate` and `r4-draw6`,
chosen here from the manifests in the corpus store), the store's credentials and a generated job.sh. gcp_vm_job.py
creates the VM (STANDARD provisioning, one thread a core), runs job.sh and brings back its out/:

1. host.json: the CPU model, the vCPUs and whether SMT is on, and the sha256 of every binary;
2. the gate: the gate's files are fetched (their SHA-256 checked), then `speed.py r4 --arm aa` and `--arm null` and
   their `r4-report --gate` (gate-aa.json, gate-null.json and their .report.json);
3. only when both arms pass: the draw's files are fetched, the first read of the draw, then `speed.py r4 --arm r4`
   and its report (r4.json, r4.report.json). A failed gate leaves the draw unread.

Then the files are deleted, and verdict.json is written: `r4` when the gate passed and R4 ran on the class asked for,
`gate failed` (a gate report found the host out of its band: a host fault, the round goes on with a new VM of the
same class), or `void` (a VM of another class or with another vCPU count, an output that did not come back whole, a
missing report of either gate arm or of R4, a report that measured nothing or left a group with pairs and no ratio,
an R4 report with a failed timed run, a gate arm that did not finish, a fallback log with any line). --cap-usd,
--max-hours and --usd-per-hour must be finite and above 0. Exit status 0 for `r4`, 3 for a failed gate, 5 for a void run, 4 when
gcp_vm_job.py brought nothing back.

Limits: the VM deletes itself at --max-hours (gcp_vm_job.py's maxRunDuration and its timeout), and job.sh powers it
off itself shortly before that, so it stops even if this script and the deletion both fail. The script refuses to
start unless --max-hours times the hourly price bound fits --cap-usd, and --cap-usd is at most 10 (the per-task
limit). Its state is kept in DIR.gcpjob: when this script stops (the machine that runs it restarts), `--collect DIR`
finishes the run, as gcp_vm_job.py's own --collect does, and writes the same verdict. DIR and DIR.gcpjob name files
by hash and the cloud project: keep them private, and publish only the reports, which name files by family and
position.

--rehearsal takes round_files.py `r4-rehearsal` (spent files) in place of draw 6 and runs its R4 arm whatever the gate
says, so that a rehearsal exercises every step; the gate's verdict is still reported. --local runs job.sh in the
upload directory on this machine instead of a VM (no power-off), for a rehearsal of the job; its verdict does not
check the host. --dry-run builds the upload directory and prints what would run.
Credentials: COREAV_GCP_SA_KEY and COREAV_GCP_PROJECT (gcp_vm_job.py), and the corpus store's R2_ENDPOINT,
R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and R2_BUCKET, which the VM gets to read the files: pass read-only ones.
"""
import argparse
import hashlib
import json
import math
import os
import shlex
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
TOOLS = os.path.join(ROOT, 'tools', 'compat-corpus')
sys.path.insert(0, TOOLS)
import round_files  # noqa: E402

MACHINE = 'c4-standard-16'
USD_PER_HOUR = 1.20            # an upper bound on the on-demand price of c4-standard-16 and its disk
CAP_USD = 10.0                 # the per-task spend limit
MAX_HOURS = 7.0                # the plan's estimate of four to six hours, and a margin
DISK_GB = 60
CREDENTIALS = ('R2_ENDPOINT', 'R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY', 'R2_BUCKET')
UPLOADED = ['tools/compat-corpus/speed.py', 'tools/compat-corpus/release_round.py',
            'tools/compat-corpus/round_files.py', 'tools/compat-corpus/commands-0.1.0/commands.json',
            'tools/stage3-report/r2pool.py']


def job_script(rounds, cpus, rehearsal):
    """job.sh: `bash job.sh POWEROFF_MINUTES` (0: no power-off). Only a rehearsal, whose draw is spent files, reads
    its draw after a failed gate."""
    r4 = (f'python3 -I repo/tools/compat-corpus/speed.py r4 --coreav bin/coreav --ffmpeg bin/ffmpeg '
          f'--ffprobe bin/ffprobe --scratch scratch --rounds {rounds} --cpus {shlex.quote(cpus)}')
    report = 'python3 -I repo/tools/compat-corpus/speed.py r4-report'
    fetch = 'python3 -I repo/tools/compat-corpus/round_files.py fetch'
    return f'''set -u
mkdir -p out
if [ "${{1:-0}}" != 0 ]; then sudo shutdown -P "+$1" > out/poweroff.log 2>&1; fi
chmod 700 bin/coreav bin/ffmpeg bin/ffprobe
python3 -I - > out/host.json <<'PY'
import hashlib, json, os, platform
model = next((l.split(':', 1)[1].strip() for l in open('/proc/cpuinfo') if l.startswith('model name')), None)
smt = open('/sys/devices/system/cpu/smt/active').read().strip() if os.path.exists('/sys/devices/system/cpu/smt/active') else None
binaries = {{n: hashlib.sha256(open(f'bin/{{n}}', 'rb').read()).hexdigest() for n in ('coreav', 'ffmpeg', 'ffprobe')}}
print(json.dumps({{'cpu_model': model, 'vcpus': os.cpu_count(), 'smt_active': smt, 'kernel': platform.release(),
                  'sha256': binaries}}, indent=1))
PY
bin/ffmpeg -version | head -1 > out/ffmpeg-version.txt
set -a; . ./r2.env; set +a
finish() {{ rm -rf data scratch; shred -u r2.env 2>/dev/null || rm -f r2.env; }}
trap finish EXIT
{fetch} --files gate.json --dest data/gate > out/fetch-gate.log 2>&1 || {{ echo 'fetch failed' > out/gate.verdict; exit 1; }}
gate=pass
for arm in aa null; do
  {r4} --arm "$arm" --files data/gate/files.json --out "out/gate-$arm.json" 2> "out/gate-$arm.log" || gate=error
  rc=0
  {report} --gate --results "out/gate-$arm.json" --out "out/gate-$arm.report.json" > "out/gate-$arm.report.txt" || rc=$?
  if [ "$rc" -gt 2 ] || [ "$gate" = error ]; then gate=error; elif [ "$rc" = 2 ]; then gate=void
  elif [ "$rc" = 1 ] && [ "$gate" = pass ]; then gate=fail; fi
done
echo "$gate" > out/gate.verdict
rm -rf data/gate
[ "$gate" = pass ] || [ {'true' if rehearsal else 'false'} = true ] || exit 3
{fetch} --files draw.json --dest data/draw > out/fetch-draw.log 2>&1 || exit 1
{r4} --arm r4 --files data/draw/files.json --out out/r4.json 2> out/r4.log || exit 1
{report} --results out/r4.json --out out/r4.report.json > out/r4.report.txt
exit 0
'''


def build(args, up):
    for rel in UPLOADED:
        os.makedirs(os.path.join(up, 'repo', os.path.dirname(rel)), exist_ok=True)
        shutil.copy2(os.path.join(ROOT, rel), os.path.join(up, 'repo', rel))
    os.makedirs(os.path.join(up, 'bin'))
    for name in ('coreav', 'ffmpeg', 'ffprobe'):
        shutil.copy2(getattr(args, name), os.path.join(up, 'bin', name))
    gate = round_files.select('r4-gate', args.manifests, args.per_family)
    draw = round_files.select('r4-rehearsal' if args.rehearsal else 'r4-draw6', args.manifests, args.per_family)
    for name, files in (('gate.json', gate), ('draw.json', draw)):
        with open(os.path.join(up, name), 'w') as f:
            json.dump(files, f, indent=1)
    fd = os.open(os.path.join(up, 'r2.env'), os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as f:
        f.writelines(f'{k}={shlex.quote(os.environ[k])}\n' for k in CREDENTIALS)
    with open(os.path.join(up, 'job.sh'), 'w') as f:
        f.write(job_script(args.rounds, args.cpus, args.rehearsal))
    return gate, draw


def read(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def judge(out, rc, run):
    """verdict.json from what came back: the class of the host, the gate, R4."""
    vm = os.path.join(out, 'vm')
    if rc != 0 and not os.path.exists(os.path.join(vm, 'job.status')):
        print(f'gcp_vm_job.py exited {rc}: nothing came back, no verdict', flush=True)
        return 4
    reasons = []
    if rc != 0:
        reasons.append(f'gcp_vm_job.py exited {rc}: the output did not come back whole')
    if not run['local']:
        inst, host = read(os.path.join(vm, 'instance.json')), read(os.path.join(vm, 'host.json'))
        if inst is None or host is None:
            reasons.append('instance.json or host.json did not come back')
        inst, host = inst or {}, host or {}
        size = run['machine'].rsplit('-', 1)[-1]
        vcpus = int(size) // 2 if size.isdigit() else None
        if host.get('vcpus') != vcpus:
            reasons.append(f"the VM has {host.get('vcpus')} vCPUs, not {vcpus} (one thread a core)")
        if inst.get('machine_type') != run['machine']:
            reasons.append(f"the VM is {inst.get('machine_type')}, not {run['machine']}")
        if (inst.get('advanced_machine_features') or {}).get('threadsPerCore') != 1 or host.get('smt_active') == '1':
            reasons.append('SMT is not off')
        if inst.get('provisioning') != 'STANDARD':
            reasons.append('the VM is not on-demand')
    gate = open(os.path.join(vm, 'gate.verdict')).read().strip() if os.path.exists(os.path.join(vm, 'gate.verdict')) else None
    reports = {name: read(os.path.join(vm, f'{name}.report.json')) for name in ('gate-aa', 'gate-null', 'r4')}
    for name in ('gate-aa', 'gate-null'):
        if not reports[name]:
            reasons.append(f'{name} produced no report')
    if reports['r4'] and reports['r4'].get('incomplete'):
        reasons.append(f"r4 has {len(reports['r4']['incomplete'])} pairs whose timed run failed (a crashed run)")
    for name in ('gate-aa', 'gate-null', 'r4'):
        if reports[name] and reports[name]['fallback_log']['void']:
            reasons.append(f'the fallback log of {name} is void')
        if reports[name] and not reports[name]['classes']:
            reasons.append(f'{name} measured nothing')
        elif reports[name] and reports[name].get('unmeasured'):
            reasons.append(f"{name} left {', '.join(reports[name]['unmeasured'])} unmeasured")
    if (gate == 'pass' or run['rehearsal']) and not reports['r4']:
        reasons.append('R4 produced no report')
    gate_void = any(reports[name] and reports[name]['fallback_log']['void'] for name in ('gate-aa', 'gate-null'))
    if gate not in ('pass', 'fail') and not (gate == 'void' and gate_void):
        reasons.append(f'the gate did not finish ({gate})')
    verdict = 'void' if reasons else 'gate failed' if gate == 'fail' else 'r4'
    result = {'verdict': verdict, 'reasons': reasons, 'machine': run['machine'], 'local': run['local'],
              'rehearsal': run['rehearsal'], 'files': run['files'],
              'gate': {a: reports[f'gate-{a}'] and reports[f'gate-{a}']['failing'] for a in ('aa', 'null')},
              'r4_failing': reports['r4'] and reports['r4']['failing']}
    with open(os.path.join(out, 'verdict.json'), 'w') as f:
        json.dump(result, f, indent=1)
    print(json.dumps(result), flush=True)
    return {'r4': 0, 'gate failed': 3, 'void': 5}[verdict]


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--collect', metavar='DIR', help='finish the run of DIR after this script stopped')
    ap.add_argument('--out', help='new directory for the run (verdict.json, vm/ with the reports)')
    ap.add_argument('--coreav')
    ap.add_argument('--ffmpeg')
    ap.add_argument('--ffprobe')
    ap.add_argument('--rehearsal', action='store_true',
                    help='spent files in place of draw 6, timed whatever the gate says')
    ap.add_argument('--per-family', type=int, help='at most N files a family (a smoke run; default the set\'s 20)')
    ap.add_argument('--manifests', help='read the manifests from DIR/corpus/<name>/ instead of the corpus store')
    ap.add_argument('--rounds', type=int, default=5)
    ap.add_argument('--cpus', default='4-7')
    ap.add_argument('--machine', default=MACHINE)
    ap.add_argument('--usd-per-hour', type=float, help=f'required with another machine ({USD_PER_HOUR} for {MACHINE})')
    ap.add_argument('--zone', default='us-central1-a')
    ap.add_argument('--max-hours', type=float, default=MAX_HOURS)
    ap.add_argument('--cap-usd', type=float, default=CAP_USD)
    ap.add_argument('--local', action='store_true', help='run job.sh here, not on a VM')
    ap.add_argument('--dry-run', action='store_true')
    args = ap.parse_args()
    if args.collect:
        run = read(os.path.join(args.collect, 'run.json'))
        cmd = [sys.executable, os.path.join(HERE, 'gcp_vm_job.py'), '--collect', args.collect.rstrip('/') + '.gcpjob']
        return judge(args.collect, subprocess.run(cmd).returncode, run)
    missing = [n for n in ('out', 'coreav', 'ffmpeg', 'ffprobe') if getattr(args, n) is None]
    if missing:
        ap.error('the following arguments are required: ' + ', '.join('--' + n for n in missing))
    if args.usd_per_hour is None:
        if args.machine != MACHINE:
            ap.error('--usd-per-hour is required with a machine other than ' + MACHINE)
        args.usd_per_hour = USD_PER_HOUR
    for name in ('cap_usd', 'max_hours', 'usd_per_hour'):
        if not (math.isfinite(getattr(args, name)) and getattr(args, name) > 0):
            ap.error(f"--{name.replace('_', '-')} must be a finite number above 0")
    if args.cap_usd > CAP_USD:
        ap.error(f'--cap-usd is at most {CAP_USD:g} (the per-task spend limit)')
    if args.max_hours * args.usd_per_hour > args.cap_usd:
        ap.error(f'--max-hours times {args.usd_per_hour:.2f} USD an hour is above --cap-usd')
    if not all(os.environ.get(k) for k in CREDENTIALS):
        ap.error(', '.join(CREDENTIALS) + ' must be set: the VM reads the files from the corpus store')
    if os.path.isdir(args.out) and os.listdir(args.out):
        ap.error('--out is not empty: a run that is run again takes a new directory')
    os.makedirs(args.out, exist_ok=True)
    os.chmod(args.out, 0o700)
    up = os.path.join(args.out, 'upload')
    gate, draw = build(args, up)
    run = {'machine': args.machine, 'local': args.local, 'rehearsal': args.rehearsal, 'rounds': args.rounds,
           'cpus': args.cpus, 'files': {'gate': len(gate), 'draw': len(draw)}}
    with open(os.path.join(args.out, 'run.json'), 'w') as f:
        json.dump(run, f, indent=1)
    vm = os.path.join(args.out, 'vm')
    if args.local:
        cmd = ['bash', 'job.sh', '0']
        if args.dry_run:
            print('would run in', up, ':', shlex.join(cmd))
            return 0
        with open(os.path.join(up, 'job.log'), 'w') as log:
            rc = subprocess.run(cmd, cwd=up, stdout=log, stderr=subprocess.STDOUT,
                                timeout=int(args.max_hours * 3600)).returncode
        shutil.copytree(os.path.join(up, 'out'), vm)
        with open(os.path.join(vm, 'job.status'), 'w') as f:
            f.write(f'{rc}\n')
        os.remove(os.path.join(up, 'r2.env')) if os.path.exists(os.path.join(up, 'r2.env')) else None
        return judge(args.out, 0, run)
    poweroff = max(1, int(args.max_hours * 60) - 15)
    cmd = [sys.executable, os.path.join(HERE, 'gcp_vm_job.py'), '--upload', up, '--job', f'job.sh {poweroff}',
           '--download', vm, '--state', args.out.rstrip('/') + '.gcpjob', '--zone', args.zone,
           '--machine', args.machine, '--usd-per-hour', str(args.usd_per_hour), '--max-hours', str(args.max_hours),
           '--cap-usd', str(args.cap_usd), '--provisioning', 'standard', '--threads-per-core', '1',
           '--disk-gb', str(DISK_GB)]
    if args.dry_run:
        print('would run:', shlex.join(cmd))
        return 0
    try:
        rc = subprocess.run(cmd).returncode
    finally:
        os.remove(os.path.join(up, 'r2.env'))
    return judge(args.out, rc, run)


if __name__ == '__main__':
    sys.exit(main())
