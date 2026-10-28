"""python3 -I analyze.py DIR -> the Intra_4x4 and level-scale round's ratios between two builds timed in the same rounds.

DIR holds g.{sd,720p,1080p,4k}.json (one thread, pinned) and t8.{sd,720p,1080p,4k}.json (eight
threads, unpinned) from scripts/h264_parity.py with the decoders `base` (`main`) and `k` (the
branch's code commit), built alike. For k over base, each pairing
(discard, write), overall and per class: the median per-stream ratio with a 95% bootstrap interval
(2,000 resamples, random.Random(20261024)) and how many streams are faster. Then bars IS-1 to IS-4 of
docs/design/h264.md ("Kernel round: Intra_4x4 edges in vectors, and level scales built once"), each build's median ratio to FFmpeg and the
streams faster than it, overall and per class, as information, and each stream's ratio. IS-5
(output) is read from identity.txt and the conformance report, not here.
"""
import json, random, statistics, sys

d = sys.argv[1]
classes = ('sd', '720p', '1080p', '4k')
groups = ('overall',) + classes
builds = ('base', 'k')


def member(x, lab):
    return lab == 'overall' or x['class'] == lab


def load(pre):
    rows = []
    for c in classes:
        rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
    ok = [x for x in rows if x.get('seconds')
          and all(f'{b}_{p}' in x['seconds'] for b in builds for p in ('discard', 'write'))]
    return rows, ok


def ci(v, seed=20261024, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(p, sel):
    v = [x['seconds'][f'k_{p}'] / x['seconds'][f'base_{p}'] for x in sel]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
            'faster': sum(1 for t in v if t < 1)}


out = {}
for pre, label in (('g', '1 thread'), ('t8', '8 threads')):
    rows, ok = load(pre)
    res = {'failed': [x['stream'] for x in rows if x not in ok]}
    for p in ('discard', 'write'):
        r = {lab: summary(p, [x for x in ok if member(x, lab)]) for lab in groups}
        r['per_stream'] = {x['stream']: x['seconds'][f'k_{p}'] / x['seconds'][f'base_{p}'] for x in ok}
        res[p] = r
    res['vs_ffmpeg'] = {}
    for b in builds:
        res['vs_ffmpeg'][b] = {}
        for p in ('discard', 'write'):
            r = {}
            for lab in groups:
                v = [x['ratio'][f'{b}:{p}'] for x in ok if member(x, lab)]
                r[lab] = {'n': len(v), 'median': statistics.median(v), 'faster': sum(1 for t in v if t < 1)}
            res['vs_ffmpeg'][b][p] = r
    res['rows'] = ok
    out[pre] = res
g, t = out['g'], out['t8']
bars = {
    'IS-1': g['discard']['overall']['ci95'][1] < 1.0 and g['discard']['overall']['median'] <= 1.0,
    'IS-2': g['write']['overall']['median'] <= 1.0,
    'IS-3': all(g[p][c]['median'] <= 1.005 for p in ('discard', 'write') for c in classes),
    'IS-4': all(t[p]['overall']['median'] <= 1.005 for p in ('discard', 'write')),
}
summ = {k: {kk: vv for kk, vv in v.items() if kk != 'rows'} for k, v in out.items()}
summ['bars'] = bars
json.dump(summ, open(f'{d}/summary.json', 'w'), indent=1)
for pre, label in (('g', '1 thread'), ('t8', '8 threads')):
    res = out[pre]
    print(label, 'failed', res['failed'])
    for p in ('discard', 'write'):
        for lab in groups:
            v = res[p][lab]
            print(f"  k/base {p:7} {lab:8} n={v['n']:2} median {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
    for b, pairs in res['vs_ffmpeg'].items():
        for p, r in pairs.items():
            print(f"  {b:4} vs FFmpeg {p:7} " + ' '.join(
                f"{lab} {v['median']:.3f} ({v['faster']}/{v['n']} faster)" for lab, v in r.items()))
print('bars (k over base; IS-5 is read from identity.txt and the conformance report):',
      ', '.join(f"{k} {'pass' if v else 'FAIL'}" for k, v in bars.items()))
print('per stream, k over base (1 thread discard, write; 8 threads discard, write):')
t8 = {x['stream']: x for x in t['rows']}
for x in g['rows']:
    s = x['stream']
    y = t8.get(s)
    print(f"  {s} {x['class']:5} {g['discard']['per_stream'][s]:.4f} {g['write']['per_stream'][s]:.4f}"
          + (f" {t['discard']['per_stream'][s]:.4f} {t['write']['per_stream'][s]:.4f}" if y else ''))
