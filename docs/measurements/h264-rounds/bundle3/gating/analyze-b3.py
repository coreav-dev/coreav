"""python3 -I analyze-b3.py DIR -> bundle 3's (#546) bars B3-1 to B3-7 (docs/measurements/h264-rounds/bundle3/plan.md, "Bars"). DIR holds t8.<class>.json (eight threads, c4-standard-16, the 32
training streams), g.<class>.json (one thread pinned, helper VM, the 32 training streams) and h1.<class>.json (one thread pinned, the 64 held-out streams, five rounds), from scripts/h264_parity*.py with decoders `base` and `k` (= the bundle), built
alike. Each ratio is the median over streams of the per-stream ratio ft/base with a 95% bootstrap interval over streams (2,000
resamples, random.Random(20261027)). Missing sets are printed as pending. B3-8 (output) is read from identity.txt and the
conformance report, not here. Information: the ratio to FFmpeg 9.0.2 and the streams faster than it, and the harness's host-fault
records."""
import json, os, random, statistics, sys

d = sys.argv[1]
classes = ('sd', '720p', '1080p', '4k')
groups = ('overall',) + classes


def load(pre):
    if not all(os.path.exists(f'{d}/{pre}.{c}.json') for c in classes):
        return None
    rows, meth = [], {}
    for c in classes:
        rep = json.load(open(f'{d}/{pre}.{c}.json'))
        rows += rep['streams']
        m = rep.get('method', {})
        meth[f'{pre}.{c}'] = (m.get('unresolved'), m.get('retried'), m.get('gate_drift_streams'),
                              max((abs(v) for x in rep['streams'] for v in x.get('null', {}).values()), default=None))
    ok = [x for x in rows if x.get('seconds') and all(f'{b}_{p}' in x['seconds'] for b in ('base', 'k') for p in ('discard', 'write'))]
    return ok, meth, len(rows)


def ci(v, n=2000):
    rng = random.Random(20261027)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summ(p, sel):
    v = [x['seconds'][f'k_{p}'] / x['seconds'][f'base_{p}'] for x in sel]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'lo': lo, 'hi': hi, 'faster': sum(t < 1 for t in v)}


R = {}
for pre, label in (('t8', '8 threads, training 32 (c4-standard-16)'), ('g', '1 thread pinned, training 32 (helper VM)'),
                   ('h1', '1 thread pinned, held-out 64 (helper VM)')):
    r = load(pre)
    if not r:
        print(label, ': pending'); continue
    ok, meth, nrows = r
    print(f'== {label}: {len(ok)} of {nrows} streams')
    res = {}
    for p in ('discard', 'write'):
        res[p] = {lab: summ(p, [x for x in ok if lab == 'overall' or x['class'] == lab]) for lab in groups}
        for lab in groups:
            v = res[p][lab]
            print(f"  ft/base {p:7} {lab:8} n={v['n']:2} median {v['median']:.4f} [{v['lo']:.4f},{v['hi']:.4f}] faster {v['faster']}/{v['n']}")
    for b in ('base', 'k'):
        for p in ('discard', 'write'):
            print(f"  {'ft' if b == 'k' else b:4} vs FFmpeg {p:7} " + ' '.join(
                f"{lab} {statistics.median([x['ratio'][f'{b}:{p}'] for x in ok if lab == 'overall' or x['class'] == lab]):.3f}"
                f" ({sum(x['ratio'][f'{b}:{p}'] < 1 for x in ok if lab == 'overall' or x['class'] == lab)} faster)" for lab in groups))
    for k, v in meth.items():
        print(f'  {k}: unresolved {v[0]}, retried {v[1]}, gate drift {v[2]}, largest null deviation {v[3]}')
    R[pre] = res
b = {}
rows1 = None
if 'g' in R:
    g = R['g']
    b['B3-1'] = g['discard']['overall']['median'] <= 0.990 and g['discard']['overall']['hi'] < 1.0
    big = [x for x in load('g')[0] if x['class'] in ('1080p', '4k')]
    v = summ('discard', big)
    print(f"  B3-2 the 16 streams at 1080p and 4k, 1T discard: median {v['median']:.4f} [{v['lo']:.4f},{v['hi']:.4f}] faster {v['faster']}/{v['n']}")
    b['B3-2'] = v['median'] <= 0.985
    b['B3-3'] = g['write']['overall']['median'] <= 1.0
    b['B3-4'] = all(g[p][c]['median'] <= 1.005 for p in g for c in classes)
if 't8' in R:
    t = R['t8']
    b['B3-5'] = all(t[p]['overall']['median'] <= 1.005 for p in t)
    b['B3-6'] = all(t[p][c]['median'] <= 1.010 for p in t for c in classes)
if 'h1' in R:
    b['B3-7'] = R['h1']['discard']['overall']['median'] <= 1.0
print('bars:', ', '.join(f"{k} {'pass' if v else 'FAIL'}" for k, v in b.items()), '(B3-8 from identity.txt, conformance.txt)')
json.dump({'bars': b, 'results': R}, open(f'{d}/summary.json', 'w'), indent=1)
