"""python3 -I analyze.py DIR -> the residual-dispatch round's ratios between two builds timed in the same rounds.

DIR holds g.<class>.json (one thread pinned; decoders `base` (main e2271b3a), `k` (the branch's code commit) and
`knofast` (k built with --no-default-features)), t8.<class>.json (eight threads, unpinned, calls of at least 3 s; `base`,
`k`) and h.<class>.json (one thread pinned, the 64 held-out streams; `base`, `k`) from scripts/h264_parity.py. For k over
base, each pairing (discard, write), overall and per class: the median per-stream ratio with a 95% bootstrap interval (2,000
resamples, random.Random(20261024)) and how many streams are faster. Then bars RD-1 to RD-5 of docs/design/h264.md ("Entropy
round: the residual's block dispatch"); knofast over k at one thread (information, ADR 0038 point 4); each build's median ratio
to FFmpeg and the streams faster than it as information; the harness's host-fault records (null-arm deviation, retried and
unresolved streams, in-call gates), the unresolved streams listed (they stay in the medians). RD-6 (output) is read from
identity.txt and the conformance report, not here."""
import json, random, statistics, sys

d = sys.argv[1]
classes = ('sd', '720p', '1080p', '4k')
groups = ('overall',) + classes
method = {}
unresolved_list = {}


def member(x, lab):
    return lab == 'overall' or x['class'] == lab


def load(pre, builds):
    rows = []
    for c in classes:
        rep = json.load(open(f'{d}/{pre}.{c}.json'))
        rows += rep['streams']
        m = rep.get('method', {})
        method[f'{pre}.{c}'] = {k: m.get(k) for k in ('unresolved', 'retried', 'gate_drift_streams')}
        method[f'{pre}.{c}']['null_max'] = max((abs(v) for x in rep['streams'] for v in x.get('null', {}).values()),
                                               default=None)
        unresolved_list[f'{pre}.{c}'] = [x['stream'] for x in rep['streams'] if x.get('unresolved')]
    ok = [x for x in rows if x.get('seconds') and all(f'{b}_{p}' in x['seconds'] for b in builds for p in ('discard', 'write'))]
    return rows, ok


def ci(v, seed=20261024, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(a, b, p, sel):
    v = [x['seconds'][f'{a}_{p}'] / x['seconds'][f'{b}_{p}'] for x in sel]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi], 'faster': sum(1 for t in v if t < 1)}


out = {}
for pre, builds in (('g', ('base', 'k', 'knofast')), ('t8', ('base', 'k')), ('h', ('base', 'k'))):
    rows, ok = load(pre, builds)
    res = {'failed': [x['stream'] for x in rows if x not in ok]}
    for p in ('discard', 'write'):
        res[p] = {lab: summary('k', 'base', p, [x for x in ok if member(x, lab)]) for lab in groups}
    res['vs_ffmpeg'] = {}
    for b in builds:
        res['vs_ffmpeg'][b] = {}
        for p in ('discard', 'write'):
            r = {}
            for lab in groups:
                v = [x['ratio'][f'{b}:{p}'] for x in ok if member(x, lab)]
                r[lab] = {'n': len(v), 'median': statistics.median(v), 'faster': sum(1 for t in v if t < 1)}
            res['vs_ffmpeg'][b][p] = r
    if 'knofast' in builds:
        res['knofast_over_k'] = {p: {lab: summary('knofast', 'k', p, [x for x in ok if member(x, lab)])
                                     for lab in groups} for p in ('discard', 'write')}
    res['rows'] = ok
    out[pre] = res
g, t, h = out['g'], out['t8'], out['h']
bars = {
    'RD-1': g['discard']['overall']['median'] <= 0.995 and g['discard']['overall']['ci95'][1] < 1.0,
    'RD-2': g['write']['overall']['median'] <= 1.000,
    'RD-3': all(g[p][c]['median'] <= 1.005 for p in ('discard', 'write') for c in classes),
    'RD-4': t['discard']['overall']['median'] <= 1.000 and all(t['discard'][c]['median'] <= 1.010 for c in classes),
    'RD-5': h['discard']['overall']['n'] == 64 and h['discard']['overall']['median'] <= 1.000,
}
summ = {k: {kk: vv for kk, vv in v.items() if kk != 'rows'} for k, v in out.items()}
summ['bars'] = bars
summ['method'] = method
summ['unresolved'] = unresolved_list
json.dump(summ, open(f'{d}/summary.json', 'w'), indent=1)
for pre, label in (('g', '1 thread, 32 training'), ('t8', '8 threads, 32 training'), ('h', '1 thread, 64 held-out')):
    res = out[pre]
    print(label, 'failed', res['failed'])
    for p in ('discard', 'write'):
        for lab in groups:
            v = res[p][lab]
            print(f"  k/base {p:7} {lab:8} n={v['n']:2} median {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
    for b, pairs in res['vs_ffmpeg'].items():
        for p, r in pairs.items():
            print(f"  {b:7} vs FFmpeg {p:7} " + ' '.join(
                f"{lab} {v['median']:.3f} ({v['faster']}/{v['n']} faster)" for lab, v in r.items()))
    if 'knofast_over_k' in res:
        for p, r in res['knofast_over_k'].items():
            print(f"  knofast/k {p:7} " + ' '.join(f"{lab} {v['median']:.4f} [{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}]"
                                                   for lab, v in r.items()))
for k, v in method.items():
    print(f"  {k}: unresolved {v['unresolved']} {unresolved_list[k]}, retried {v['retried']}, gate drift {v['gate_drift_streams']}, "
          f"largest null deviation {v['null_max']}")
print('bars (k over base; RD-6 is read from identity.txt and the conformance report):',
      ', '.join(f"{k} {'pass' if v else 'FAIL'}" for k, v in bars.items()))
