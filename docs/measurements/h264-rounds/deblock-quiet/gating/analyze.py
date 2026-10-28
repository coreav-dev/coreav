"""python3 -I analyze.py DIR: the bars QZ-1 to QZ-6 of docs/design/h264.md ("Deblocking round: macroblocks below the thresholds")
from the gating calls' reports in DIR: g.<class>.json (one
thread pinned, the 32 training streams), t8.<class>.json (eight threads) and h.<class>.json (one thread pinned, the
64 held-out streams), each a report of scripts/h264_parity.py with the decoders `base` and `k`. QZ-7 (output) is read
from the identity and conformance files, not here. Per call and mode: the median per-stream ratio `k` over `base`
with a bootstrap 95% interval, over all streams, the target streams T, the rest R and each class, and the ratios to
FFmpeg with the share of streams faster than it."""
import json, os, random, statistics, sys

d = sys.argv[1]
classes = ('sd', '720p', '1080p', '4k')
# The target streams, fixed before any timing: 1.5% or more fewer instructions at the class frame counts.
T = {'81ed4778ece2', 'ed75c044e944', 'ae16bb7c7192', 'e624cbe0d2be', '4b54de28c2b3', 'f8d92a911e72',
     '6e90422a670a', 'a6af290d9053', '3b1b441e23ae', '36501d448eaa'}


def seconds(row, arm, mode):
    s = row.get('seconds') or {}
    return next((v for k, v in s.items() if k.split('@')[0] == f'{arm}_{mode}'), None)


def ratio_ff(row, arm, mode):
    r = row.get('ratio') or {}
    return next((v for k, v in r.items() if k.split('@')[0] == f'{arm}:{mode}'), None)


def load(prefix):
    rows = []
    for c in classes:
        p = f'{d}/{prefix}.{c}.json'
        if os.path.exists(p):
            rows += json.load(open(p))['streams']
    return rows


def ci(v, seed=20261026, n=4000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(rows, mode):
    v, rb, rk = [], [], []
    for x in rows:
        a, b = seconds(x, 'k', mode), seconds(x, 'base', mode)
        if a and b:
            v.append(a / b)
            rb.append(ratio_ff(x, 'base', mode))
            rk.append(ratio_ff(x, 'k', mode))
    if not v:
        return None
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi], 'faster': sum(t < 1 for t in v),
            'base_vs_ffmpeg': statistics.median(rb), 'k_vs_ffmpeg': statistics.median(rk),
            'faster_than_ffmpeg_base': sum(t < 1 for t in rb), 'faster_than_ffmpeg_k': sum(t < 1 for t in rk)}


out, unresolved = {}, {}
for call in ('g', 't8', 'h'):
    rows = load(call)
    unresolved[call] = [x['stream'] for x in rows if x.get('unresolved')]
    sets = {'all': rows, 'T': [x for x in rows if x['stream'][:12] in T],
            'R': [x for x in rows if x['stream'][:12] not in T]}
    sets.update({c: [x for x in rows if x['class'] == c] for c in classes})
    out[call] = {mode: {lab: summary(sel, mode) for lab, sel in sets.items()} for mode in ('discard', 'write')}
    out[call]['per_stream'] = {x['stream']: {m: (seconds(x, 'k', m) / seconds(x, 'base', m)
                                                 if seconds(x, 'k', m) and seconds(x, 'base', m) else None)
                                             for m in ('discard', 'write')} for x in rows}
    for mode in ('discard', 'write'):
        for lab, r in out[call][mode].items():
            if r:
                print(f"{call:3} {mode:7} {lab:5} n={r['n']:2} k/base {r['median']:.4f} [{r['ci95'][0]:.4f}, "
                      f"{r['ci95'][1]:.4f}] faster {r['faster']}/{r['n']} | vs FFmpeg base {r['base_vs_ffmpeg']:.3f} "
                      f"({r['faster_than_ffmpeg_base']}/{r['n']}) k {r['k_vs_ffmpeg']:.3f} ({r['faster_than_ffmpeg_k']}/{r['n']})")


def get(call, mode, lab, key='median'):
    r = (out.get(call) or {}).get(mode, {}).get(lab)
    return None if r is None else r[key]


def le(x, bar):
    return x is not None and x <= bar


bars = {
    'QZ-1': le(get('g', 'discard', 'T'), 0.975) and (get('g', 'discard', 'T', 'ci95') or [9, 9])[1] < 1.0,
    'QZ-2': le(get('g', 'write', 'T'), 0.985),
    'QZ-3': le(get('g', 'discard', 'R'), 1.003),
    'QZ-4': all(le(get('g', m, 'all'), 1.000) for m in ('discard', 'write'))
            and all(le(get('g', m, c), 1.005) for m in ('discard', 'write') for c in classes),
    'QZ-5': le(get('t8', 'discard', 'all'), 1.000) and le(get('t8', 'discard', 'T'), 1.000)
            and all(le(get('t8', 'discard', c), 1.010) for c in classes),
    'QZ-6': le(get('h', 'discard', 'all'), 1.000),
}
print('unresolved', unresolved)
print('bars', bars)
json.dump({'calls': out, 'unresolved': unresolved, 'bars': bars}, open(f'{d}/summary.json', 'w'), indent=1)
