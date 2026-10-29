#!/usr/bin/env python3
"""Run the eight-thread arms of an A/B comparison on a Compute Engine VM (docs/design/h264.md, "Where the
eight-thread arms run"), or, with `--one-thread-class`, its one-thread arms (same document, "The one-thread class of rounds whose bars are
committed from here on").

    h264_parity_gcp.py --out DIR --es-dir ES --decoder NAME=PATH [--decoder ...] --ffmpeg PATH \\
        --gate-stream PATH [--null-arm NAME] [--rounds 10] [--seed N] [--cap-usd 5]

Builds one upload directory (the decoders, FFmpeg, `scripts/h264_parity.py`, the elementary streams of the
calls, the gate stream and a generated `job.sh`), runs `scripts/gcp_vm_job.py` with a `c4-standard-16`, one
thread a core, STANDARD provisioning, and reads the reports it brings back into DIR: `t8.<class>.json` (`tN.` with `--threads N`) and
`.log` for each class, `host.txt` (the CPU, SMT state, sha256 of every file uploaded, the streams included), `ref.json` (the gate
stream's FFmpeg time, eight threads, before the first call and after the last), `spend.json`, and
`verdict.json`: the run is void when the two reference times differ by more than 5%, when a call has more than 2
unresolved streams, when a call produced no report or fewer streams than it was given, or when the VM does not
have 8 vCPUs with SMT off. The exit status is 0 for a run that is not void, 3 for a void run (its files stay in
DIR, which must be new: a run that is run again takes a new DIR), and 4 when `gcp_vm_job.py` itself failed (no
verdict is written). `--dry-run` builds the upload directory and prints what would run, without touching
Compute Engine.

`--one-thread-class` runs the one-thread arms on a `c4-standard-8` with one thread a core (4 vCPUs, the CPU class of the
one-thread bars): each call is `--pin 2 --threads 1` with no repeat, the reports are `g.<class>.json`, and the host-health
gate is that of `docs/measurements/h264-rounds/host-gate/` with the class's own references, three measurements of two reference
streams (`--ref-stream PATH:FRAMES`, twice) five minutes apart before the first call, a check before and after every call, and a
call whose after-check fails is run again (at most 3 attempts; the voided files stay as `*.void<N>`). The run is void when the
three references disagree by more than 5%, when a call never passed its checks, or by the rules above (no reference drift rule:
the checks stand in for it).

Every call is `--threads 8 --min-seconds 3 --shuffle-seed N --null-arm A` with the in-call gate and the bars of
that host (`--null-bar 0.02 --spread-bar 1.25`), one call per class at its frame count. `--threads N` (and
`--pin CORE`) time another thread count on the same host, for a one-thread diagnostic; its reports are
`tN.<class>.json`, and with `--pin` the in-call gate runs at one thread on that core.
"""
import argparse
import glob
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
FRAMES = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
CLASSES = tuple(FRAMES)
MACHINE = 'c4-standard-16'
USD_PER_HOUR = 1.20            # an upper bound on the on-demand price of c4-standard-16 and its disk
MACHINE_1T = 'c4-standard-8'   # the one-thread class: one thread a core, 4 vCPUs
USD_PER_HOUR_1T = 0.60         # an upper bound on the on-demand price of c4-standard-8 and its disk
GATE_ATTEMPTS = 3
GATE1T = r'''"""gate1t.py ref|check LABEL: the host-health gate (docs/measurements/h264-rounds/host-gate/) on this VM's own references.
ref: one measurement (FFmpeg, taskset -c <the call's core>, -threads 1, median of 3, each reference stream) appended to out/gate-refs.json.
check: a measurement compared with the median of the references, appended to out/gate-checks.log; exit 0 on PASS (all within 5%)."""
import json, os, statistics, subprocess, sys, time
R = json.load(open('refs/refs.json'))
S, CORE = R['streams'], str(R['core'])
def measure():
    out = {}
    for n, f in S.items():
        ts = []
        for _ in range(3):
            t = time.perf_counter()
            subprocess.run(['taskset', '-c', CORE, './ffmpeg', '-nostdin', '-v', 'error', '-threads', '1', '-i', f'refs/{n}',
                            '-frames:v', str(f), '-f', 'null', '-'], check=True, stderr=subprocess.DEVNULL)
            ts.append(time.perf_counter() - t)
        out[n] = statistics.median(ts)
    return out
mode, label = sys.argv[1], sys.argv[2]
m = measure(); now = time.strftime('%H:%M:%SZ', time.gmtime())
if mode == 'ref':
    p = 'out/gate-refs.json'
    d = json.load(open(p)) if os.path.exists(p) else []
    d.append({'label': label, 'time': now, **m}); json.dump(d, open(p, 'w'), indent=1); print(label, m)
else:
    refs = json.load(open('out/gate-refs.json'))
    ref = {n: statistics.median(r[n] for r in refs) for n in S}
    ratios = {n: m[n] / ref[n] for n in S}
    ok = all(abs(r - 1) <= 0.05 for r in ratios.values())
    with open('out/gate-checks.log', 'a') as f:
        f.write(('PASS ' if ok else 'FAIL ') + label + ' ' + now + ' ' + '; '.join(f'{n} {m[n]:.3f}s ratio {ratios[n]:.3f}' for n in S) + '\n')
    sys.exit(0 if ok else 1)
'''
CPU_MATCH = '8581C'            # the CPU of the calibration (Xeon Platinum 8581C @ 2.30 GHz)
REF_DRIFT = 0.05
MAX_UNRESOLVED = 2


def job_script_1t(args, calls, decoders):
    """The `job.sh` of the one-thread class: host facts, three references five minutes apart, then per class a gate
    check, the call, a gate check; a call whose after-check fails is run again (GATE_ATTEMPTS)."""
    lines = ['set -u', 'mkdir -p out', 'chmod +x decode-* ffmpeg',
             '{ lscpu; nproc; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)";'
             ' python3 --version; sha256sum decode-* ffmpeg h264_parity.py gate1t.py gate.h264 refs/* es/*; } > out/host.txt 2>&1',
             'echo "start $(date -u +%T)" >> out/progress.txt',
             'for i in 1 2 3; do', '  python3 gate1t.py ref "ref$i" >> out/progress.txt 2>&1',
             '  [ $i -lt 3 ] && sleep ${REF_SLEEP:-300}', 'done', 'echo "refs done $(date -u +%T)" >> out/progress.txt']
    for cls, files in calls:
        flags = (f'--ffmpeg ./ffmpeg --frames {FRAMES[cls]} --pin {args.pin} --rounds {args.rounds} '
                 f'--shuffle-seed {args.seed} --gate-stream gate.h264 --gate-frames {args.gate_frames} --gate-every 4 '
                 f'--null-bar {args.null_bar} --spread-bar {args.spread_bar} --null-arm {args.null_arm}')
        if args.cpu_match:
            flags += f' --require-cpu {shlex.quote(args.cpu_match)}'
        tag = f'g.{cls}'
        lines += [f'for attempt in {" ".join(str(i) for i in range(1, GATE_ATTEMPTS + 1))}; do',
                  f'  ok=0; for i in 1 2 3 4 5; do python3 gate1t.py check "before {cls} #$attempt" && {{ ok=1; break; }}; '
                  f'[ $i -lt 5 ] && sleep ${{CHECK_SLEEP:-60}}; done',
                  # a call does not start on a host that fails its gate (HOST-GATE.md)
                  f'  if [ $ok = 0 ]; then echo "{tag} attempt $attempt: the host failed its gate, no call" >> out/progress.txt; continue; fi',
                  f'  echo "{tag} attempt $attempt $(date -u +%T)" >> out/progress.txt',
                  f'  python3 h264_parity.py {decoders} {flags} --report out/{tag}.json '
                  + ' '.join(shlex.quote(f) for f in files) + f' > out/{tag}.log 2>&1',
                  f'  echo "{tag} rc=$? $(date -u +%T)" >> out/progress.txt',
                  f'  if [ -f out/{tag}.json ] && python3 gate1t.py check "after {cls} #$attempt"; then touch out/{tag}.gate_ok; break; fi',
                  f'  [ -f out/{tag}.json ] && mv out/{tag}.json out/{tag}.json.void$attempt',
                  f'  [ -f out/{tag}.log ] && mv out/{tag}.log out/{tag}.log.void$attempt',
                  'done']
    lines.append('echo "end $(date -u +%T)" >> out/progress.txt')
    return '\n'.join(lines) + '\n'


def job_script(args, calls):
    """The `job.sh` that runs on the VM: host facts, the reference before, one call a class, the reference after."""
    names = [n for n, _ in args.decoders]
    decoders = ' '.join(f'--decoder {n}=./decode-{n}' for n in names)
    if args.one_thread_class:
        return job_script_1t(args, calls, decoders)
    ref = ('python3 - "$1" <<\'PY\'\n'
           'import argparse, json, sys\n'
           'sys.path.insert(0, ".")\n'
           'import h264_parity as h\n'
           'ns = argparse.Namespace(ffmpeg="./ffmpeg", gate_stream="gate.h264", gate_frames='
           f'{args.gate_frames}, gate_threads=8, gate_pin=None, prefix=[], min_seconds={args.min_seconds})\n'
           'median, repeat = h.gate_sample(ns)\n'
           'try:\n'
           '    d = json.load(open("out/ref.json"))\n'
           'except OSError:\n'
           '    d = {}\n'
           'd[sys.argv[1]] = {"median": median, "repeat": repeat}\n'
           'json.dump(d, open("out/ref.json", "w"), indent=1)\n'
           'PY\n')
    lines = ['set -u', 'mkdir -p out', 'chmod +x decode-* ffmpeg',
             '{ lscpu; nproc; uname -a; echo "smt active: $(cat /sys/devices/system/cpu/smt/active 2>&1)";'
             ' python3 --version; sha256sum decode-* ffmpeg h264_parity.py gate.h264 es/*; } > out/host.txt 2>&1',
             'ref() {', ref.rstrip('\n'), '}',
             'ref before']
    for cls, files in calls:
        flags = (f'--ffmpeg ./ffmpeg --frames {FRAMES[cls]} --threads {args.threads} --rounds {args.rounds} '
                 f'--min-seconds {args.min_seconds} --shuffle-seed {args.seed} '
                 f'--gate-stream gate.h264 --gate-frames {args.gate_frames} --gate-every 4 '
                 # A pinned call puts its gate on the same core at one thread (h264_parity.py takes no
                 # --gate-threads with --pin).
                 + ('' if args.pin is not None else '--gate-threads 8 ') +
                 f'--null-bar {args.null_bar} --spread-bar {args.spread_bar}')
        if args.null_arm:
            flags += f' --null-arm {args.null_arm}'
        if args.pin is not None:
            flags += f' --pin {args.pin}'
        if args.cpu_match:
            flags += f' --require-cpu {shlex.quote(args.cpu_match)}'
        tag = f't{args.threads}.{cls}'
        lines.append(f'echo "{tag} $(date -u +%T)" >> out/progress.txt')
        lines.append(f'python3 h264_parity.py {decoders} {flags} --report out/{tag}.json '
                     + ' '.join(shlex.quote(f) for f in files) + f' > out/{tag}.log 2>&1')
        lines.append(f'echo "{tag} rc=$?" >> out/progress.txt')
    lines += ['ref after', 'echo "end $(date -u +%T)" >> out/progress.txt']
    return '\n'.join(lines) + '\n'


def host_problems(out, vcpus=8):
    """What host.txt says is wrong with the VM: not `vcpus` vCPUs, or SMT on."""
    try:
        with open(os.path.join(out, 'host.txt')) as f:
            text = f.read()
    except OSError:
        return ['no host.txt']
    cpus = re.search(r'^CPU\(s\):\s+(\d+)', text, re.M)
    smt = re.search(r'^smt active: (\S+)', text, re.M)
    problems = []
    if not cpus or int(cpus.group(1)) != vcpus:
        problems.append(f'the VM has {cpus.group(1) if cpus else "an unknown number of"} vCPUs, not {vcpus}')
    if not smt or smt.group(1) != '0':
        problems.append('SMT is not off')
    return problems


def refs_problems(out):
    """The one-thread class: its three references (gate-refs.json) must agree within 5% of their median, per stream."""
    try:
        with open(os.path.join(out, 'gate-refs.json')) as f:
            refs = json.load(f)
        names = [k for k in refs[0] if k not in ('label', 'time')]
        if len(refs) != 3 or not names:
            return ['the host needs three reference measurements']
        out_ = []
        for n in names:
            v = [r[n] for r in refs]
            m = sorted(v)[1]
            if not all(math.isfinite(t) and abs(t / m - 1) <= REF_DRIFT for t in v):
                out_.append(f'the references of {n} disagree by more than 5%: {v}')
        return out_
    except (OSError, KeyError, IndexError, TypeError, ValueError, ZeroDivisionError):
        return ['no usable gate references']


def verdict(out, calls, threads=8, one_thread_class=False):
    """Whether the run is void, and why, from the files that came back. `calls` is the list of (class, files),
    `threads` the count each call timed (its reports are `t<threads>.<class>.json`); with `one_thread_class` the reports
    are `g.<class>.json`, each call must have passed its gate checks (`g.<class>.gate_ok`) and the three references
    must agree, in place of the reference before and after."""
    reasons = host_problems(out, 4 if one_thread_class else 8)
    drift = None
    prefix = 'g' if one_thread_class else f't{threads}'
    if one_thread_class:
        reasons += refs_problems(out)
        for cls, _ in calls:
            if not os.path.exists(os.path.join(out, f'g.{cls}.gate_ok')):
                reasons.append(f'g.{cls}: no attempt passed its gate checks')
    else:
        try:
            with open(os.path.join(out, 'ref.json')) as f:
                ref = json.load(f)
            drift = ref['after']['median'] / ref['before']['median'] - 1
            if not math.isfinite(drift):
                raise ValueError
            if abs(drift) > REF_DRIFT:
                reasons.append(f'the reference moved {drift:+.1%} between the first call and the last')
        except (OSError, KeyError, TypeError, ValueError, ZeroDivisionError):
            drift = None
            reasons.append('no usable reference before and after')
    unresolved = {}
    for cls, files in calls:
        try:
            with open(os.path.join(out, f'{prefix}.{cls}.json')) as f:
                report = json.load(f)
            n = report['method']['unresolved']
            got = len(report['streams'])
            failed = [f"{f['stream']} ({f['reason']})" for f in report.get('failed') or []]
        except (OSError, KeyError, TypeError, ValueError):
            reasons.append(f'no report for {prefix}.{cls}')
            continue
        unresolved[cls] = n
        if got != len(files) or failed:
            reasons.append(f'{prefix}.{cls}: {got} of {len(files)} streams in the report (failed: '
                           f'{"; ".join(failed) or "a stream, the report does not say which"})')
        if n > MAX_UNRESOLVED:
            reasons.append(f'{prefix}.{cls}: {n} unresolved streams')
    v = {'void': bool(reasons), 'reasons': reasons, 'reference_drift': drift, 'unresolved': unresolved}
    with open(os.path.join(out, 'verdict.json'), 'w') as f:
        json.dump(v, f, indent=1)
    return v


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--out', required=True, help='directory that receives the reports and verdict.json')
    ap.add_argument('--es-dir', required=True, help='elementary streams named <id>.<track>.<class>.h264')
    ap.add_argument('--decoder', action='append', required=True, metavar='NAME=PATH',
                    help='NAME is letters, digits, - and _')
    ap.add_argument('--ffmpeg', required=True, help='the FFmpeg binary to upload (a static build)')
    ap.add_argument('--gate-stream', required=True, help='the reference stream of the in-call gate and of the host check')
    ap.add_argument('--gate-frames', type=int, default=900)
    ap.add_argument('--null-arm', required=True, metavar='NAME', help='the decoder whose binary is timed twice')
    ap.add_argument('--classes', default=','.join(CLASSES))
    ap.add_argument('--limit-per-class', type=int, metavar='N', help='the first N streams of each class (a smoke run)')
    ap.add_argument('--rounds', type=int, default=10)
    ap.add_argument('--min-seconds', type=float, default=None, help='default 3 s (none with --one-thread-class)')
    ap.add_argument('--seed', type=int, default=20261026)
    ap.add_argument('--threads', type=int, default=None,
                    help='threads of every timed command (the reference stays at 8, and the in-call gate too '
                         'unless --pin is given); reports are t<threads>.<class>.json (default 8, 1 with --one-thread-class)')
    ap.add_argument('--pin', type=int, metavar='CORE', help='run every timed command on this core (taskset -c)')
    ap.add_argument('--one-thread-class', action='store_true',
                    help='the one-thread arms on a c4-standard-8 with one thread a core (see the module docstring); needs '
                         'two --ref-stream; --pin defaults to 2')
    ap.add_argument('--ref-stream', action='append', default=[], metavar='PATH:FRAMES',
                    help='a reference stream of the host-health gate and its frame count (twice, with --one-thread-class)')
    ap.add_argument('--null-bar', type=float, default=None, help='default 0.02 (0.04 with --one-thread-class)')
    ap.add_argument('--spread-bar', type=float, default=None, help='default 1.25 (1.50 with --one-thread-class)')
    ap.add_argument('--cpu-match', default=CPU_MATCH, help="a substring of the VM's CPU model ('' to skip the check)")
    ap.add_argument('--zone', default='us-central1-a',
                    help='the zone of the VM (the calibration\'s by default; another zone of the same machine type, '
                         'whose CPU --cpu-match checks, when the region\'s quota is taken)')
    ap.add_argument('--max-hours', type=float, default=2.5)
    ap.add_argument('--cap-usd', type=float, default=5.0)
    ap.add_argument('--upload-dir', help='build the upload directory here and keep it (with --dry-run: to run job.sh '
                                         'elsewhere)')
    ap.add_argument('--dry-run', action='store_true')
    args = ap.parse_args()
    if args.one_thread_class:
        args.machine, args.usd_per_hour = MACHINE_1T, USD_PER_HOUR_1T
        if args.threads not in (None, 1):
            ap.error('--one-thread-class times one thread')
        args.threads = 1
        if args.pin is None:
            args.pin = 2
        if args.pin > 3:
            ap.error('--pin is a core of the 4 vCPUs, 0 to 3')
        if args.min_seconds is not None:
            ap.error('--min-seconds does not apply to --one-thread-class (its calls do not repeat)')
        args.min_seconds = 0.0
        if args.null_bar is None:
            args.null_bar = 0.04
        if args.spread_bar is None:
            args.spread_bar = 1.50
        if len(args.ref_stream) != 2:
            ap.error('--one-thread-class needs two --ref-stream PATH:FRAMES')
        refs = []
        for r in args.ref_stream:
            path, _, frames = r.rpartition(':')
            if not path or not (frames.isascii() and frames.isdigit()) or int(frames) < 1 or not os.path.isfile(path):
                ap.error(f'--ref-stream {r!r}: PATH:FRAMES with an existing PATH')
            refs.append((path, int(frames)))
        args.refs = refs
    else:
        args.machine, args.usd_per_hour = MACHINE, USD_PER_HOUR
        if args.ref_stream:
            ap.error('--ref-stream is for --one-thread-class')
        if args.threads is None:
            args.threads = 8
        if args.min_seconds is None:
            args.min_seconds = 3
        if args.null_bar is None:
            args.null_bar = 0.02
        if args.spread_bar is None:
            args.spread_bar = 1.25
    args.decoders = [tuple(d.split('=', 1)) for d in args.decoder if '=' in d]
    if len(args.decoders) != len(args.decoder):
        ap.error('--decoder takes NAME=PATH')
    names = [n for n, _ in args.decoders]
    if not all(re.fullmatch(r'[A-Za-z0-9_][A-Za-z0-9_-]*', n) for n in names) or len(set(names)) != len(names):
        ap.error('decoder names are distinct, of letters, digits, - and _, and do not start with -')
    if args.null_arm not in [n for n, _ in args.decoders]:
        ap.error('--null-arm is not one of the decoders')
    classes = list(dict.fromkeys(c for c in args.classes.split(',') if c))
    if not classes or not set(classes) <= set(CLASSES):
        ap.error(f'--classes: {CLASSES}')
    for name in ('rounds', 'null_bar', 'spread_bar', 'max_hours', 'cap_usd', 'gate_frames', 'threads'):
        v = getattr(args, name)
        if not (0 < v < math.inf):
            ap.error(f'--{name.replace("_", "-")} must be a positive number')
    if not (0 <= args.min_seconds < math.inf):
        ap.error('--min-seconds must be a number, 0 or more')
    if args.pin is not None and args.pin < 0:
        ap.error('--pin must be a core number, 0 or more')
    if args.limit_per_class is not None and args.limit_per_class < 1:
        ap.error('--limit-per-class must be at least 1')
    if args.cap_usd > 10:
        ap.error('--cap-usd is at most 10 (the per-task spend limit)')
    if args.max_hours * args.usd_per_hour > args.cap_usd:
        ap.error(f'--max-hours times {args.usd_per_hour:.2f} USD an hour is above --cap-usd: a request that does not fit is '
                 'split by the caller into runs (--classes, --limit-per-class)')
    if not args.dry_run and os.path.isdir(args.out) and os.listdir(args.out):
        ap.error('--out is not empty: a run that is run again takes a new directory')
    if args.upload_dir and os.path.isdir(args.upload_dir) and os.listdir(args.upload_dir):
        ap.error('--upload-dir is not empty')

    up = args.upload_dir or tempfile.mkdtemp(prefix='parity-gcp.')
    try:
        os.makedirs(os.path.join(up, 'es'), exist_ok=True)
        calls = []
        for cls in classes:
            files = sorted(glob.glob(os.path.join(glob.escape(args.es_dir), f'*.{cls}.h264')))
            if args.limit_per_class:
                files = files[:args.limit_per_class]
            if not files:
                ap.error(f'no {cls} streams in {args.es_dir}')
            for f in files:
                shutil.copy(f, os.path.join(up, 'es', os.path.basename(f)))
            calls.append((cls, [f'es/{os.path.basename(f)}' for f in files]))
        for n, path in args.decoders:
            shutil.copy(path, os.path.join(up, f'decode-{n}'))
        shutil.copy(args.ffmpeg, os.path.join(up, 'ffmpeg'))
        shutil.copy(args.gate_stream, os.path.join(up, 'gate.h264'))
        shutil.copy(os.path.join(HERE, 'h264_parity.py'), os.path.join(up, 'h264_parity.py'))
        if args.one_thread_class:
            os.makedirs(os.path.join(up, 'refs'), exist_ok=True)
            names = {}
            for path, frames in args.refs:
                if os.path.basename(path) in names or os.path.basename(path) in ('label', 'time'):
                    ap.error('the two --ref-stream files need different names (not label or time)')
                names[os.path.basename(path)] = frames
                shutil.copy(path, os.path.join(up, 'refs', os.path.basename(path)))
            with open(os.path.join(up, 'refs', 'refs.json'), 'w') as f:
                json.dump({'core': args.pin, 'streams': names}, f)
            with open(os.path.join(up, 'gate1t.py'), 'w') as f:
                f.write(GATE1T)
        with open(os.path.join(up, 'job.sh'), 'w') as f:
            f.write(job_script(args, calls))
        size = sum(os.path.getsize(os.path.join(r, n)) for r, _, ns in os.walk(up) for n in ns)
        cmd = [sys.executable, os.path.join(HERE, 'gcp_vm_job.py'), '--upload', up, '--job', 'job.sh',
               '--download', args.out, '--zone', args.zone, '--machine', args.machine, '--provisioning', 'standard',
               '--threads-per-core', '1',
               '--usd-per-hour', str(args.usd_per_hour), '--max-hours', str(args.max_hours), '--cap-usd', str(args.cap_usd)]
        print(f'upload {size / 1e6:.0f} MB, {sum(len(f) for _, f in calls)} streams in {len(calls)} calls; '
              f'spend bound {args.max_hours * args.usd_per_hour:.2f} USD of {args.cap_usd:.2f}', flush=True)
        if args.dry_run:
            kept = os.path.abspath(args.out)
            os.makedirs(kept, exist_ok=True)
            shutil.copy(os.path.join(up, 'job.sh'), os.path.join(kept, 'job.sh'))
            print('dry run:', ' '.join(shlex.quote(c) for c in cmd))
            return 0
        os.makedirs(args.out, exist_ok=True)
        shutil.copy(os.path.join(up, 'job.sh'), os.path.join(args.out, 'job.sh'))
        rc = subprocess.run(cmd).returncode
        if rc != 0:
            print(f'gcp_vm_job.py exited {rc}: no verdict', flush=True)
        if rc != 0:
            return 4
        v = verdict(args.out, calls, args.threads, args.one_thread_class)
        print(json.dumps(v), flush=True)
        return 3 if v['void'] else 0
    finally:
        if not args.upload_dir:
            shutil.rmtree(up, ignore_errors=True)


if __name__ == '__main__':
    sys.exit(main())
