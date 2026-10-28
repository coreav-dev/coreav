"""python3 -I analyze-cw.py DIR1 [DIR8]: the bars SL-1 to SL-5 of #540 (docs/design/h264.md, 'CAVLC slice loop', Bars)
from the calls' reports: g.<class>.json in DIR1 (one thread pinned, 32 training streams, arms `base` and `k`), t8.<class>.json in DIR8 (eight threads, the
32 training streams, Compute Engine) and in h.<class>.json in DIR1 (one thread pinned, the 64 held-out streams). Per call and mode: the median per-stream ratio `k` over `base` with a
bootstrap 95% interval (2,000 resamples over streams), over all streams, per class and over the six CAVLC streams, and the ratios to FFmpeg with the
share of streams faster than it. SL-6 (output) is read from identity and conformance files, not here."""
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


HELD_CAVLC = ('176aa2655e6b', '178de4a3a5a2', '280136b1ab5c', '391ec79e178a', '75cf7bf8391b', '8754f59f19d5', '985a28c5abe5', 'ac8f060d2280',
              'd4d0cacfe7ff', 'd56594fb1f83', 'd67bb9087d1a', 'ea3b4d48a931', 'eb872a518732', 'ee0d6661e357', 'ff84167e699d')  # PPS entropy_coding_mode_flag 0 (pps_entropy.py)
d1 = sys.argv[1]
d8 = sys.argv[2] if len(sys.argv) > 2 else None
g = lambda t, m, k: t[m][k]
r1 = load(d1, 'g'); t1 = table(r1); show('1T', t1)
rh = load(d1, 'h'); th = table(rh); show('1Th', th)
hc = [x for x in rh if x['stream'][:12] in HELD_CAVLC]
for m in ('discard', 'write'):
    v = summary(hc, m)
    if v:
        print(f"1Th {m:7} cavlc15 n={v['n']} k/base {v['median']:.4f} [{v['ci'][0]:.4f}, {v['ci'][1]:.4f}] faster {v['faster']}/{v['n']} (information)")
f6 = [x for x in r1 if x['stream'][:12] == '6f7799768413']
for m in ('discard', 'write'):
    v = summary(f6, m)
    if v:
        print(f"1T  {m:7} 6f77   k/base {v['median']:.4f} | vs FFmpeg base {v['ff_base']:.3f} k {v['ff_k']:.3f} (information)")
print('unresolved 1T', unresolved(r1), 'held-out', unresolved(rh))
bars = {}
bars['SL-1'] = g(t1, 'discard', 'cavlc')['n'] == 6 and g(t1, 'discard', 'cavlc')['median'] <= 0.990 and g(t1, 'discard', 'cavlc')['ci'][1] < 1.000
bars['SL-2'] = g(t1, 'discard', 'all')['n'] == 32 and g(t1, 'discard', 'all')['median'] <= 1.000 and g(t1, 'write', 'all')['median'] <= 1.000
bars['SL-3'] = all(t1[m][c] and t1[m][c]['median'] <= 1.005 for m in ('discard', 'write') for c in classes)
bars['SL-5'] = g(th, 'discard', 'all')['n'] == 64 and g(th, 'discard', 'all')['median'] <= 1.000
if d8:
    r8 = load(d8, 't8'); t8 = table(r8); show('8T', t8); print('unresolved 8T', unresolved(r8))
    bars['SL-4'] = g(t8, 'discard', 'all')['n'] == 32 and g(t8, 'discard', 'all')['median'] <= 1.005 and all(t8['discard'][c]['median'] <= 1.010 for c in classes)
print('bars', dict(sorted(bars.items())), '(SL-4 needs DIR8)')
