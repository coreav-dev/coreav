"""python3 -I analyze-b2.py DIR1 [DIR8 [DIR8H]]: the bars B2-1 to B2-5 and B2-7 of bundle 2 (docs/measurements/h264-rounds/bundle2/plan.md)
from the calls' reports: g.<class>.json in DIR1 (one thread pinned, 32 training streams, arms `base` and `k`), t8.<class>.json in DIR8 (eight threads, the
32 training streams, Compute Engine) and in DIR8H (the 64 held-out streams). Per call and mode: the median per-stream ratio `k` over `base` with a
bootstrap 95% interval (2,000 resamples over streams), over all streams, per class and over the six CAVLC streams, and the ratios to FFmpeg with the
share of streams faster than it. B2-6 (output) is read from identity and conformance files, not here."""
import json, os, random, statistics, sys

classes = ('sd', '720p', '1080p', '4k')
CAVLC = ('5185721dda11', '6f7799768413', '9db41aa16b5a', 'b71b6b31e54f', 'ec473c99c538', 'f9e973f44570')


def seconds(row, arm, mode):
    s = row.get('seconds') or {}
    return next((v for k, v in s.items() if k.split('@')[0] == f'{arm}_{mode}'), None)


def ratio_ff(row, arm, mode):
    r = row.get('ratio') or {}
    return next((v for k, v in r.items() if k.split('@')[0] == f'{arm}:{mode}'), None)


def load(d, prefix):
    rows = []
    for c in classes:
        p = f'{d}/{prefix}.{c}.json'
        if os.path.exists(p):
            rows += json.load(open(p))['streams']
    return rows


def ci(v, seed=20261026, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(rows, mode):
    v, rb, rk = [], [], []
    for x in rows:
        a, b = seconds(x, 'k', mode), seconds(x, 'base', mode)
        if a and b:
            v.append(a / b); rb.append(ratio_ff(x, 'base', mode)); rk.append(ratio_ff(x, 'k', mode))
    if not v:
        return None
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci': (lo, hi), 'faster': sum(t < 1 for t in v),
            'ff_base': statistics.median(rb), 'ff_k': statistics.median(rk),
            'ffster_base': sum(t < 1 for t in rb), 'ffster_k': sum(t < 1 for t in rk)}


def table(rows):
    res = {}
    for m in ('discard', 'write'):
        r = {'all': summary(rows, m)}
        for c in classes:
            r[c] = summary([x for x in rows if x['class'] == c], m)
        r['cavlc'] = summary([x for x in rows if x['stream'][:12] in CAVLC], m)
        res[m] = r
    return res


def show(name, t):
    for m in ('discard', 'write'):
        for k, v in t[m].items():
            if v:
                print(f"{name:3} {m:7} {k:6} n={v['n']:2} k/base {v['median']:.4f} [{v['ci'][0]:.4f}, {v['ci'][1]:.4f}] faster {v['faster']}/{v['n']}"
                      f" | vs FFmpeg base {v['ff_base']:.3f} ({v['ffster_base']}/{v['n']}) k {v['ff_k']:.3f} ({v['ffster_k']}/{v['n']})")


def unresolved(rows):
    return [x['stream'][:12] for x in rows if x.get('unresolved')]


d1 = sys.argv[1]
d8 = sys.argv[2] if len(sys.argv) > 2 else None
d8h = sys.argv[3] if len(sys.argv) > 3 else None
r1 = load(d1, 'g'); t1 = table(r1); show('1T', t1)
bars = {}
g = lambda t, m, k: t[m][k]
bars['B2-1'] = g(t1, 'discard', 'all')['n'] == 32 and g(t1, 'discard', 'all')['median'] <= 0.995 and g(t1, 'discard', 'all')['ci'][1] < 1.000
bars['B2-3'] = g(t1, 'discard', 'cavlc')['n'] == 6 and g(t1, 'discard', 'cavlc')['median'] <= 0.985
ok4 = g(t1, 'write', 'all')['median'] <= 1.000
ok5 = all(t1[m][c] and t1[m][c]['median'] <= 1.005 for m in ('discard', 'write') for c in classes)
print('unresolved 1T', unresolved(r1))
if d8:
    r8 = load(d8, 't8'); t8 = table(r8); show('8T', t8); print('unresolved 8T', unresolved(r8))
    bars['B2-2'] = g(t8, 'discard', 'all')['n'] == 32 and g(t8, 'discard', 'all')['median'] <= 0.990 and g(t8, 'discard', 'all')['ci'][1] < 1.000
    ok4 = ok4 and g(t8, 'write', 'all')['median'] <= 1.000
    ok5 = ok5 and all(t8[m][c] and t8[m][c]['median'] <= 1.005 for m in ('discard', 'write') for c in classes)
if d8h:
    rh = load(d8h, 't8'); th = table(rh); show('8Th', th); print('unresolved 8T held-out', unresolved(rh))
    bars['B2-7'] = g(th, 'discard', 'all')['n'] == 64 and g(th, 'discard', 'all')['median'] <= 1.000
bars['B2-4'] = ok4
bars['B2-5'] = ok5
print('bars', dict(sorted(bars.items())), '(B2-2, B2-7 need DIR8, DIR8H; B2-4/5 read over the calls given)')
