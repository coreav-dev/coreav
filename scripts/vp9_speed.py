#!/usr/bin/env python3
"""Speed of `coreav thumbnail` on VP9 through libvpx against FFmpeg's own VP9 decoder, and bar 1 of the decode
boundary (docs/design/decode.md) for the libvpx backend, on corpus v1.

    vp9_speed.py select --selection SEL.json
    vp9_speed.py run --selection SEL.json --files DIR --coreav libvpx=PATH --coreav fallback=PATH --gate FILE \\
        --out DIR [--decode-oracle PATH] [--tmp /dev/shm/vp9-speed] [--ffmpeg PATH] [--ffprobe PATH]
    vp9_speed.py oracle --selection SEL.json --files DIR --decode-oracle PATH --out DIR [--backend libvpx]
    vp9_speed.py summarize --out DIR

`select` (the manifest at corpus/v1/manifest.json under the checkout root; no download): corpus v1 `real` files whose
first video stream's codec is VP9, in the order of sha256, each with its `holdout` flag. Encrypted files are counted
apart. Every other file that cannot be used is set apart by the run, with its reason.

`run`: the method of the thumbnail speed bars S1 to S5 (docs/design/thumbnails.md, "Bars"), once per `--coreav` arm.
Per file, the source is read twice (page cache), outputs go to TMP (a tmpfs; another file system only with
`--allow-non-tmpfs`, never for a claim), one run of the first arm learns the frame it chooses, every command runs once
as a warm-up, and then 5 rounds run the commands in rotated order (round r starts at command r), each command a
process timed from outside. A command's time is the median of its rounds, a bar's ratio an arm's command / FFmpeg's:

    S1  coreav thumbnail F --at T --width 320 -o o.jpg --force   against  ffmpeg -ss T -i F -frames:v 1 -vf scale=W:H f.jpg
    S2  the same                                                 against  ffmpeg -ss K -i F ... f.jpg
    S3  the same with --exact                                    against  S1's ffmpeg command
    S4  coreav thumbnail ... -o o.png                            against  S2's ffmpeg command writing f.png
    S5  coreav thumbnail ... -o o.webp                           against  S2's ffmpeg command writing f.webp
                                                                 with -c:v libwebp -lossless 1

`--coreav NAME=PATH` names an arm and its program. `libvpx` is a build with `libvpx` and `ffmpeg-fallback`, whose
`coreav.thumbnail/1` summary must name `libvpx` as `decoder.backend`, and `fallback` a build with `ffmpeg-fallback`
only, which decodes VP9 in the sandboxed worker through FFmpeg. Both reach the same `ffmpeg` as the baseline
(`COREAV_FFMPEG`). A file where an arm names another backend keeps its times and leaves that arm's ratios out, counted
apart (`--expect NAME=BACKEND` changes what an arm must name, for a check of the plumbing with one build, never for a
claim). A command that fails leaves its bar out for that file only. Untimed, the PNGs of the frames of S2 and S3 are
compared with FFmpeg's (PSNR over 8-bit RGB), so that a bar timed on different frames shows.

Beside the bars, with no bar, each file's whole-stream decode is timed (a warm-up and `--decode-rounds` rounds, 3 by
default, rotated):

    ffmpeg -nostdin -threads 0 -c:v vp9 -i F -map 0:v:0 -f null -
    ffmpeg -nostdin -threads 0 -c:v libvpx-vp9 -i F -map 0:v:0 -f null -       (when the binary has that decoder)
    decode-oracle F --backend libvpx --packets <the stream's packet count>     (with --decode-oracle)

The gate is the S1 baseline command on the gate file (a VP9 file; `--gate`), 3 runs after one untimed warm-up, before
the first file and after the last (a resumed run times another `after`; the last counts). The run is void when the
two medians differ by more than 5%. Each arm must first make a thumbnail of the gate file, with the backend it is
named for: a misconfigured arm stops the run. Results append to OUT/files.jsonl and a rerun skips the files in it. They
name files by SHA-256 only, with their extension, size and set.

The exit status is 3 for a void run, 1 when an arm of `--judge` (default `libvpx`) has a median ratio above `--bar`
(default 1.00) in a bar over the files of `--judge-set` (default `all`: both partitions) or no timed file, and 0
otherwise.

`oracle`: bar 1 for the libvpx backend. The first 120 packets of each file's default video stream go through
`decode-oracle --backend libvpx`, and its frames' MD5s are compared with FFmpeg's `framemd5` of the same stream
decoded with `-c:v vp9` (scripts/thumbnail_oracle.py, `frames`, with its options). Rows go to OUT/oracle.jsonl, and
a rerun skips the files in it. The exit status is 1 when fewer than 99% of the streams or 99.9% of the frames agree.

`summarize`: per bar and arm, the median of the per-file ratios with a bootstrap 95% interval (10,000 resamples), the
share of files below 1.00, over all files, the development files and the held-out files, and split by FFmpeg's own
time for the bar's command (under 50 ms, 50 to 300 ms, over 300 ms); libvpx / fallback beside them; the decode-only
pair; the oracle counts. Markdown to standard output and OUT/summary.json. Uses only the Python standard library.
"""
import argparse
import collections
import concurrent.futures
import json
import os
import pathlib
import platform
import re
import shutil
import statistics
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import thumbnail_oracle as to  # noqa: E402
import transcode_check as tc  # noqa: E402

ROUNDS = 5
TIMEOUT = 300
DECODE_TIMEOUT = 3600
GATE_RUNS = 3
GATE_DRIFT = 0.05
BAR = 1.00
STREAMS_SHARE = 0.99
FRAMES_SHARE = 0.999
OURS = {
    'ours_key_jpg': ('jpg', ()),
    'ours_exact_jpg': ('jpg', ('--exact',)),
    'ours_key_png': ('png', ()),
    'ours_key_webp': ('webp', ()),
}
BARS = {
    'S1': ('ours_key_jpg', 'ff_t_jpg'),
    'S2': ('ours_key_jpg', 'ff_k_jpg'),
    'S3': ('ours_exact_jpg', 'ff_t_jpg'),
    'S4': ('ours_key_png', 'ff_k_png'),
    'S5': ('ours_key_webp', 'ff_k_webp'),
}
EXPECTED_BACKEND = {'libvpx': 'libvpx', 'fallback': 'ffmpeg'}
PAIR = ('libvpx', 'fallback')
DECODE_RATIOS = (('ffmpeg_libvpx_vp9', 'ffmpeg_vp9'), ('oracle', 'ffmpeg_vp9'), ('oracle', 'ffmpeg_libvpx_vp9'))
TIME_BUCKETS = (('under 50 ms', 0, 0.05), ('50 to 300 ms', 0.05, 0.3), ('over 300 ms', 0.3, None))
GROUPS = ('all', 'dev', 'held-out')


def select(args):
    files = json.loads(tc.MANIFEST.read_text())['files']
    chosen, apart = [], collections.Counter()
    for e in sorted(files, key=lambda e: e['sha256']):
        codec = next((t[2:] for t in e.get('tags', ()) if t.startswith('v:')), None)
        if e['set'] != 'real' or codec != 'vp9':
            continue
        if 'encrypted' in e.get('tags', ()):
            apart['encrypted'] += 1
            continue
        chosen.append({'sha256': e['sha256'], 'ext': e['ext'], 'size': e['size'], 'holdout': bool(e.get('holdout'))})
    held = sum(1 for f in chosen if f['holdout'])
    tc._write(pathlib.Path(args.selection),
              {'params': {'class': 'vp9', 'set': 'real', 'order': 'sha256'}, 'files': chosen, 'apart': dict(apart),
               'held_out': held, 'dev': len(chosen) - held})
    tc.log(f'vp9: {len(chosen)} files, {held} held-out, apart {dict(apart)}')
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


def tail(p):
    return (p.stderr or p.stdout or b'').decode(errors='replace')[-300:]


def summary_of(p):
    try:
        s = json.loads(p.stdout)
    except ValueError:
        return None
    return s if isinstance(s, dict) else None


def ours_cmd(coreav, path, t, out, extra=()):
    return [coreav, 'thumbnail', path, '--at', f'{t:.3f}', '--width', str(to.WIDTH), '-o', out, '--force', *extra]


def ff_cmd(args, path, ss, w, h, out, extra=()):
    return [args.ffmpeg, '-v', 'error', '-y', '-ss', ss, '-i', path, '-frames:v', '1', '-vf', f'scale={w}:{h}', *extra,
            out]


def measure(commands, rounds, timeout):
    """(times, failed, warm): one warm-up of every command, then `rounds` rounds in rotated order. A command that
    fails or times out is out of the rest, with the tail of its output."""
    failed, warm = {}, {}
    for name, cmd in commands.items():
        dt, p = timed(cmd, timeout)
        warm[name] = p
        if dt is None or p.returncode != 0:
            failed[name] = tail(p)
    names = [n for n in commands if n not in failed]
    times = {n: [] for n in names}
    for r in range(rounds):
        k = r % len(names) if names else 0
        for name in names[k:] + names[:k]:
            if name in failed:
                continue
            dt, p = timed(commands[name], timeout)
            if dt is None or p.returncode != 0:
                failed[name] = tail(p)
                continue
            times[name].append(dt)
    return times, failed, warm


def medians(times, failed):
    return {n: statistics.median(v) for n, v in times.items() if v and n not in failed}


def same_frame(args, a, b):
    try:
        return to.psnr(to.rgb(a, args.ffmpeg), to.rgb(b, args.ffmpeg))
    except Exception:  # noqa: BLE001 - a missing or odd image is reported as None
        return None


def time_file(args, f, path, tmp):
    res = {'sha256': f['sha256'], 'class': 'vp9', 'ext': f['ext'], 'size': f['size'], 'holdout': f['holdout']}
    t = to.at_time(path, args.ffprobe)
    if t is None:
        return dict(res, status='no_duration')
    res['t'] = t
    prime(path)
    arms = [n for n, _ in args.arms]
    first, learned, errors = None, None, {}
    for name, coreav in args.arms:
        _, r = timed(ours_cmd(coreav, path, t, tmp / 'o.jpg'))
        s = summary_of(r) if r.returncode == 0 else None
        if s is None:
            errors[name] = to.error_of(r)
            continue
        first, learned = name, s
        break
    if learned is None:
        return dict(res, status='refused', error=errors)
    w, h = learned['output']['width'], learned['output']['height']
    k = to.ffmpeg_time(args.ffprobe, path, learned)
    res.update({'width': w, 'height': h, 'k': k, 'learned_from': first})
    if k is None:
        res['no_keyframe_time'] = True
    bars = [b for b in args.bars if k is not None or b in ('S1', 'S3')]
    wanted = {c for b in bars for c in BARS[b]}
    commands = {}
    for key, (fmt, extra) in OURS.items():
        if key not in wanted:
            continue
        for name, coreav in args.arms:
            commands[f'{name}:{key}'] = ours_cmd(coreav, path, t, tmp / f'o.{fmt}', extra)
    ff = {'ff_t_jpg': (f'{t:.3f}', 'f.jpg', ())}
    if k is not None:
        ff.update({'ff_k_jpg': (k, 'f.jpg', ()), 'ff_k_png': (k, 'f.png', ()),
                   'ff_k_webp': (k, 'f.webp', ('-c:v', 'libwebp', '-lossless', '1'))})
    for key, (ss, name, extra) in ff.items():
        if key in wanted:
            commands[key] = ff_cmd(args, path, ss, w, h, tmp / name, extra)
    times, failed, warm = measure(commands, ROUNDS, TIMEOUT)
    info = {}
    for name in arms:
        s = next((summary_of(p) for key, p in warm.items() if key.startswith(f'{name}:') and p.returncode == 0), None)
        if s is not None:
            info[name] = {'backend': s['decoder']['backend'], 'decoder': s['decoder']['name'],
                          'frame': s['time']['frame'], 'keyframe': s.get('keyframe')}
    res['arms'] = info
    res['frame_differs_between_arms'] = len({json.dumps(i['frame'], sort_keys=True) for i in info.values()}) > 1
    excluded = {n: f'backend {i["backend"]}' for n, i in info.items()
                if args.expected.get(n) not in (None, i['backend'])}
    res['psnr'] = {}
    pngs = {'S3': (f'{t:.3f}', ('--exact',))}
    if k is not None:
        pngs['S2'] = (k, ())
    for bar, (ss, extra) in pngs.items():
        if bar not in bars or timed(ff_cmd(args, path, ss, w, h, tmp / 'same_ff.png'))[1].returncode != 0:
            continue
        for name, coreav in args.arms:
            _, r = timed(ours_cmd(coreav, path, t, tmp / 'same_o.png', extra))
            res['psnr'].setdefault(bar, {})[name] = (same_frame(args, tmp / 'same_o.png', tmp / 'same_ff.png')
                                                     if r.returncode == 0 else None)
    med = medians(times, failed)
    res['times'] = times
    res['median_seconds'] = med
    res['ratios'] = {}
    for name in arms:
        if name in excluded:
            continue
        for bar in bars:
            a, b = f'{name}:{BARS[bar][0]}', BARS[bar][1]
            if a in med and b in med:
                res['ratios'].setdefault(name, {})[bar] = med[a] / med[b]
    if excluded:
        res['excluded'] = excluded
    if failed:
        res['failed'] = failed
    if args.decode:
        res['decode'] = decode_only(args, path, f)
    res['status'] = 'ok'
    return res


def packet_count(args, path):
    r = to.run([args.ffprobe, '-v', 'error', '-count_packets', '-select_streams', 'v:0', '-show_entries',
                'stream=nb_read_packets', '-of', 'default=noprint_wrappers=1:nokey=1', str(path)])
    try:
        return int(r.stdout.decode().strip())
    except ValueError:
        return None


def decode_only(args, path, f):
    def ffmpeg(decoder):
        return [args.ffmpeg, '-nostdin', '-threads', '0', '-c:v', decoder, '-i', path, '-map', '0:v:0', '-f', 'null',
                '-']
    commands = {'ffmpeg_vp9': ffmpeg('vp9')}
    if args.host_decoders.get('libvpx-vp9'):
        commands['ffmpeg_libvpx_vp9'] = ffmpeg('libvpx-vp9')
    out = {}
    if args.decode_oracle:
        n = packet_count(args, path)
        if n is None:
            out['oracle_skipped'] = 'no packet count'
        elif f['size'] > args.decode_max_bytes:
            out['oracle_skipped'] = 'over the size limit'
        else:
            commands['oracle'] = [args.decode_oracle, path, '--packets', str(n), '--backend', args.decode_backend,
                                  *to.DECODE_ORACLE_ARGS]
            out['packets'] = n
    times, failed, warm = measure(commands, args.decode_rounds, DECODE_TIMEOUT)
    med = medians(times, failed)
    if 'oracle' in warm and 'oracle' not in failed:
        try:
            out['oracle_frames'] = len(json.loads(warm['oracle'].stdout)['frames'])
        except (ValueError, KeyError, TypeError):
            out['oracle_frames'] = None
    out.update({'times': times, 'median_seconds': med,
                'ratios': {f'{a}/{b}': med[a] / med[b] for a, b in DECODE_RATIOS if a in med and b in med}})
    if failed:
        out['failed'] = failed
    return out


def gate(args, tmp, which, spec):
    cmd = ff_cmd(args, args.gate, f'{spec["t"]:.3f}', spec['w'], spec['h'], tmp / 'gate.jpg')
    prime(args.gate)
    timed(cmd)
    ts = []
    for _ in range(GATE_RUNS):
        dt, p = timed(cmd)
        if dt is None or p.returncode != 0:
            raise SystemExit(f'the gate command failed: {tail(p)}')
        ts.append(dt)
    (tmp / 'gate.jpg').unlink(missing_ok=True)
    return {'which': which, 'times': ts, 'median': statistics.median(ts), **spec}


def preflight(args, tmp):
    """Each arm makes a thumbnail of the gate file with the backend it is named for; the gate's S1 command needs T
    and the size of the first arm's."""
    t = to.at_time(pathlib.Path(args.gate), args.ffprobe)
    if t is None:
        raise SystemExit('the gate file has no presented duration')
    spec = None
    for name, coreav in args.arms:
        _, r = timed(ours_cmd(coreav, args.gate, t, tmp / 'preflight.jpg'))
        s = summary_of(r) if r.returncode == 0 else None
        if s is None:
            raise SystemExit(f'arm {name} does not make a thumbnail of the gate file: exit {r.returncode} '
                             f'{json.dumps(to.error_of(r))[:300]}')
        backend = s['decoder']['backend']
        if args.expected.get(name) not in (None, backend):
            raise SystemExit(f'arm {name} decoded the gate file with {backend}, not {args.expected[name]}')
        spec = spec or {'t': t, 'w': s['output']['width'], 'h': s['output']['height']}
    (tmp / 'preflight.jpg').unlink(missing_ok=True)
    return spec


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
    decoders = out([args.ffmpeg, '-hide_banner', '-decoders']) or ''
    programs = {f'coreav-{n}': c for n, c in args.arms}
    programs.update({'ffmpeg': args.ffmpeg, 'ffprobe': args.ffprobe})
    if args.decode_oracle:
        programs['decode-oracle'] = args.decode_oracle
    smt = pathlib.Path('/sys/devices/system/cpu/smt/active')
    return {'platform': platform.platform(), 'python': platform.python_version(), 'cpus': os.cpu_count(),
            'smt_active': smt.read_text().strip() if smt.exists() else None, 'lscpu': out(['lscpu']),
            'meminfo': (out(['head', '-3', '/proc/meminfo']) or '').splitlines(),
            'tmp_filesystem': fs[2] if fs else None,
            'tools': {k: {'path': os.path.basename(p), 'sha256': tc.sha256_file(p) if os.path.isfile(p) else None}
                      for k, p in programs.items()},
            'ffmpeg_version': (out([args.ffmpeg, '-version']) or '').splitlines()[:1],
            'ffmpeg_decoders': {d: bool(re.search(rf'^\s*V\S*\s+{re.escape(d)}\s', decoders, re.M))
                                for d in ('vp9', 'libvpx-vp9')},
            'arms': {n: os.path.basename(c) for n, c in args.arms}, 'expected_backends': args.expected}


def read_rows(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def fetch(args, f):
    path = pathlib.Path(args.files) / f'{f["sha256"]}.{f["ext"]}'
    if not path.exists() and os.environ.get('R2_ACCESS_KEY_ID'):
        path.parent.mkdir(parents=True, exist_ok=True)
        tc.download(f, path)
    return path if path.exists() else None


def queue_of(args):
    sel = json.loads(pathlib.Path(args.selection).read_text())
    files = [f for f in sel['files'] if args.set == 'all' or f['holdout'] == (args.set == 'holdout')]
    files = [f for f in files if not args.sha or any(f['sha256'].startswith(p) for p in args.sha)]
    return files[:args.limit] if args.limit else files


def guarded(f, call):
    try:
        return call()
    except subprocess.TimeoutExpired as e:
        return {'sha256': f['sha256'], 'status': 'timeout', 'error': str(e)[:300]}
    except Exception as e:  # noqa: BLE001 - recorded, so that one file cannot stop the run
        return {'sha256': f['sha256'], 'status': 'worker_error', 'error': repr(e)[:500]}


def run(args):
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    tmp = pathlib.Path(args.tmp)
    tmp.mkdir(parents=True, exist_ok=True)
    info = host(args)
    args.host_decoders = info['ffmpeg_decoders']
    tc._write(out / 'host.json', info)
    if info['tmp_filesystem'] != 'tmpfs':
        if not args.allow_non_tmpfs:
            raise SystemExit(f'{args.tmp} is on {info["tmp_filesystem"]}, not tmpfs (--allow-non-tmpfs to time anyway)')
        tc.log(f'warning: {args.tmp} is on {info["tmp_filesystem"]}, not tmpfs')
    if not info['ffmpeg_decoders']['vp9']:
        raise SystemExit('the ffmpeg binary has no vp9 decoder')
    spec = preflight(args, tmp)
    results = out / 'files.jsonl'
    done = {r['sha256'] for r in read_rows(results) if r.get('status') != 'missing'}
    gates_path = out / 'gate.json'
    gates = json.loads(gates_path.read_text()) if gates_path.exists() else []
    if gates:
        spec = {k: gates[0][k] for k in ('t', 'w', 'h')}
    else:
        gates.append(gate(args, tmp, 'before', spec))
        tc._write(gates_path, gates)
    statuses = collections.Counter()
    queue = queue_of(args)
    with results.open('a') as fh:
        for i, f in enumerate(queue):
            if f['sha256'] in done:
                continue
            path = fetch(args, f)
            r = guarded(f, lambda: time_file(args, f, path, tmp)) if path else {
                'sha256': f['sha256'], 'status': 'missing'}
            fh.write(json.dumps(r) + '\n')
            fh.flush()
            statuses[r['status']] += 1
            tc.log(f'{i + 1}/{len(queue)} {f["sha256"][:12]} {r["status"]} '
                   + ' '.join(f'{a}:{b}={x:.3f}' for a, rs in r.get('ratios', {}).items() for b, x in rs.items()
                              if b == 'S1'))
    gates.append(gate(args, tmp, 'after', spec))
    tc._write(gates_path, gates)
    tc.log(f'{dict(statuses)}')
    rep = summarize(out)
    print(report(rep))
    return status(rep, args, ('speed',))


def oracle(args):
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    results = out / 'oracle.jsonl'
    done = {r['sha256'] for r in read_rows(results) if r.get('status') != 'missing'}
    queue = [f for f in queue_of(args) if f['sha256'] not in done]
    counts = collections.Counter()

    def one(f):
        row = {'sha256': f['sha256'], 'class': 'vp9', 'ext': f['ext'], 'size': f['size'], 'holdout': f['holdout']}
        path = fetch(args, f)
        if path is None:
            return dict(row, status='missing')
        t = time.monotonic()
        try:
            row['frames'] = to.frames(args.decode_oracle, args.ffmpeg, args.ffprobe, path, f['ext'].lower(),
                                      args.backend)
            row['status'] = 'ok'
        except subprocess.TimeoutExpired as e:
            row['status'], row['error'] = 'timeout', str(e)[:300]
        except Exception as e:  # noqa: BLE001 - recorded, so that one file cannot stop the run
            row['status'], row['error'] = 'worker_error', repr(e)[:500]
        row['seconds'] = round(time.monotonic() - t, 3)
        return row

    with results.open('a') as fh, concurrent.futures.ThreadPoolExecutor(args.jobs) as ex:
        for i, row in enumerate(ex.map(one, queue)):
            fh.write(json.dumps(row) + '\n')
            fh.flush()
            counts[row['status']] += 1
            counts['agree'] += bool(row.get('frames', {}).get('agree'))
            if (i + 1) % 10 == 0 or i + 1 == len(queue):
                tc.log(f'{i + 1}/{len(queue)} {dict(counts)}')
    rep = summarize(out)
    print(report(rep))
    return status(rep, args, ('oracle',))


def stat(values):
    med, lo, hi = tc.bootstrap(values)
    return {'n': len(values), 'median': med, 'ci95': [lo, hi],
            'below_1': (sum(1 for x in values if x < 1) / len(values)) if values else None,
            'max': max(values) if values else None}


def group_of(r):
    return 'held-out' if r.get('holdout') else 'dev'


def bucket_of(seconds):
    return next(n for n, lo, hi in TIME_BUCKETS if lo <= seconds and (hi is None or seconds < hi))


def pair_ratio(r, bar):
    a, b = (f'{n}:{BARS[bar][0]}' for n in PAIR)
    med = r.get('median_seconds', {})
    if any(n in r.get('excluded', {}) for n in PAIR) or a not in med or b not in med:
        return None
    return med[a] / med[b]


def under_30(ok, bar, arm):
    vals = [r['psnr'][bar][arm] for r in ok if arm in (r.get('psnr') or {}).get(bar, {})]
    return sum(1 for v in vals if v is None or v < 30)


def summarize(out):
    out = pathlib.Path(out)
    rows = list({r['sha256']: r for r in read_rows(out / 'files.jsonl')}.values())
    ok = [r for r in rows if r.get('status') == 'ok']
    gates = json.loads((out / 'gate.json').read_text()) if (out / 'gate.json').exists() else []
    info = json.loads((out / 'host.json').read_text()) if (out / 'host.json').exists() else {}
    arms = list(info.get('arms', {})) or sorted({a for r in ok for a in r.get('ratios', {})})
    rep = {'void': False, 'reasons': [], 'gate': gates, 'arms': arms, 'files': len(rows),
           'expected_backends': info.get('expected_backends')}
    if rows:
        before = next((g for g in gates if g['which'] == 'before'), None)
        after = next((g for g in reversed(gates) if g['which'] == 'after'), None)
        if not before or not after:
            rep['void'], rep['reasons'] = True, ['no gate time before and after']
        else:
            rep['gate_drift'] = after['median'] / before['median'] - 1
            if abs(rep['gate_drift']) > GATE_DRIFT:
                rep['void'] = True
                rep['reasons'].append(f'the gate moved {rep["gate_drift"]:+.1%}')
    rep['apart'] = dict(collections.Counter(
        f'{r["status"]}: {json.dumps(r["error"])[:80]}' if r.get('error') else r['status']
        for r in rows if r.get('status') != 'ok'))
    rep['backends'] = {a: dict(collections.Counter(((r.get('arms') or {}).get(a) or {}).get('backend') for r in ok))
                       for a in arms}
    rep['excluded'] = {a: sum(1 for r in ok if a in r.get('excluded', {})) for a in arms}
    rep['frame_differs_between_arms'] = sum(1 for r in ok if r.get('frame_differs_between_arms'))
    rep['bars'], rep['by_ffmpeg_time'] = {}, {}
    names = arms + (['/'.join(PAIR)] if all(n in arms for n in PAIR) else [])
    for bar, (_, ffname) in BARS.items():
        per = rep['bars'][bar] = {}
        by_time = rep['by_ffmpeg_time'][bar] = {}
        for name in names:
            vals = [(r, pair_ratio(r, bar) if '/' in name else (r.get('ratios', {}).get(name) or {}).get(bar))
                    for r in ok]
            vals = [(r, v) for r, v in vals if v is not None]
            per[name] = {g: dict(stat([v for r, v in vals if g in ('all', group_of(r))]),
                                 left_out=sum(1 for r in ok if g in ('all', group_of(r)))
                                 - sum(1 for r, _ in vals if g in ('all', group_of(r))))
                         for g in GROUPS}
            timed_vals = [(r['median_seconds'][ffname], v) for r, v in vals if ffname in r['median_seconds']]
            by_time[name] = {b: stat([v for s, v in timed_vals if bucket_of(s) == b]) for b, _, _ in TIME_BUCKETS}
    rep['psnr_under_30'] = {bar: {a: under_30(ok, bar, a) for a in arms} for bar in ('S2', 'S3')}
    decode = [(r, r['decode']) for r in ok if r.get('decode')]
    rep['decode_only'] = {
        'files': len(decode),
        'ratios': {k: {g: stat([d['ratios'][k] for r, d in decode if k in d['ratios'] and g in ('all', group_of(r))])
                       for g in GROUPS} for k in (f'{a}/{b}' for a, b in DECODE_RATIOS)},
        'median_seconds': {c: statistics.median([d['median_seconds'][c] for _, d in decode if c in d['median_seconds']])
                           for c in sorted({c for _, d in decode for c in d['median_seconds']})},
        'failed': dict(collections.Counter(c for _, d in decode for c in d.get('failed', {}))),
        'skipped': dict(collections.Counter(d['oracle_skipped'] for _, d in decode if d.get('oracle_skipped'))),
        'ffmpeg_decoders': info.get('ffmpeg_decoders')}
    rep['oracle'] = oracle_summary(read_rows(out / 'oracle.jsonl'))
    tc._write(out / 'summary.json', rep)
    return rep


def oracle_summary(rows):
    """Streams and frames as docs/design/decode.md counts them: a stream that did not decode, or gave no frame,
    disagrees in full, with FFmpeg's frames of it compared and unequal; a run that hung or crashed is a disagreeing
    stream with no frames known."""
    rows = list({r['sha256']: r for r in rows if r.get('status') != 'missing'}.values())
    if not rows:
        return None
    out = {}
    for g in GROUPS:
        fr = [r.get('frames') or {'status': f'run {r["status"]}'} for r in rows if g in ('all', group_of(r))]
        ok = [f for f in fr if f.get('status') == 'ok']
        frames = sum(f['compared'] for f in ok) + sum(
            f.get('ffmpeg_frames', 0) for f in fr if f.get('status') in ('decode_failed', 'no_frames'))
        equal = sum(f['equal'] for f in ok)
        agree = sum(1 for f in ok if f.get('agree'))
        out[g] = {'streams': len(fr), 'agree': agree, 'frames': frames, 'equal': equal,
                  'streams_share': agree / len(fr) if fr else None,
                  'frames_share': equal / frames if frames else None,
                  'not_ok': dict(collections.Counter(f.get('status') for f in fr if f.get('status') != 'ok')),
                  'decoders': dict(collections.Counter(str(f.get('decoder')) for f in ok))}
    return out


def fmt(x, digits=3):
    return '-' if x is None else f'{x:.{digits}f}'


def pct(x, digits=0):
    return '-' if x is None else f'{100 * x:.{digits}f}%'


def stat_cells(s):
    return (f'{s["n"]} | {fmt(s["median"])} | {fmt(s["ci95"][0])} to {fmt(s["ci95"][1])} | {pct(s["below_1"])} | '
            f'{fmt(s["max"])}')


def report(rep):
    lines = []
    if rep['files']:
        lines += [f'Gate drift {fmt(rep.get("gate_drift"), 4)}; void: {rep["void"]} {rep["reasons"]}; '
                  f'expected backends {rep["expected_backends"]}\n',
                  f'Arms {rep["arms"]}; backends {rep["backends"]}; ratios left out for another backend '
                  f'{rep["excluded"]}; files where the arms chose another frame {rep["frame_differs_between_arms"]}; '
                  f'apart {rep["apart"]}\n']
        for bar, per in rep['bars'].items():
            lines += [f'### {bar} (ratio: arm / FFmpeg; {"/".join(PAIR)}: libvpx / fallback)\n',
                      '| Arm | Set | Files | Median | 95% CI | Below 1.00 | Max | Left out |',
                      '|---|---|---:|---:|---|---:|---:|---:|']
            lines += [f'| {name} | {g} | {stat_cells(s)} | {s["left_out"]} |' for name, gs in per.items()
                      for g, s in gs.items()]
            lines.append('')
        lines += ["### By FFmpeg's time for the bar's command\n",
                  "| Bar | Arm | FFmpeg's time | Files | Median | 95% CI | Below 1.00 | Max |",
                  '|---|---|---|---:|---:|---|---:|---:|']
        lines += [f'| {bar} | {name} | {b} | {stat_cells(s)} |' for bar, per in rep['by_ffmpeg_time'].items()
                  for name, bs in per.items() for b, s in bs.items()]
        d = rep['decode_only']
        lines += [f'\nFrames under 30 dB against FFmpeg\'s PNG (or not comparable), by bar and arm: '
                  f'{rep["psnr_under_30"]}\n',
                  f'### Decode only (information, no bar; {d["files"]} files; first / second)\n',
                  f'FFmpeg decoders in the binary: {d["ffmpeg_decoders"]}; median seconds {d["median_seconds"]}; '
                  f'failed {d["failed"]}; skipped {d["skipped"]}\n',
                  '| Pair | Set | Files | Median | 95% CI | Below 1.00 | Max |', '|---|---|---:|---:|---|---:|---:|']
        lines += [f'| {k} | {g} | {stat_cells(s)} |' for k, gs in d['ratios'].items() for g, s in gs.items()]
        lines.append('')
    if rep['oracle']:
        lines += ['### Frames against FFmpeg framemd5 (bar 1: 99% of streams, 99.9% of frames)\n',
                  '| Set | Streams | Agree | Frames | Equal | Not decoded | Decoder |',
                  '|---|---:|---:|---:|---:|---|---|']
        lines += [f'| {g} | {v["streams"]} | {v["agree"]} ({pct(v["streams_share"], 1)}) | {v["frames"]} | '
                  f'{v["equal"]} ({pct(v["frames_share"], 2)}) | {v["not_ok"]} | {v["decoders"]} |'
                  for g, v in rep['oracle'].items()]
    return '\n'.join(lines)


def status(rep, args, parts):
    """3 for a void run; 1 when a judged arm misses the bar or has no timed file, or bar 1 is missed; else 0."""
    if 'speed' in parts and rep['void']:
        return 3
    problems = []
    if 'speed' in parts:
        for bar, per in rep['bars'].items():
            for name in [a for a in args.judge.split(',') if a]:
                s = (per.get(name) or {}).get(args.judge_set)
                if s is None or s['n'] == 0:
                    problems.append(f'{name} {bar}: no timed file')
                elif s['median'] > args.bar:
                    problems.append(f'{name} {bar}: median {s["median"]:.4f} above {args.bar}')
    o = rep['oracle'] if 'oracle' in parts else None
    if o:
        if o['all']['streams_share'] < STREAMS_SHARE:
            problems.append(f'streams agreeing {pct(o["all"]["streams_share"], 1)}')
        if (o['all']['frames_share'] or 0) < FRAMES_SHARE:
            problems.append(f'frames agreeing {pct(o["all"]["frames_share"], 2)}')
    for p in problems:
        print('FAIL:', p)
    return 1 if problems else 0


def parse_arms(values, parser):
    arms = []
    for v in values or []:
        name, _, path = v.partition('=')
        if not re.fullmatch(r'[A-Za-z0-9_-]+', name) or not path or name in dict(arms):
            parser.error(f'--coreav {v!r}: NAME=PATH, NAME distinct, of letters, digits, - and _')
        arms.append((name, os.path.abspath(path)))
    return arms


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument('command', choices=('select', 'run', 'oracle', 'summarize'))
    p.add_argument('--selection')
    p.add_argument('--files')
    p.add_argument('--coreav', action='append', metavar='NAME=PATH')
    p.add_argument('--decode-oracle')
    p.add_argument('--decode-backend', default='libvpx', help="the decode oracle's backend for the decode-only timing")
    p.add_argument('--decode-max-bytes', type=int, default=1 << 30,
                   help='no decode-oracle timing of a file above this size (it holds all packets in memory)')
    p.add_argument('--decode-memory-gib', type=int, default=0,
                   help="the decode oracle's memory budget per decoder process, in GiB (0: the default)")
    p.add_argument('--no-decode', action='store_true', help='skip the decode-only timing')
    p.add_argument('--decode-rounds', type=int, default=3, help='rounds of the decode-only timing, after its warm-up')
    p.add_argument('--backend', default='libvpx', help="oracle: the decode oracle's backend (FFmpeg decodes with vp9)")
    p.add_argument('--ffmpeg', default=shutil.which('ffmpeg'))
    p.add_argument('--ffprobe', default=shutil.which('ffprobe'))
    p.add_argument('--gate')
    p.add_argument('--expect', action='append', default=[], metavar='NAME=BACKEND',
                   help='the backend an arm must name, over the default (libvpx=libvpx, fallback=ffmpeg): for a '
                        'check of the plumbing with one build, never for a claim')
    p.add_argument('--out')
    p.add_argument('--tmp', default='/dev/shm/vp9-speed')
    p.add_argument('--set', choices=('all', 'holdout', 'dev'), default='all')
    p.add_argument('--bars', default=','.join(BARS))
    p.add_argument('--limit', type=int, default=0)
    p.add_argument('--sha', action='append', default=[], help='only files whose SHA-256 starts with this (repeatable)')
    p.add_argument('--jobs', type=int, default=2, help='oracle: files checked at a time')
    p.add_argument('--judge', default='libvpx', help='the arms the exit status judges (comma-separated)')
    p.add_argument('--judge-set', choices=('all', 'dev', 'held-out'), default='all')
    p.add_argument('--bar', type=float, default=BAR)
    p.add_argument('--allow-non-tmpfs', action='store_true', help='time with outputs on a disk (not for a claim)')
    args = p.parse_args()
    args.bars = [b for b in args.bars.split(',') if b]
    if not set(args.bars) <= set(BARS):
        p.error(f'--bars: {list(BARS)}')
    if args.command == 'select':
        if not args.selection:
            p.error('select needs --selection')
        return select(args)
    if args.command == 'summarize':
        if not args.out:
            p.error('summarize needs --out')
        rep = summarize(args.out)
        print(report(rep))
        return status(rep, args, ('speed', 'oracle'))
    for need in ('selection', 'files', 'out', 'ffmpeg', 'ffprobe'):
        if not getattr(args, need):
            p.error(f'{args.command} needs --{need.replace("_", "-")}')
    args.ffmpeg, args.ffprobe = os.path.abspath(args.ffmpeg), os.path.abspath(args.ffprobe)
    tc.ENV = tc.child_env(args.ffmpeg)
    to.ENV = tc.ENV
    to.DECODE_ORACLE_ARGS = ['--max-memory-gib', str(args.decode_memory_gib)] if args.decode_memory_gib else []
    if args.decode_oracle:
        args.decode_oracle = os.path.abspath(args.decode_oracle)
    if args.command == 'oracle':
        if not args.decode_oracle:
            p.error('oracle needs --decode-oracle')
        return oracle(args)
    args.expected = dict(EXPECTED_BACKEND, **dict(v.partition('=')[::2] for v in args.expect))
    args.arms = parse_arms(args.coreav, p)
    if not args.arms or not args.gate:
        p.error('run needs at least one --coreav and --gate')
    args.gate = os.path.abspath(args.gate)
    args.decode = not args.no_decode
    return run(args)


if __name__ == '__main__':
    sys.exit(main())
