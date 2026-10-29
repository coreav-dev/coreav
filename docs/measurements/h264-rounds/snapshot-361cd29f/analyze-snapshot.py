"""python3 -I analyze-snapshot.py DIR -> the official snapshot: CoreAV's per-stream ratio to FFmpeg 9.0.2 (ours over FFmpeg;
above 1 is slower) on one build `main`, reports of scripts/h264_parity.py with the decoder `main` and its null copy:
g.<class>.json (32 training streams, one thread pinned, helper VM), h.<class>.json (64 held-out, one thread pinned, helper VM),
t8.<class>.json (32 training, eight threads, Compute Engine) and h8.<class>.json (64 held-out, eight threads, Compute Engine).
Per call present, discard and write, overall and per class: the median per-stream ratio, its 95% bootstrap interval
(2,000 resamples, random.Random(20261024)) and the share of streams where ours is faster (ratio below 1); the null copy of
`main` over `main` (median per-stream ratio, the noise of the median); the harness's host-fault records."""
import json, os, random, statistics, sys

d = sys.argv[1]
classes = ('sd', '720p', '1080p', '4k')
groups = ('overall',) + classes
CALLS = (('g', '1 thread, 32 training (helper VM)'), ('h', '1 thread, 64 held-out (helper VM)'),
         ('t8', '8 threads, 32 training (Compute Engine)'), ('h8', '8 threads, 64 held-out (Compute Engine)'))


def ci(v, seed=20261024, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


out = {}
for pre, label in CALLS:
    rows, method = [], {}
    for c in classes:
        p = f'{d}/{pre}.{c}.json'
        if not os.path.exists(p):
            rows = None
            break
        rep = json.load(open(p))
        rows += rep['streams']
        m = rep.get('method', {})
        method[c] = {k: m.get(k) for k in ('unresolved', 'retried')}
    if rows is None:
        continue
    print(f'== {pre}: {label}: {len(rows)} streams')
    res = {}
    for mode in ('discard', 'write'):
        for lab in groups:
            sel = [x for x in rows if lab == 'overall' or x['class'] == lab]
            v = [x['ratio'][f'main:{mode}'] for x in sel]
            lo, hi = ci(v)
            faster = sum(1 for t in v if t < 1)
            res[f'{mode}.{lab}'] = {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi], 'faster': faster}
            print(f'  {mode:7} {lab:8} n={len(v):3} median {statistics.median(v):.3f} [{lo:.3f}, {hi:.3f}] faster {faster}/{len(v)} ({100 * faster / len(v):.0f}%)')
    nulls = {}
    for mode in ('discard', 'write'):
        v = [statistics.median(x['rounds'][f'main~null_{mode}']) / x['seconds'][f'main_{mode}'] for x in rows
             if f'main~null_{mode}' in x.get('rounds', {})]
        if v:
            lo, hi = ci(v)
            nulls[mode] = {'median': statistics.median(v), 'ci95': [lo, hi]}
            print(f'  null copy over main, {mode}: median {statistics.median(v):.4f} [{lo:.4f}, {hi:.4f}] (n={len(v)})')
    unres = sum(m['unresolved'] or 0 for m in method.values())
    retried = sum(m['retried'] or 0 for m in method.values())
    print(f'  host faults: {retried} streams timed again, {unres} unresolved')
    out[pre] = {'label': label, 'results': res, 'null': nulls, 'unresolved': unres, 'retried': retried}
json.dump(out, open(f'{d}/summary.json', 'w'), indent=1)
