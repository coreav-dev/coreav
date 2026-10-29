"""Tables of docs/measurements/av1-thumbnail-fixed-cost.md from its evidence.

    python3 -I summarize.py DIR

DIR holds speed-dev.jsonl and speed-holdout.jsonl (the thumbnail spike's speed runner) and arms.jsonl (arms.py).
Medians of per-file ratios with a bootstrap 95% confidence interval (10,000 resamples, seed 207).
"""
import json
import pathlib
import random
import statistics
import sys

BARS = ('S1', 'S2', 'S3', 'S4', 'S5')
CLASSES = (('under 50 ms', 0, 0.05), ('50 to 300 ms', 0.05, 0.3), ('over 300 ms', 0.3, float('inf')))


def load(path):
    return [r for r in (json.loads(l) for l in path.read_text().splitlines() if l.strip())
            if r.get('class', 'av1') == 'av1']


def ci(values):
    rng = random.Random(207)
    meds = sorted(statistics.median(rng.choices(values, k=len(values))) for _ in range(10000))
    return meds[249], meds[9749]


def cell(values):
    if not values:
        return 'none'
    lo, hi = ci(values)
    faster = sum(v < 1 for v in values) / len(values)
    return f'{statistics.median(values):.3f} ({lo:.3f} to {hi:.3f}), {faster:.0%} faster'


def time_class(seconds):
    return next(name for name, lo, hi in CLASSES if lo <= seconds < hi)


def main():
    d = pathlib.Path(sys.argv[1])
    speed = {s: load(d / f'speed-{s}.jsonl') for s in ('dev', 'holdout')}
    ok = {s: [r for r in rows if r.get('status') == 'ok'] for s, rows in speed.items()}
    for s, rows in speed.items():
        other = sorted({r.get('status') for r in rows} - {'ok'})
        print(f'{s}: {len(ok[s])} ok of {len(rows)}; other statuses {other}; '
              f'failed commands {sum(1 for r in ok[s] if r.get("failed"))}')
    print('\n| Bar | Development (62) | Held-out (26) |\n|---|---|---|')
    for bar in BARS:
        print(f'| {bar} | ' + ' | '.join(cell([r['ratios'][bar] for r in ok[s] if bar in r['ratios']])
                                       for s in ('dev', 'holdout')) + ' |')
    everyone = ok['dev'] + ok['holdout']
    ff = {r['sha256']: r['median_seconds']['ff_t_jpg'] for r in everyone if 'ff_t_jpg' in r['median_seconds']}
    print("\nBy FFmpeg's time for S3's command, all files:\n")
    print('| Class | Files | S3 | S1 | S3, CoreAV minus FFmpeg (ms) |\n|---|---:|---|---|---:|')
    for name, lo, hi in CLASSES:
        rows = [r for r in everyone if r['sha256'] in ff and lo <= ff[r['sha256']] < hi]
        diff = [1e3 * (r['median_seconds']['ours_exact_jpg'] - r['median_seconds']['ff_t_jpg'])
                for r in rows if 'ours_exact_jpg' in r['median_seconds']]
        print(f'| {name} | {len(rows)} | {cell([r["ratios"]["S3"] for r in rows if "S3" in r["ratios"]])} | '
              f'{cell([r["ratios"]["S1"] for r in rows if "S1" in r["ratios"]])} | '
              f'{statistics.median(diff):+.1f} |' if diff else f'| {name} | 0 | | | |')
    arms_path = d / 'arms.jsonl'
    if not arms_path.exists():
        return
    arms = [r for r in load(arms_path) if r.get('status') == 'ok']
    print(f'\narms: {len(arms)} ok; failed commands {sum(1 for r in arms if r.get("failed"))}')
    m = lambda r, a, b: r['median_seconds'][a] / r['median_seconds'][b]
    has = lambda r, *names: all(n in r['median_seconds'] for n in names)
    aa = [m(r, 'exact_aa', 'exact') for r in arms if has(r, 'exact_aa', 'exact')]
    print(f'A/A (exact_aa over exact), all files: {cell(aa)}')
    dev = [r for r in arms if r['set'] == 'dev' and has(r, 'worker', 'in_process')]
    over = [m(r, 'worker', 'in_process') - 1 for r in dev]
    print(f'worker overhead, development files: median {statistics.median(over):.3f} ({len(over)} files)')
    print('\nThreads arms over the default, by time class:\n')
    names = ('key_t1', 'key_t2', 'key_t4', 'exact_t1', 'exact_t2', 'exact_t4')
    print('| Class | Files | ' + ' | '.join(names) + ' | worker minus in-process (ms) |')
    print('|---|---:|' + '---|' * (len(names) + 1))
    for name, lo, hi in CLASSES:
        rows = [r for r in arms if r['sha256'] in ff and lo <= ff[r['sha256']] < hi]
        cells = []
        for n in names:
            base = n.split('_')[0]
            v = [m(r, n, base) for r in rows if has(r, n, base)]
            dv = [1e3 * (r['median_seconds'][n] - r['median_seconds'][base]) for r in rows if has(r, n, base)]
            cells.append(f'{statistics.median(v):.3f} ({statistics.median(dv):+.1f} ms)' if v else 'none')
        wd = [1e3 * (r['median_seconds']['worker'] - r['median_seconds']['in_process'])
              for r in rows if has(r, 'worker', 'in_process')]
        worker = f'{statistics.median(wd):+.1f}' if wd else 'none'
        print(f'| {name} | {len(rows)} | ' + ' | '.join(cells) + f' | {worker} |')


if __name__ == '__main__':
    main()
