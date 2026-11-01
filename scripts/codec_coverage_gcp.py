#!/usr/bin/env python3
"""The run of the 0.1.0 codec coverage bars around the workflow
(docs/measurements/compat-0.1.0/codec-coverage-plan.md): the record of a host, the run manifest, and the rerun of
V1's host-limited files on one Compute Engine VM through scripts/gcp_vm_job.py.

    codec_coverage_gcp.py host --job NAME --oracle DIR [--fixtures DIR] [--bin DIR]
    codec_coverage_gcp.py manifest --build FILE --public DIR... --set SET --shards N --evidence-key SHA256 --out FILE
    codec_coverage_gcp.py rerun --pending FILE --manifest FILE --bin DIR --out DIR [--jobs 4]
                                [--max-hours 8] [--cap-usd 10] [--machine c4-standard-16 --usd-per-hour 1.20]
                                [--local] [--dry-run]
    codec_coverage_gcp.py collect DIR

host prints one job's host as JSON: the runner image and its version, the CPU model, the cores, the memory and the
kernel, the version line and SHA-256 of the pinned ffmpeg and ffprobe in DIR, with --fixtures the SHA-256 of the five
inputs of V2 and V3, with --bin the SHA-256 of decode-oracle there.

manifest puts the build's part (written by the workflow's build job) and the hosts (every host-*.json of the --public
directories, with the V1 shards' counts, v1-*.json; a directory that is missing or empty adds nothing, as for a pull
request, which runs no V1 shard) together into the run manifest
and checks it against the plan's table: the full build with release-full and the three libraries, decode-oracle
with native-h264, dav1d and libvpx, the ffmpeg test executable with release-full, x264 at the pin's commit, one
pinned ffmpeg and ffprobe on every host, the pin's build in their version line, the evidence key's fingerprint,
the five fixtures, and a count from every V1 shard. Prints a Markdown summary; exit status 1 when an item differs.

rerun runs V1 again, both sides, on the files decode_breadth.py pending lists (a host limit on the runners, or no
record), on a STANDARD c4-standard-16 with the run's own decode-oracle, ffmpeg and ffprobe, which it checks
against the manifest first: 6 hours a side (the plan's "Host limits"), the VM fetching each file from the corpus
store with the read-only credentials in R2_ENDPOINT, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and R2_BUCKET. It
writes DIR/vm/rerun.jsonl (decode_breadth.py records), DIR/vm/host.json and DIR/verdict.json: `complete` when
every pending file has a record that is a result, else `incomplete` with the count (a file of the set with no
result after the rerun voids the run). --max-hours bounds the VM (it deletes itself then, and job.sh powers it off
shortly before); the script refuses to start unless --max-hours times --usd-per-hour fits --cap-usd, which is at
most 10. collect finishes a rerun after this script stopped, as gcp_vm_job.py --collect does. DIR, DIR.gcpjob and
the pending list name files by hash and the cloud project: keep them private.
Credentials: COREAV_GCP_SA_KEY and COREAV_GCP_PROJECT (gcp_vm_job.py).
"""
import argparse
import hashlib
import json
import math
import os
import pathlib
import platform
import re
import shlex
import shutil
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
MACHINE = 'c4-standard-16'
USD_PER_HOUR = 1.20            # an upper bound on the on-demand price of c4-standard-16 and its disk
CAP_USD = 10.0                 # the per-task spend limit
MAX_HOURS = 8.0                # 6 hours a side, both sides of a file at once, and the fetches
SIDE_MINUTES = 360
DISK_GB = 100                  # --jobs files of at most 6 GB on disk at once
CREDENTIALS = ('R2_ENDPOINT', 'R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY', 'R2_BUCKET')
FIXTURES = ('basic.mp4', 'mkv_h264_aac.mkv', 'webm_vp9_opus.webm', 'ts_h264_aac.ts', 'audio_aac.m4a')
BINARIES = ('decode-oracle', 'ffmpeg', 'ffprobe')
UPLOADED = ['scripts/codec_coverage_gcp.py', 'scripts/thumbnail_oracle.py',
            'tools/compat-corpus/decode_breadth.py', 'tools/compat-corpus/decode_reasons.json',
            'tools/compat-corpus/round_files.py',
            'tools/compat-corpus/release_round.py', 'tools/stage3-report/r2pool.py']


def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def host(job, oracle, fixtures=None, binaries=None):
    cpu = mem = None
    if os.path.exists('/proc/cpuinfo'):
        cpu = next((l.split(':', 1)[1].strip() for l in open('/proc/cpuinfo') if l.startswith('model name')), None)
        mem = next((int(l.split()[1]) for l in open('/proc/meminfo') if l.startswith('MemTotal:')), None)
    oracle = pathlib.Path(oracle)
    out = {'job': job, 'image': os.environ.get('ImageOS'), 'image_version': os.environ.get('ImageVersion'),
           'cpu_model': cpu, 'cores': os.cpu_count(), 'memory_kib': mem, 'kernel': platform.release(),
           'ffmpeg': {n: {'sha256': sha256(oracle / n),
                          'version': subprocess.run([str(oracle / n), '-version'], capture_output=True,
                                                    text=True).stdout.partition('\n')[0]}
                      for n in ('ffmpeg', 'ffprobe')}}
    if fixtures:
        out['fixtures'] = {n: sha256(pathlib.Path(fixtures) / n) for n in FIXTURES}
    if binaries:
        out['decode-oracle'] = sha256(pathlib.Path(binaries) / 'decode-oracle')
    return out


def pin(root=ROOT):
    text = (pathlib.Path(root) / 'scripts' / 'oracle-ffmpeg.env').read_text()
    return dict(re.findall(r'^([A-Z][A-Z0-9_]*)=(\S+)$', text, re.M))


def codec_versions(root=ROOT):
    text = (pathlib.Path(root) / 'packages' / 'cli' / 'scripts' / 'codecs.sh').read_text()
    return {name: re.search(rf'^{var}=(\S+)$', text, re.M).group(1)
            for name, var in (('dav1d', 'DAV1D_VERSION'), ('libvpx', 'LIBVPX_VERSION'), ('libopus', 'OPUS_VERSION'))}


def manifest_problems(build, hosts, set_name, shards, shard_counts, key, root=ROOT):
    """What differs from the plan's run manifest, as sentences; none when it agrees."""
    problems = []
    coreav, oracle = build.get('coreav', {}), build.get('decode-oracle', {})
    if coreav.get('features') != 'release-full' or coreav.get('profile') != 'dist':
        problems.append('coreav is not the full build (release-full, dist)')
    if coreav.get('libraries') != codec_versions(root):
        problems.append('coreav does not record the dav1d, libvpx and libopus versions codecs.sh pins')
    if set(oracle.get('features', '').split(',')) != {'native-h264', 'dav1d', 'libvpx'} or \
            oracle.get('target') != 'x86_64-unknown-linux-musl' or oracle.get('profile') != 'dist':
        problems.append('decode-oracle is not built with native-h264, dav1d and libvpx for the musl target')
    if build.get('ffmpeg-harness', {}).get('features') != 'release-full':
        problems.append('the ffmpeg test executable is not built with release-full')
    if build.get('x264', {}).get('commit') != pin(root).get('X264_COMMIT'):
        problems.append('x264 is not built at the commit the pin names')
    if build.get('evidence_key_sha256') != key:
        problems.append('the evidence key is not the one the records are sealed to')
    for name in ('coreav', 'decode-oracle', 'x264', 'ffmpeg-harness'):
        if not re.fullmatch(r'[0-9a-f]{64}', build.get(name, {}).get('sha256') or ''):
            problems.append(f'{name} has no SHA-256')
    if not hosts:
        problems.append('no host was recorded')
    if len({json.dumps(h['ffmpeg'], sort_keys=True) for h in hosts}) > 1:
        problems.append('the jobs ran different ffmpeg or ffprobe binaries')
    build_id = pin(root).get('BUILD', '')
    if any(build_id not in v['version'] for h in hosts for v in h['ffmpeg'].values()):
        problems.append(f'an ffmpeg or ffprobe is not the pinned build {build_id}')
    fixtures = [h for h in hosts if 'fixtures' in h]
    if len(fixtures) != 1 or sorted(fixtures[0]['fixtures']) != sorted(FIXTURES):
        problems.append('the five fixtures of V2 and V3 are not recorded once')
    if set_name != 'fixtures' and sorted(c['shard'] for c in shard_counts) != list(range(shards)):
        problems.append(f'{shards - len(shard_counts)} of {shards} V1 shards reported no count: '
                        'their files go to the rerun host')
    return problems


def manifest(args):
    build = json.loads(pathlib.Path(args.build).read_text())
    found = lambda pattern: [json.loads(p.read_text()) for d in args.public for p in sorted(pathlib.Path(d).glob(pattern))]
    hosts, counts = found('host-*.json'), found('v1-*.json')
    problems = manifest_problems(build, hosts, args.set, args.shards, counts, args.evidence_key)
    first = hosts[0]['ffmpeg'] if hosts else None
    fixtures = next((h['fixtures'] for h in hosts if 'fixtures' in h), None)
    out = {**build, 'set': args.set, 'ffmpeg': first, 'fixtures': fixtures,
           'hosts': [{k: v for k, v in h.items() if k not in ('ffmpeg', 'fixtures')} for h in hosts],
           'v1_shards': sorted(counts, key=lambda c: c['shard']), 'problems': problems}
    pathlib.Path(args.out).write_text(json.dumps(out, indent=1) + '\n')
    print(f"## codec-coverage: {args.set} at {build.get('commit', '')[:12]}\n")
    print(f"coreav (full) sha256 `{build.get('coreav', {}).get('sha256')}`, decode-oracle "
          f"`{build.get('decode-oracle', {}).get('sha256')}`, evidence key `{build.get('evidence_key_sha256')}`.\n")
    if counts:
        total = {k: sum(c[k] for c in counts) for k in ('files', 'records', 'pending')}
        print(f"V1 on the runners: {total['files']} files, {total['records']} records, "
              f"{total['pending']} left for the rerun host.\n")
    print('Manifest: ' + ('; '.join(problems) if problems else 'as the plan names it') + '.')
    return 1 if problems else 0


def job_script(jobs):
    """job.sh: `bash job.sh POWEROFF_MINUTES` (0: no power-off). The runner stops starting files, and stops the sides
    still running, ten minutes before the power-off."""
    return f'''set -u
mkdir -p out
if [ "${{1:-0}}" != 0 ]; then sudo shutdown -P "+$1" > out/poweroff.log 2>&1; fi
deadline=0
[ "${{1:-0}}" != 0 ] && deadline=$(( $(date +%s) + ($1 - 10) * 60 ))
chmod 700 bin/decode-oracle bin/ffmpeg bin/ffprobe
python3 -I repo/scripts/codec_coverage_gcp.py host --job rerun --oracle bin --bin bin > out/host.json
finish() {{ rm -rf data; shred -u r2.env 2>/dev/null || rm -f r2.env; }}
trap finish EXIT
set -a; . ./r2.env; set +a
deadline_args=()
[ "$deadline" != 0 ] && deadline_args=(--deadline "$deadline")
python3 -I repo/tools/compat-corpus/decode_breadth.py run --decode-oracle bin/decode-oracle --ffmpeg bin/ffmpeg \\
  --ffprobe bin/ffprobe --files pending.json --fetch data --limit-minutes {SIDE_MINUTES} --rerun-host ${{deadline_args[@]+"${{deadline_args[@]}}"}} \\
  --records out/rerun.jsonl --jobs {jobs} 2> out/rerun.log
'''


def check_binaries(bin_dir, manifest_):
    """The binaries differ from the run's, as sentences."""
    want = {'decode-oracle': manifest_['decode-oracle']['sha256'],
            'ffmpeg': manifest_['ffmpeg']['ffmpeg']['sha256'], 'ffprobe': manifest_['ffmpeg']['ffprobe']['sha256']}
    return [f'{name} is not the run\'s' for name in BINARIES
            if not (pathlib.Path(bin_dir) / name).is_file() or sha256(pathlib.Path(bin_dir) / name) != want[name]]


def build(args, up, pending):
    for rel in UPLOADED:
        (up / 'repo' / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / rel, up / 'repo' / rel)
    (up / 'bin').mkdir()
    for name in BINARIES:
        shutil.copy2(pathlib.Path(args.bin) / name, up / 'bin' / name)
    (up / 'pending.json').write_text(json.dumps(pending, indent=1) + '\n')
    fd = os.open(up / 'r2.env', os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as f:
        f.writelines(f'{k}={shlex.quote(os.environ[k])}\n' for k in CREDENTIALS)
    (up / 'job.sh').write_text(job_script(args.jobs))


def judge(out, rc, run):
    """verdict.json: whether every pending file came back with a result."""
    out = pathlib.Path(out)
    vm = out / 'vm'
    if rc != 0 and not (vm / 'job.status').exists():
        print(f'gcp_vm_job.py exited {rc}: nothing came back, no verdict', flush=True)
        return 4
    reasons = []
    if rc != 0:
        reasons.append(f'gcp_vm_job.py exited {rc}: the output did not come back whole')
    if not run['local']:
        try:
            inst = json.loads((vm / 'instance.json').read_text())
        except (OSError, ValueError):
            inst = {}
        if inst.get('machine_type') != run['machine']:
            reasons.append(f"the VM is {inst.get('machine_type')}, not {run['machine']}")
    pending = json.loads((out / 'upload' / 'pending.json').read_text())
    records = {}
    if (vm / 'rerun.jsonl').exists():
        for line in (vm / 'rerun.jsonl').read_text().splitlines():
            if line.strip():
                r = json.loads(line)
                records[r.get('sha256')] = r
    missing = sum(1 for e in pending if e['sha256'] not in records or records[e['sha256']].get('host_limit'))
    if missing:
        reasons.append(f'{missing} of {len(pending)} files have no result')
    verdict = 'incomplete' if reasons else 'complete'
    result = {'verdict': verdict, 'reasons': reasons, 'machine': run['machine'], 'local': run['local'],
              'files': len(pending), 'without_result': missing}
    (out / 'verdict.json').write_text(json.dumps(result, indent=1) + '\n')
    print(json.dumps(result), flush=True)
    return 0 if verdict == 'complete' else 5


def rerun(args, ap):
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
    if args.max_hours * 60 - 10 < SIDE_MINUTES + 30:
        ap.error(f'--max-hours leaves less than {SIDE_MINUTES} minutes a side')
    if not all(os.environ.get(k) for k in CREDENTIALS):
        ap.error(', '.join(CREDENTIALS) + ' must be set: the VM reads the files from the corpus store')
    pending = json.loads(pathlib.Path(args.pending).read_text())
    if not pending:
        ap.error('--pending lists no file: there is nothing to rerun')
    if any(not {'sha256', 'key', 'ext', 'size'} <= set(e) for e in pending):
        ap.error('--pending entries need sha256, key, ext and size (decode_breadth.py pending)')
    wrong = check_binaries(args.bin, json.loads(pathlib.Path(args.manifest).read_text()))
    if wrong:
        ap.error('; '.join(wrong))
    out = pathlib.Path(args.out)
    if out.is_dir() and any(out.iterdir()):
        ap.error('--out is not empty: a rerun that is run again takes a new directory')
    out.mkdir(parents=True, exist_ok=True)
    os.chmod(out, 0o700)
    up = out / 'upload'
    build(args, up, pending)
    run = {'machine': args.machine, 'local': args.local, 'files': len(pending), 'jobs': args.jobs}
    (out / 'run.json').write_text(json.dumps(run, indent=1) + '\n')
    vm = out / 'vm'
    if args.local:
        cmd = ['bash', 'job.sh', '0']
        if args.dry_run:
            print('would run in', up, ':', shlex.join(cmd))
            (up / 'r2.env').unlink()
            return 0
        with open(up / 'job.log', 'w') as log:
            rc = subprocess.run(cmd, cwd=up, stdout=log, stderr=subprocess.STDOUT,
                                timeout=int(args.max_hours * 3600)).returncode
        shutil.copytree(up / 'out', vm)
        (vm / 'job.status').write_text(f'{rc}\n')
        if (up / 'r2.env').exists():
            (up / 'r2.env').unlink()
        return judge(out, 0, run)
    poweroff = max(1, int(args.max_hours * 60) - 15)
    cmd = [sys.executable, str(HERE / 'gcp_vm_job.py'), '--upload', str(up), '--job', f'job.sh {poweroff}',
           '--download', str(vm), '--state', str(out) + '.gcpjob', '--zone', args.zone, '--machine', args.machine,
           '--usd-per-hour', str(args.usd_per_hour), '--max-hours', str(args.max_hours), '--cap-usd', str(args.cap_usd),
           '--provisioning', 'standard', '--disk-gb', str(DISK_GB)]
    if args.dry_run:
        print('would run:', shlex.join(cmd))
        (up / 'r2.env').unlink()
        return 0
    try:
        rc = subprocess.run(cmd).returncode
    finally:
        if (up / 'r2.env').exists():
            (up / 'r2.env').unlink()
    return judge(out, rc, run)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    sub = ap.add_subparsers(dest='mode', required=True)
    p = sub.add_parser('host')
    p.add_argument('--job', required=True)
    p.add_argument('--oracle', required=True)
    p.add_argument('--fixtures')
    p.add_argument('--bin')
    p = sub.add_parser('manifest')
    p.add_argument('--build', required=True)
    p.add_argument('--public', nargs='+', required=True, help='directories of the jobs\' public artifacts')
    p.add_argument('--set', required=True, choices=('fixtures', 'rehearsal', 'v1'))
    p.add_argument('--shards', type=int, required=True)
    p.add_argument('--evidence-key', required=True)
    p.add_argument('--out', required=True)
    p = sub.add_parser('rerun')
    p.add_argument('--pending', required=True)
    p.add_argument('--manifest', required=True)
    p.add_argument('--bin', required=True, help='decode-oracle, ffmpeg and ffprobe of the run')
    p.add_argument('--out', required=True)
    p.add_argument('--jobs', type=int, default=4)
    p.add_argument('--machine', default=MACHINE)
    p.add_argument('--usd-per-hour', type=float, help=f'required with another machine ({USD_PER_HOUR} for {MACHINE})')
    p.add_argument('--zone', default='us-central1-a')
    p.add_argument('--max-hours', type=float, default=MAX_HOURS)
    p.add_argument('--cap-usd', type=float, default=CAP_USD)
    p.add_argument('--local', action='store_true', help='run job.sh here, not on a VM')
    p.add_argument('--dry-run', action='store_true')
    p = sub.add_parser('collect')
    p.add_argument('dir')
    args = ap.parse_args()
    if args.mode == 'host':
        print(json.dumps(host(args.job, args.oracle, args.fixtures, args.bin), indent=1))
        return 0
    if args.mode == 'manifest':
        return manifest(args)
    if args.mode == 'collect':
        run = json.loads((pathlib.Path(args.dir) / 'run.json').read_text())
        cmd = [sys.executable, str(HERE / 'gcp_vm_job.py'), '--collect', args.dir.rstrip('/') + '.gcpjob']
        return judge(args.dir, subprocess.run(cmd).returncode, run)
    return rerun(args, ap)


if __name__ == '__main__':
    sys.exit(main())
