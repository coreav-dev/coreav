"""Where the time of a short AV1 thumbnail goes (docs/measurements/av1-thumbnail-fixed-cost.md, "Arms").

    python3 -I arms.py --files DIR --list LIST.jsonl --coreav PATH --decode-oracle PATH \
        --ffmpeg PATH --ffprobe PATH --out results.jsonl [--scripts DIR] [--tmp /dev/shm/arms]

LIST.jsonl has one JSON object per line with `sha256`, `ext` and `set`; DIR holds `<sha256>.<ext>`. Per file, at
the thumbnail bars' time T and width: one warm-up of every command, then ROUNDS rounds with the commands in rotated
order; a command's time is the median of its rounds. Every command is timed from outside as a process, outputs on a
tmpfs. Results carry the file's SHA-256, extension and set, no names. Uses the Python standard library and
scripts/thumbnail_oracle.py (from --scripts).
"""
import argparse
import json
import pathlib
import statistics
import subprocess
import sys
import time

ROUNDS = 5
TIMEOUT = 300
WIDTH = 320


def timed(cmd):
    start = time.perf_counter()
    r = subprocess.run(cmd, capture_output=True, timeout=TIMEOUT, check=False)
    return time.perf_counter() - start, r


def commands(args, path, t, tmp):
    at = f'{t:.3f}'
    ours = lambda *extra: [args.coreav, 'thumbnail', str(path), '--at', at, '--width', str(WIDTH),
                           '-o', str(tmp / 'o.jpg'), '--force', *extra]
    oracle = lambda *extra: [args.decode_oracle, 'thumbnail', str(path), '--at', at, '--keyframe', '--backend',
                             'dav1d', '--width', str(WIDTH), '-o', str(tmp / 'd.jpg'), *extra]
    out = {
        'ff_t': [args.ffmpeg, '-v', 'error', '-y', '-ss', at, '-i', str(path), '-frames:v', '1', '-vf',
                 f'scale={WIDTH}:-2', str(tmp / 'f.jpg')],
        'key': ours(),
        'exact': ours('--exact'),
        'exact_aa': ours('--exact'),
        'worker': oracle(),
        'in_process': oracle('--in-process'),
    }
    for n in (1, 2, 4):
        out[f'key_t{n}'] = ours('--threads', str(n))
        out[f'exact_t{n}'] = ours('--exact', '--threads', str(n))
    return out


def one(entry, args, tmp, oracle):
    path = pathlib.Path(args.files) / f'{entry["sha256"]}.{entry["ext"]}'
    out = {'sha256': entry['sha256'], 'ext': entry['ext'], 'set': entry['set']}
    t = oracle.at_time(path, args.ffprobe)
    if t is None:
        out['status'] = 'no_duration'
        return out
    out['t'] = t
    cmds = commands(args, path, t, tmp)
    failed = {}
    for name, cmd in cmds.items():
        _, r = timed(cmd)
        if r.returncode != 0:
            failed[name] = r.stderr.decode(errors='replace')[-300:]
    names = [n for n in cmds if n not in failed]
    times = {n: [] for n in names}
    for round_ in range(ROUNDS):
        k = round_ % len(names) if names else 0
        for name in names[k:] + names[:k]:
            if name in failed:
                continue
            dt, r = timed(cmds[name])
            if r.returncode != 0:
                failed[name] = r.stderr.decode(errors='replace')[-300:]
                continue
            times[name].append(dt)
    out['median_seconds'] = {n: statistics.median(v) for n, v in times.items() if v and n not in failed}
    out['rounds_seconds'] = {n: v for n, v in times.items() if n not in failed}
    if failed:
        out['failed'] = failed
    out['status'] = 'ok'
    return out


def main():
    p = argparse.ArgumentParser()
    for name in ('--files', '--list', '--coreav', '--decode-oracle', '--ffmpeg', '--ffprobe', '--out'):
        p.add_argument(name, required=True)
    p.add_argument('--scripts', default=str(pathlib.Path(__file__).resolve().parents[3] / 'scripts'))
    p.add_argument('--tmp', default='/dev/shm/arms')
    args = p.parse_args()
    sys.path.insert(0, args.scripts)
    import thumbnail_oracle
    tmp = pathlib.Path(args.tmp)
    tmp.mkdir(parents=True, exist_ok=True)
    entries = [json.loads(line) for line in pathlib.Path(args.list).read_text().splitlines() if line.strip()]
    with open(args.out, 'a') as fh:
        for i, entry in enumerate(entries):
            try:
                r = one(entry, args, tmp, thumbnail_oracle)
            except subprocess.TimeoutExpired as e:
                r = {'sha256': entry['sha256'], 'ext': entry['ext'], 'set': entry['set'], 'status': 'timeout',
                     'error': str(e)[:200]}
            fh.write(json.dumps(r) + '\n')
            fh.flush()
            print(f'{i + 1}/{len(entries)} {r["status"]}', flush=True)


if __name__ == '__main__':
    main()
