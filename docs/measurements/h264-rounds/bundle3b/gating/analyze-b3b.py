"""python3 -I analyze-b3b.py GATING_DIR: bundle 3b's bars B3b-1 to B3b-6 (plan.md, "Bars") from the calls' reports:
1t/g.<class>.json and 1t/h.<class>.json (one thread on the bars' 2.90 GHz class, the 32 training and the 64 held-out
streams) and gcp-8t/t8.<class>.json (eight threads, the 32 training streams). The one-thread runs on the Compute Engine
class (gcp-1t-train/, gcp-1t-heldout/) are read as information and decide nothing. Each bar is the median of the
per-stream ratios k over base (each stream's time the median of its rounds) with a 95% bootstrap interval over streams
(2,000 resamples, random.Random(20261026)). Time ratios to FFmpeg 9.0.2, and how many streams are below 1, are
information."""
import json, os, random, statistics, sys

CLASSES = ('sd', '720p', '1080p', '4k')


def load(d, prefix):
    rows = []
    for c in CLASSES:
        with open(os.path.join(d, f'{prefix}.{c}.json')) as f:
            rows += json.load(f)['streams']
    return rows


def arm(row, name):
    return statistics.median(row['rounds'][name])


def ci(v, seed=20261026, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(rows, mode):
    v = [arm(r, f'k_{mode}') / arm(r, f'base_{mode}') for r in rows]
    ff = [[r['ratio'][f'{a}:{mode}'] for r in rows] for a in ('base', 'k')]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'lo': lo, 'hi': hi, 'faster': sum(x < 1 for x in v),
            'ff': [statistics.median(x) for x in ff], 'ff_faster': [sum(t < 1 for t in x) for x in ff]}


def table(name, rows, modes):
    out = {}
    for mode in modes:
        for group in ('all',) + CLASSES:
            g = rows if group == 'all' else [r for r in rows if r['class'] == group]
            s = out[(mode, group)] = summary(g, mode)
            print(f"{name:12} {mode:7} {group:5} n={s['n']:2} k/base {s['median']:.4f} [{s['lo']:.4f}, {s['hi']:.4f}] "
                  f"faster {s['faster']}/{s['n']} | to FFmpeg base {s['ff'][0]:.3f} ({s['ff_faster'][0]}/{s['n']} below 1) "
                  f"k {s['ff'][1]:.3f} ({s['ff_faster'][1]}/{s['n']} below 1)")
    print(f"{name:12} unresolved {[r['stream'][:12] for r in rows if r.get('unresolved')]}")
    return out


d = sys.argv[1]
t1 = table('1T train', load(os.path.join(d, '1t'), 'g'), ('discard', 'write'))
h1 = table('1T heldout', load(os.path.join(d, '1t'), 'h'), ('discard', 'write'))
t8 = table('8T train', load(os.path.join(d, 'gcp-8t'), 't8'), ('discard', 'write'))
table('CE1T train', load(os.path.join(d, 'gcp-1t-train'), 'g'), ('discard', 'write'))
table('CE1T heldout', load(os.path.join(d, 'gcp-1t-heldout'), 'g'), ('discard', 'write'))
bars = {
    'B3b-1': t1[('discard', 'all')]['median'] <= 0.990 and t1[('discard', 'all')]['hi'] < 1.0,
    'B3b-2': h1[('discard', 'all')]['median'] <= 0.995 and h1[('discard', 'all')]['hi'] < 1.0,
    'B3b-3': t1[('write', 'all')]['median'] <= 1.000,
    'B3b-4': all(t1[(m, c)]['median'] <= 1.005 for m in ('discard', 'write') for c in CLASSES),
    'B3b-5': all(t8[(m, 'all')]['median'] <= 1.005 for m in ('discard', 'write')),
    'B3b-6': all(t8[(m, c)]['median'] <= 1.010 for m in ('discard', 'write') for c in CLASSES),
}
print('bars', bars)
