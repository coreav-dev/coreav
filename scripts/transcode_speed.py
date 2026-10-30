#!/usr/bin/env python3
"""Bar B5 of `coreav transcode` (docs/design/transcode.md, "Bars"): speed against the FFmpeg baseline, on one host.

    transcode_speed.py select --work DIR --selection SEL.json [--classes ...]
    transcode_speed.py run --selection SEL.json --files DIR --coreav PATH --arm NAME=ARGS [--arm ...] \\
        --gate FILE --out DIR [--tmp /dev/shm/transcode-speed] [--ffmpeg PATH] [--ffprobe PATH]
    transcode_speed.py summarize --out DIR

`select` (needs the corpus: R2 credentials as for scripts/transcode_check.py): corpus v1 `real` files with
`holdout: true`, tagged c:isobmff, c:matroska or c:webm, classed by the codec of the default video stream, whose
video is 1 to 120 s long, taken per class in the order of sha256("coreav-transcode-speed-v1:" + sha256): at most
25 files a class, and a file that would take the class past 4.8e10 pixel-frames (from `ffprobe`, before any run)
is skipped and counted "over budget". Every other eligible file of the class goes to the class's reserve, in
ascending order of pixel-frames, for the top-up. Each file is downloaded to WORK/files, probed (WORK/probe caches
the probe) and deleted again; `run` fetches the files it times into --files when they are not there and the R2
credentials are set (scripts/transcode_speed_gcp.py uploads them instead).

`run`: first each arm transcodes the preflight file (the gate file unless `--preflight` names another), and the
run stops when one does not: a misconfigured arm (no encoder program) must not turn every file into a failure.
Then, for each class, its selected files, then, while the class has fewer than 5 timed files, its reserve in order
until 5 are timed or none is left. A file is timed when its baseline ran; an arm that fails or times out on it
keeps it timed, with the ratio +inf for that arm only. Per file: the source read twice (page cache), outputs on
TMP (a tmpfs; another file system only with `--allow-non-tmpfs`, never for a claim), 3 rounds of the three commands in rotated order (round r starts at
command r: the first arm, the second arm, the baseline), each command a process timed from outside, and then the
information command once:

    coreav transcode IN OUT.mp4 --preset medium --crf 23 ARGS --force              (each arm)
    ffmpeg -nostdin -noautorotate -flags +unaligned -i IN -map 0:<video> [-map 0:<audio>]... -c:v libx264 -preset medium \\
           -crf 23 -c:a copy -fps_mode passthrough -y OUT.mp4                       (baseline)
    ffmpeg -i IN -c:v libx264 -crf 23 -preset medium -c:a copy -y OUT.mp4           (information)

The baseline's `-map` options are the streams the first arm's summary lists that wrote one (round 0 runs every
arm before the baseline); when no arm did, the default video stream and the audio streams `coreav remux` keeps.
A command's time is the median of its rounds, a file's ratio arm / baseline. Counted apart, by reason, are only
a refusal of the input (round 0, an input-side kind that does not name the arm's setup) and a baseline that fails
to run. The gate is video only: the baseline command on the gate file's default video stream with `-an`, timed 3
times after one untimed warm-up, before the first file and again after the last (a resumed run times another
`after`; the last one counts); the run is void when the two medians differ by more than 5%. The exit status is 3
for a void run, 1 when an arm misses the bar in a class or overall, a class has no timed file or a top-up could
not be honoured, and 0 otherwise. FFmpeg's failure text is kept only from the same command run again at
`-loglevel error`.

`summarize`: per arm, per class and over all files, the median of the per-file ratios with a bootstrap 95%
interval (10,000 resamples, seed 518), against the bar (median at most 1.05); a "faster" claim needs the
interval's upper bound below 1.00. The backends that ran, per class. Results name files by SHA-256 only. Uses
only the Python standard library.
"""
import argparse
import collections
import json
import math
import os
import pathlib
import platform
import shutil
import statistics
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import transcode_check as tc  # noqa: E402

SALT = 'coreav-transcode-speed-v1:'
PRESET = 'medium'
CRF = 23
MAX_FILES = 25
BUDGET = 4.8e10
MIN_SECONDS, MAX_SECONDS = 1, 120
MIN_TIMED = 5
ROUNDS = 3
BAR = 1.05
GATE_RUNS = 3
GATE_DRIFT = 0.05
TIMEOUT = 3600
# docs/design/transcode.md, "Size of the runs": `medium` encodes about 1e8 pixels a second and each file is
# encoded ten times, so a pixel-frame costs about 1e-7 s of the run.
SECONDS_PER_PIXEL_FRAME = 1e-7


def select(args):
    tc.ENV = tc.child_env(args.ffmpeg)
    args.work = str(pathlib.Path(args.work))
    probe_of = tc.make_prober(args, keep=False)
    by_class = tc.manifest_candidates(args.classes, holdout=True)
    sel = {'params': {'salt': SALT, 'max_files': MAX_FILES, 'budget': BUDGET, 'min_seconds': MIN_SECONDS,
                      'max_seconds': MAX_SECONDS, 'min_timed': MIN_TIMED, 'holdout': True},
           'classes': {}}

    def row(e, p):
        return {'sha256': e['sha256'], 'ext': e['ext'], 'size': e['size'],
                'pixel_frames': p['pixel_frames'], 'seconds': p['seconds'], 'width': p['width'],
                'height': p['height'], 'pix_fmt': p['pix_fmt']}
    for cls in args.classes:
        chosen, apart, rest = tc.select_class(by_class.get(cls, []), probe_of, cls, max_files=MAX_FILES,
                                              budget=BUDGET, min_seconds=MIN_SECONDS, max_seconds=MAX_SECONDS,
                                              salt=SALT, probe_rest=True)
        reserve = sorted(rest, key=lambda ep: (ep[1]['pixel_frames'], ep[0]['sha256']))
        sel['classes'][cls] = {'files': [row(e, p) for e, p in chosen], 'reserve': [row(e, p) for e, p in reserve],
                               'pixel_frames': sum(p['pixel_frames'] for _, p in chosen), 'apart': dict(apart)}
        tc.log(f'{cls}: {len(chosen)} files, {len(reserve)} in reserve, '
               f'{sel["classes"][cls]["pixel_frames"]:.3g} pixel-frames, apart {dict(apart)}')
    tc._write(pathlib.Path(args.selection), sel)
    return 0


def timed(cmd, timeout=TIMEOUT):
    t = time.perf_counter()
    try:
        r = subprocess.run([str(c) for c in cmd], capture_output=True, timeout=timeout, check=False, env=tc.ENV)
    except subprocess.TimeoutExpired:
        return None, subprocess.CompletedProcess(cmd, -999, b'', b'timeout')
    return time.perf_counter() - t, r


def prime(path):
    for _ in range(2):
        with open(path, 'rb') as f:
            while f.read(1 << 24):
                pass


def rotation(names, r):
    k = r % len(names)
    return names[k:] + names[:k]


def baseline_cmd(ffmpeg, src, out, video, audio):
    cmd = [ffmpeg, '-nostdin', '-noautorotate', '-flags', '+unaligned', '-i', src, '-map', f'0:{video}']
    for a in audio:
        cmd += ['-map', f'0:{a}']
    return cmd + ['-c:v', 'libx264', '-preset', PRESET, '-crf', str(CRF), '-c:a', 'copy', '-fps_mode', 'passthrough',
                  '-y', out]


def time_file(args, f, path, tmp):
    """One file's rounds. A refusal of the input (round 0) sets the file apart, and so does a baseline that fails
    to run; an arm that fails or times out keeps the file judged, with the ratio +inf for that arm only."""
    res = {'sha256': f['sha256'], 'ext': f['ext'], 'pixel_frames': f['pixel_frames'], 'seconds': f['seconds']}
    prime(path)
    arms = [n for n, _ in args.arms]
    cmds = {n: [args.coreav, 'transcode', path, tmp / f'{n}.mp4', '--preset', PRESET, '--crf', str(CRF), *a,
                '--force'] for n, a in args.arms}
    times = collections.defaultdict(list)
    failed = {}
    for r in range(ROUNDS):
        # Round 0 runs every arm before the baseline, whose `-map` options come from the first arm's summary.
        for name in rotation(arms + ['baseline'], r):
            if name in failed:
                continue
            if name == 'baseline' and 'baseline' not in cmds:
                # No arm wrote an output: the default video stream and the audio streams `coreav remux` keeps.
                src = tc.Source(args, path)
                src.remux(tmp)
                v = tc.default_video(src.streams)
                if v is None:
                    return dict(res, status='baseline_failed', reason='no video stream', failed=failed)
                res['maps'] = {'video': v['index'], 'audio': src.remux_audio or [], 'from': 'probe'}
                cmds['baseline'] = baseline_cmd(args.ffmpeg, path, tmp / 'baseline.mp4', v['index'],
                                                src.remux_audio or [])
            out = tmp / f'{name}.mp4'
            out.unlink(missing_ok=True)
            dt, p = timed(cmds[name])
            out.unlink(missing_ok=True)
            if p.returncode != 0 or dt is None:
                if name == 'baseline':
                    return dict(res, status='baseline_failed', reason=f'exit {p.returncode}',
                                error=tc.ffmpeg_error_text(cmds[name]), failed=failed)
                err = tc.error_of(p)
                refused = (p.returncode == 1 and err.get('schema') == 'coreav.error/1'
                           and err.get('kind') in tc.REFUSED_KINDS and not tc.misconfigured(err))
                if refused and r == 0:
                    return dict(res, status='refused', reason=err.get('kind'), error=err, arm=name)
                failed[name] = {'exit': p.returncode, 'timeout': dt is None, 'round': r, 'error': err}
                continue
            if name in arms and r == 0:
                try:
                    s = json.loads(p.stdout)
                except ValueError:
                    s = {}
                video = s.get('video') or {}
                res.setdefault('backend', {})[name] = video.get('backend')
                res.setdefault('encoder', {})[name] = video.get('encoder')
                if 'baseline' not in cmds and 'stream_index' in video:
                    audio = [a['stream_index'] for a in s.get('audio', [])]
                    res['maps'] = {'video': video['stream_index'], 'audio': audio, 'from': name}
                    cmds['baseline'] = baseline_cmd(args.ffmpeg, path, tmp / 'baseline.mp4', video['stream_index'],
                                                    audio)
            times[name].append(dt)
    info = [args.ffmpeg, '-nostdin', '-i', path, '-c:v', 'libx264', '-crf', str(CRF), '-preset', PRESET,
            '-c:a', 'copy', '-y', tmp / 'info.mp4']
    dt, p = timed(info)
    (tmp / 'info.mp4').unlink(missing_ok=True)
    res['information_seconds'] = dt if p.returncode == 0 else None
    res['times'] = dict(times)
    med = {n: statistics.median(v) for n, v in times.items()}
    res['median_seconds'] = med
    if failed:
        res['failed'] = failed
    res['ratios'] = {n: (math.inf if n in failed else med[n] / med['baseline']) for n in arms}
    res['status'] = 'timed'
    return res


def gate(args, tmp, which):
    streams = tc.streams_of(args.ffprobe, args.gate) or []
    v = tc.default_video(streams)
    cmd = [args.ffmpeg, '-nostdin', '-noautorotate', '-flags', '+unaligned', '-i', args.gate, '-map', f'0:{v["index"]}', '-c:v', 'libx264',
           '-preset', PRESET, '-crf', str(CRF), '-an', '-fps_mode', 'passthrough', '-y', tmp / 'gate.mp4']
    prime(args.gate)
    timed(cmd)  # one warm-up, untimed
    ts = []
    for _ in range(GATE_RUNS):
        dt, p = timed(cmd)
        if p.returncode != 0 or dt is None:
            raise SystemExit(f'the gate command failed: {tc.ffmpeg_error_text(cmd)}')
        ts.append(dt)
    (tmp / 'gate.mp4').unlink(missing_ok=True)
    return {'which': which, 'times': ts, 'median': statistics.median(ts)}


def host(args):
    def out(cmd):
        try:
            return subprocess.run(cmd, capture_output=True, text=True, timeout=60).stdout
        except (OSError, subprocess.SubprocessError):
            return None
    mounts = out(['cat', '/proc/mounts']) or ''
    tmp = str(pathlib.Path(args.tmp).resolve())
    fs = max((line.split() for line in mounts.splitlines() if len(line.split()) > 2 and
              (tmp + '/').startswith(line.split()[1].rstrip('/') + '/')), key=lambda f: len(f[1]), default=None)
    return {'platform': platform.platform(), 'python': platform.python_version(), 'cpus': os.cpu_count(),
            'lscpu': out(['lscpu']), 'meminfo': (out(['head', '-3', '/proc/meminfo']) or '').splitlines(),
            'tmp_filesystem': fs[2] if fs else None, 'tools': tc.tool_hashes(args),
            'ffmpeg_version': (out([args.ffmpeg, '-version']) or '').splitlines()[:1]}


def run(args):
    tc.ENV = tc.child_env(args.ffmpeg)
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    tmp = pathlib.Path(args.tmp)
    tmp.mkdir(parents=True, exist_ok=True)
    sel = json.loads(pathlib.Path(args.selection).read_text())
    info = host(args)
    info['arms'] = {n: a for n, a in args.arms}
    tc._write(out / 'host.json', info)
    if info['tmp_filesystem'] != 'tmpfs':
        if not args.allow_non_tmpfs:
            raise SystemExit(f'{args.tmp} is on {info["tmp_filesystem"]}, not tmpfs (--allow-non-tmpfs to time anyway)')
        tc.log(f'warning: {args.tmp} is on {info["tmp_filesystem"]}, not tmpfs')
    preflight(args, tmp)
    results_path = out / 'files.jsonl'
    done = {}
    if results_path.exists():
        for line in results_path.read_text().splitlines():
            if line.strip():
                r = json.loads(line)
                done[r['sha256']] = r
    gates_path = out / 'gate.json'
    gates = json.loads(gates_path.read_text()) if gates_path.exists() else []
    if not gates:
        gates.append(gate(args, tmp, 'before'))
        tc._write(gates_path, gates)
    with results_path.open('a') as fh:
        for cls in args.classes:
            c = sel['classes'].get(cls)
            if not c:
                continue
            timed_n = 0
            queue = [(f, False) for f in c['files']] + [(f, True) for f in c['reserve']]
            for f, topup in queue:
                if topup and timed_n >= MIN_TIMED:
                    break
                if f['sha256'] in done:
                    r = done[f['sha256']]
                else:
                    path = pathlib.Path(args.files) / f'{f["sha256"]}.{f["ext"]}'
                    if not path.exists() and os.environ.get('R2_ACCESS_KEY_ID'):
                        path.parent.mkdir(parents=True, exist_ok=True)
                        tc.download(f, path)
                    if not path.exists():
                        r = {'sha256': f['sha256'], 'status': 'missing'}
                    else:
                        r = time_file(args, f, path, tmp)
                    r.update({'class': cls, 'topup': topup})
                    fh.write(json.dumps(r) + '\n')
                    fh.flush()
                    tc.log(f'{cls} {f["sha256"][:12]} {r["status"]} {r.get("reason", "")} '
                           + ' '.join(f'{n}={v:.4f}' for n, v in r.get('ratios', {}).items()))
                timed_n += r['status'] == 'timed'
    gates.append(gate(args, tmp, 'after'))
    tc._write(gates_path, gates)
    rep = summarize(out, sel, args.classes)
    print(json.dumps({k: rep[k] for k in ('void', 'reasons')}, indent=1))
    print(table(rep))
    return status(rep)


def status(rep):
    """3 for a void run; 1 when an arm misses the bar in a class or overall, a class or an arm has no timed
    file, or a class's top-up could not be honoured; else 0."""
    if rep['void']:
        return 3
    problems = []
    for arm, per in rep['arms'].items():
        for cls, v in per.items():
            if v['timed'] == 0:
                problems.append(f'{arm} {cls}: no timed file')
            elif v['met'] is False:
                problems.append(f'{arm} {cls}: median {v["median_ratio"]:.4f} above {BAR}')
            if v.get('topup_not_honoured'):
                problems.append(f'{arm} {cls}: the top-up could not be honoured')
    if not rep['arms']:
        problems.append('no arm')
    for p in problems:
        print('FAIL:', p)
    return 1 if problems else 0


def preflight(args, tmp):
    """Each arm transcodes the preflight file (the gate file unless --preflight names another) before any
    timing: a misconfigured arm stops the run instead of failing on every file."""
    path = args.preflight or args.gate
    for name, arm_args in args.arms:
        out = tmp / f'preflight-{name}.mp4'
        dt, p = timed([args.coreav, 'transcode', path, out, '--preset', 'ultrafast', '--crf', str(CRF), *arm_args,
                       '--force'])
        out.unlink(missing_ok=True)
        if p.returncode != 0 or dt is None:
            raise SystemExit(f'arm {name} does not transcode {os.path.basename(path)}: exit {p.returncode} '
                             f'{json.dumps(tc.error_of(p))[:300]}')


def summarize(out, sel=None, classes=None):
    out = pathlib.Path(out)
    rows = [json.loads(line) for line in (out / 'files.jsonl').read_text().splitlines() if line.strip()]
    # A resumed run appends: the last record of a file is its record.
    rows = list({r['sha256']: r for r in rows}.values())
    gates = json.loads((out / 'gate.json').read_text()) if (out / 'gate.json').exists() else []
    info = json.loads((out / 'host.json').read_text()) if (out / 'host.json').exists() else {}
    arms = list(info.get('arms', {})) or sorted({a for r in rows for a in r.get('ratios', {})})
    rep = {'void': False, 'reasons': [], 'gate': gates, 'arms': {}}
    # The first gate before the first file, and the last after the last (a resumed run times another).
    before = next((g for g in gates if g['which'] == 'before'), None)
    after = next((g for g in reversed(gates) if g['which'] == 'after'), None)
    if not before or not after:
        rep['void'], rep['reasons'] = True, ['no gate time before and after']
    else:
        drift = after['median'] / before['median'] - 1
        rep['gate_drift'] = drift
        if abs(drift) > GATE_DRIFT:
            rep['void'] = True
            rep['reasons'].append(f'the gate moved {drift:+.1%}')
    by_class = collections.defaultdict(list)
    for r in rows:
        by_class[r.get('class')].append(r)
    # Every class the run was asked for, so that one without a timed file shows (and fails the run).
    wanted = [c for c in (classes or tc.CLASSES) if (sel is None and c in by_class)
              or (sel is not None and c in sel['classes'])]
    for arm in arms:
        per = rep['arms'][arm] = {}
        groups = {c: by_class.get(c, []) for c in wanted}
        groups['all'] = rows
        for cls, rs in groups.items():
            ratios = [r['ratios'][arm] for r in rs if r.get('status') == 'timed' and arm in r.get('ratios', {})]
            info_ratios = [r['median_seconds'][arm] / r['information_seconds'] for r in rs
                           if r.get('status') == 'timed' and r.get('information_seconds')
                           and arm in r.get('median_seconds', {}) and arm not in r.get('failed', {})]
            med, lo, hi = tc.bootstrap(ratios)
            apart = collections.Counter(f'{r["status"]}: {r.get("reason")}' for r in rs if r.get('status') != 'timed')
            per[cls] = {'timed': len(ratios), 'topup': sum(1 for r in rs if r.get('topup') and r.get('status') == 'timed'),
                        'median_ratio': med, 'ci95': [lo, hi], 'met': None if med is None else med <= BAR,
                        'faster_claim': hi is not None and hi < 1.0,
                        'share_faster': (sum(1 for x in ratios if x < 1) / len(ratios)) if ratios else None,
                        'failed': sum(1 for x in ratios if x == math.inf),
                        'median_ratio_to_information_command': statistics.median(info_ratios) if info_ratios else None,
                        'apart': dict(apart),
                        'backends': dict(collections.Counter((r.get('backend') or {}).get(arm) for r in rs
                                                             if r.get('status') == 'timed'))}
            if sel and cls in sel['classes']:
                c = sel['classes'][cls]
                per[cls]['selection_apart'] = c['apart']
                if per[cls]['timed'] < MIN_TIMED and len(rs) >= len(c['files']) + len(c['reserve']):
                    left = c.get('reserve_not_uploaded', 0)
                    per[cls]['reserve_used_up'] = True
                    if left:
                        # The top-up rule ("until 5 are timed or none is left") is not met: eligible files
                        # remain that this host did not have.
                        per[cls]['topup_not_honoured'] = {'eligible_files_not_here': left}
    tc._write(out / 'summary.json', rep)
    return rep


def table(rep):
    lines = []
    for arm, per in rep['arms'].items():
        lines.append(f'== {arm} (bar: median <= {BAR})')
        for cls in list(tc.CLASSES) + ['all']:
            v = per.get(cls)
            if not v or v['median_ratio'] is None:
                if v:
                    lines.append(f'{cls:7} no timed file; apart {v["apart"]}')
            if v and v.get('topup_not_honoured'):
                lines.append(f'{cls:7} TOP-UP NOT HONOURED: {v["topup_not_honoured"]["eligible_files_not_here"]} '
                             'eligible files were not on this host')
            if not v or v['median_ratio'] is None:
                continue
            if v.get('failed'):
                lines.append(f'{cls:7} {v["failed"]} timed file(s) where {arm} failed (ratio +inf)')
            lines.append(f'{cls:7} n={v["timed"]:3} median {v["median_ratio"]:.4f} CI [{v["ci95"][0]:.4f}, '
                         f'{v["ci95"][1]:.4f}] {"met" if v["met"] else "MISSED"} faster on {v["share_faster"]:.0%}'
                         f' backends {v["backends"]} apart {v["apart"]}')
    return '\n'.join(lines)


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument('command', choices=('select', 'run', 'summarize'))
    p.add_argument('--selection')
    p.add_argument('--work')
    p.add_argument('--files')
    p.add_argument('--coreav')
    p.add_argument('--arm', action='append', metavar='NAME=ARGS')
    p.add_argument('--ffmpeg', default=shutil.which('ffmpeg'))
    p.add_argument('--ffprobe', default=shutil.which('ffprobe'))
    p.add_argument('--gate')
    p.add_argument('--out')
    p.add_argument('--tmp', default='/dev/shm/transcode-speed')
    p.add_argument('--classes', default=','.join(tc.CLASSES))
    p.add_argument('--preflight', help='the file each arm must transcode before the run (default: the gate file)')
    p.add_argument('--allow-non-tmpfs', action='store_true', help='time with outputs on a disk (not for a claim)')
    args = p.parse_args()
    args.classes = [c for c in args.classes.split(',') if c]
    args.decode_oracle = None
    args.compat = False
    if not set(args.classes) <= set(tc.CLASSES):
        p.error(f'--classes: {tc.CLASSES}')
    if args.command == 'select':
        if not args.work or not args.selection:
            p.error('select needs --work and --selection')
        return select(args)
    if args.command == 'summarize':
        if not args.out:
            p.error('summarize needs --out')
        sel = json.loads(pathlib.Path(args.selection).read_text()) if args.selection else None
        rep = summarize(args.out, sel, args.classes)
        print(table(rep))
        return status(rep)
    args.arms = tc.parse_arms(args.arm, p)
    for need in ('selection', 'files', 'coreav', 'gate', 'out'):
        if not getattr(args, need):
            p.error(f'run needs --{need}')
    if len(args.arms) < 1:
        p.error('run needs at least one --arm')
    return run(args)


if __name__ == '__main__':
    sys.exit(main())
