#!/usr/bin/env python3
"""Thumbnails planned from a window of the packets against planned from
every packet (docs/design/seek.md): the same packets decoded and the same
frame, so the same image, on every MP4 and MOV file with video.

    seek_check.py --decode-oracle PATH (--fixtures DIR | FILE...) [--times N] [--report OUT]

Each file is checked in keyframe and exact mode at N times (default 64)
spread from before the start to past the end, plus the edges: the start,
just after it, the end and just before and after it. Errors must match
too. Prints counts only. Exits 1 when a choice differs; or when, over the
files of a run, no choice was made from a window, which would mean the
window never took effect.
"""
import argparse
import json
import pathlib
import shutil
import subprocess
import sys

EXTS = ('mp4', 'mov', 'm4v', '3gp')


def duration(path, ffprobe):
    r = subprocess.run([ffprobe, '-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', str(path)],
                       capture_output=True, text=True, timeout=60, check=False)
    try:
        return float(r.stdout.strip())
    except ValueError:
        return None


def has_video(path, ffprobe):
    r = subprocess.run([ffprobe, '-v', 'error', '-select_streams', 'v', '-show_entries', 'stream=index',
                        '-of', 'csv=p=0', str(path)], capture_output=True, text=True, timeout=60, check=False)
    return bool(r.stdout.strip())


def times(length, n):
    out = {0.0, 0.001, 0.04}
    if length:
        out |= {round(length * k / n, 3) for k in range(n + 2)}
        out |= {round(max(length - 0.04, 0), 3), round(length, 3), round(length + 0.001, 3)}
    return sorted(t for t in out if 0 <= t < 1e12)


def choices(oracle, path, at, exact, seek):
    cmd = [oracle, 'choose', str(path), '--at', ','.join(f'{t:.3f}' for t in at), '--seek', seek]
    if exact:
        cmd.append('--exact')
    r = subprocess.run(cmd, capture_output=True, text=True, timeout=600, check=False)
    out = json.loads(r.stdout)
    if isinstance(out, dict):
        return [out] * len(at)
    return out


def check(oracle, path, ffprobe, n):
    """Counts for one file: checked, differing, from a window; and the first
    difference."""
    at = times(duration(path, ffprobe), n)
    counts = {'checked': 0, 'differ': 0, 'windowed': 0}
    first = None
    for exact in (False, True):
        on = choices(oracle, path, at, exact, 'on')
        off = choices(oracle, path, at, exact, 'off')
        for t, a, b in zip(at, on, off):
            counts['checked'] += 1
            counts['windowed'] += bool(a.get('windowed'))
            a = {k: v for k, v in a.items() if k != 'windowed'}
            b = {k: v for k, v in b.items() if k != 'windowed'}
            if a != b:
                counts['differ'] += 1
                if first is None:
                    first = {'at': t, 'exact': exact, 'window': a, 'every': b}
    return counts, first


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument('--decode-oracle', required=True)
    p.add_argument('--ffprobe', default=shutil.which('ffprobe'))
    p.add_argument('--fixtures')
    p.add_argument('--times', type=int, default=64)
    p.add_argument('--report')
    p.add_argument('files', nargs='*')
    a = p.parse_args()
    paths = [pathlib.Path(f) for f in a.files]
    if a.fixtures:
        paths += sorted(q for q in pathlib.Path(a.fixtures).iterdir() if q.suffix.lstrip('.').lower() in EXTS)
    total = {'files': 0, 'checked': 0, 'differ': 0, 'windowed': 0}
    report = []
    for path in paths:
        if not has_video(path, a.ffprobe):
            continue
        counts, first = check(a.decode_oracle, path, a.ffprobe, a.times)
        total['files'] += 1
        for k in counts:
            total[k] += counts[k]
        report.append({'file': path.name, **counts, 'first_difference': first})
        if first:
            print(f'DIFFERS {path.name}: {json.dumps(first)}', flush=True)
    if a.report:
        pathlib.Path(a.report).write_text(json.dumps({'total': total, 'files': report}, indent=1))
    print(json.dumps(total))
    return 1 if total['differ'] or not total['windowed'] else 0


if __name__ == '__main__':
    sys.exit(main())
