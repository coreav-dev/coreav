#!/usr/bin/env python3
"""Time coreav against ffprobe on Matroska files (docs/design/mkv.md).

For each file, runs each command --repeat times, one at a time, and keeps
the fastest wall time; both sides are a process per run, so process start-up
counts on both:

- probe: `coreav probe FILE` against
  `ffprobe -v error -of json -show_format -show_streams FILE`;
- demux: `mkv-adapter count FILE` (probe, then every packet) against
  `ffprobe -v error -count_packets -show_entries stream=nb_read_packets FILE`,
  which also reads every packet.

Writes per-file times as JSON (no file names) and prints
medians and totals. Informational: no bar is attached to these numbers.

  bench.py --media DIR [--media DIR ...] --out RESULTS.json [--repeat 3]
"""
import argparse
import json
import pathlib
import statistics
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
TIMEOUT = 300


def timed(command, repeat):
    best = None
    for _ in range(repeat):
        started = time.perf_counter()
        try:
            done = subprocess.run(command, capture_output=True, timeout=TIMEOUT, check=False)
        except subprocess.TimeoutExpired:
            return None
        elapsed = time.perf_counter() - started
        if done.returncode != 0:
            return None
        best = elapsed if best is None else min(best, elapsed)
    return best


def summary(rows, key):
    pairs = [(r[f'ffprobe_{key}'], r[f'coreav_{key}']) for r in rows
             if r[f'ffprobe_{key}'] is not None and r[f'coreav_{key}'] is not None]
    if not pairs:
        return {}
    ratios = [f / c for f, c in pairs if c > 0]
    return {
        'files': len(pairs),
        'ffprobe_median_s': statistics.median(f for f, _ in pairs),
        'coreav_median_s': statistics.median(c for _, c in pairs),
        'ffprobe_total_s': sum(f for f, _ in pairs),
        'coreav_total_s': sum(c for _, c in pairs),
        'median_speedup': statistics.median(ratios),
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--media', action='append', required=True)
    parser.add_argument('--out', required=True)
    parser.add_argument('--repeat', type=int, default=3)
    parser.add_argument('--coreav', default=str(ROOT / 'target' / 'release' / 'coreav'))
    parser.add_argument('--adapter', default=str(ROOT / 'target' / 'release' / 'mkv-adapter'))
    args = parser.parse_args()
    files = sorted(p for d in args.media for p in pathlib.Path(d).iterdir()
                   if p.suffix in ('.mkv', '.webm'))
    rows = []
    for path in files:
        rows.append({
            'size': path.stat().st_size,
            'ffprobe_probe': timed(['ffprobe', '-v', 'error', '-of', 'json', '-show_format', '-show_streams',
                                    str(path)], args.repeat),
            'coreav_probe': timed([args.coreav, 'probe', str(path)], args.repeat),
            'ffprobe_demux': timed(['ffprobe', '-v', 'error', '-count_packets', '-show_entries',
                                    'stream=nb_read_packets', '-of', 'csv=p=0', str(path)], args.repeat),
            'coreav_demux': timed([args.adapter, 'count', str(path)], args.repeat),
        })
    result = {'probe': summary(rows, 'probe'), 'demux': summary(rows, 'demux'), 'files': rows}
    pathlib.Path(args.out).write_text(json.dumps(result, indent=1) + '\n')
    print(json.dumps({k: v for k, v in result.items() if k != 'files'}, indent=1))
    return 0


if __name__ == '__main__':
    sys.exit(main())
