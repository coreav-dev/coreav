#!/usr/bin/env python3
"""Judge a report of scripts/ts_speed.py against the packet-listing speed bars of docs/design/mpegts.md ("Speed bars,
round 2"), or against the level that decides whether to spend the draw (--gate).

    ts_speed_bars.py REPORT.json [--summary OUT.json] [--gate]

Per file, each arm's time is the median of its rounds, and the file's ratio is CoreAV's time over libavformat's.
Files fall in the bar's size classes: `small` under 16 MB, `medium` 16 MB to under 64 MB, `large` 64 MB to 200 MB
(1 MB = 10^6 bytes). Overall and in each class: the median ratio with a bootstrap 95% interval (10,000 resamples of
the files, random.Random(89), the 2.5th and 97.5th percentiles of the resampled medians) and the share of files on
which CoreAV is faster. A class of fewer than 5 files is reported and not judged. The bars: median and upper bound
at most 1.00, overall and in every judged class. The null arm (the same binary again) gives each file a ratio of
its time over CoreAV's; the run is void when their median is outside 0.99 to 1.01. The command arms, when the
report has them, are reported as information.

--gate judges instead the level of docs/design/mpegts.md ("When the code goes to the draw"): a median of at most
0.90 overall and at most 0.95 on the files of 64 MB or more.

The report is refused, and nothing is judged, when a file is over 200 MB, a run of a listing arm (its warm-up
included) failed or has no time, or two listing arms disagree on a file's access units (every run of CoreAV and its
null arm, the warm-ups included, must list the same count and bytes). The exit status is 0 when every bar is met, 1 when one is missed, 3 for a void run and 2 for a refused
report.
"""
import argparse
import json
import math
import random
import statistics
import sys

MB = 10 ** 6
CLASSES = (('small', 0, 16 * MB), ('medium', 16 * MB, 64 * MB), ('large', 64 * MB, 200 * MB + 1))
MIN_CLASS = 5
RESAMPLES = 10_000
SEED = 89
BAR = 1.00
NULL_BAND = (0.99, 1.01)
GATE_ALL, GATE_LARGE, GATE_LARGE_FROM = 0.90, 0.95, 64 * MB
LISTING = ('coreav', 'null', 'lav')


class Refused(Exception):
    pass


def positive(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value > 0


def size_class(size):
    for name, low, high in CLASSES:
        if low <= size < high:
            return name
    raise Refused(f'a file of {size} bytes is over the 200 MB of the large class')


def interval(values, seed=SEED):
    rng = random.Random(seed)
    medians = sorted(statistics.median(rng.choices(values, k=len(values))) for _ in range(RESAMPLES))
    return medians[int(0.025 * RESAMPLES)], medians[int(0.975 * RESAMPLES) - 1]


def per_file(report):
    """Per file: its name, size, class and the median time of each arm."""
    rows = []
    for f in report.get('files', []):
        name = f.get('name')
        row = {'name': name, 'bytes': f.get('bytes'), 'class': size_class(f.get('bytes', 0)), 'ns': {}}
        listed = set()
        for arm in LISTING:
            warm = f.get('warm', {}).get(arm)
            if not (isinstance(warm, dict) and warm.get('ok') is True and positive(warm.get('ns'))):
                raise Refused(f'{name}: the warm-up run of {arm} failed or has no time')
            if arm in ('coreav', 'null'):
                listed.add((warm.get('count'), warm.get('bytes')))
        for arm, runs in f.get('runs', {}).items():
            if not runs or not all(isinstance(r, dict) and r.get('ok') is True and positive(r.get('ns')) for r in runs):
                if arm in LISTING:
                    raise Refused(f'{name}: a run of {arm} failed or has no time')
                continue
            row['ns'][arm] = statistics.median(r['ns'] for r in runs)
            if arm in ('coreav', 'null'):
                listed |= {(r.get('count'), r.get('bytes')) for r in runs}
        missing = [arm for arm in LISTING if arm not in row['ns']]
        if missing:
            raise Refused(f'{name}: no runs of {", ".join(missing)}')
        if len(listed) != 1:
            raise Refused(f'{name}: CoreAV and its null arm list different access units: {sorted(listed, key=str)}')
        rows.append(row)
    if not rows:
        raise Refused('the report has no files')
    return rows


def group(ratios):
    low, high = interval(ratios)
    return {'n': len(ratios), 'median': statistics.median(ratios), 'low': low, 'high': high,
            'faster': sum(r < 1 for r in ratios)}


def analyze(report, gate=False):
    rows = per_file(report)
    ratio = {r['name']: r['ns']['coreav'] / r['ns']['lav'] for r in rows}
    null = [r['ns']['null'] / r['ns']['coreav'] for r in rows]
    summary = {'files': [{**r, 'ratio': ratio[r['name']]} for r in rows],
               'null': {'median': statistics.median(null), 'band': list(NULL_BAND)},
               'overall': group(list(ratio.values())), 'classes': {}, 'bars': {}}
    summary['void'] = not NULL_BAND[0] <= summary['null']['median'] <= NULL_BAND[1]
    for name, _, _ in CLASSES:
        values = [ratio[r['name']] for r in rows if r['class'] == name]
        if values:
            summary['classes'][name] = {**group(values), 'judged': len(values) >= MIN_CLASS}
    for arm in ('probe-coreav', 'probe-ffprobe'):
        if all(arm in r['ns'] for r in rows):
            summary.setdefault('commands', {})[arm] = statistics.median(r['ns'][arm] for r in rows)
    if 'commands' in summary and len(summary['commands']) == 2:
        summary['commands']['ratio'] = statistics.median(
            r['ns']['probe-coreav'] / r['ns']['probe-ffprobe'] for r in rows)
    if gate:
        large = [ratio[r['name']] for r in rows if r['bytes'] >= GATE_LARGE_FROM]
        summary['bars']['gate overall'] = summary['overall']['median'] <= GATE_ALL
        summary['bars']['gate 64 MB or more'] = bool(large) and statistics.median(large) <= GATE_LARGE
        summary['gate_large'] = {'n': len(large), 'median': statistics.median(large) if large else None}
    else:
        judged = {'overall': summary['overall'], **{k: v for k, v in summary['classes'].items() if v['judged']}}
        for name, g in judged.items():
            summary['bars'][name] = g['median'] <= BAR and g['high'] <= BAR
    return summary


def show(s):
    print(f'null arm: median {s["null"]["median"]:.4f} (void outside {s["null"]["band"][0]} to {s["null"]["band"][1]})')
    for name, g in [('overall', s['overall'])] + list(s['classes'].items()):
        tag = '' if g.get('judged', True) else ' (fewer than 5 files: reported, not judged)'
        print(f'{name}: {g["n"]} files, median {g["median"]:.3f} [{g["low"]:.3f}, {g["high"]:.3f}], '
              f'faster on {g["faster"]}{tag}')
    if 'gate_large' in s:
        print(f'64 MB or more: {s["gate_large"]["n"]} files, median {s["gate_large"]["median"]}')
    for arm, v in s.get('commands', {}).items():
        print(f'{arm}: {v:.3f}' if arm == 'ratio' else f'{arm}: median {v / 1e6:.1f} ms')
    for name, met in s['bars'].items():
        print(f'bar {name}: {"met" if met else "MISSED"}')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('report')
    ap.add_argument('--summary')
    ap.add_argument('--gate', action='store_true')
    args = ap.parse_args()
    with open(args.report) as f:
        report = json.load(f)
    try:
        summary = analyze(report, args.gate)
    except Refused as e:
        print(f'refused: {e}', file=sys.stderr)
        return 2
    show(summary)
    if args.summary:
        with open(args.summary, 'w') as f:
            json.dump(summary, f, indent=1)
    if summary['void']:
        print('void: the null arm is outside its band')
        return 3
    return 0 if all(summary['bars'].values()) else 1


if __name__ == '__main__':
    sys.exit(main())
