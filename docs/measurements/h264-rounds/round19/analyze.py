"""python3 -I analyze.py DIR PREFIX1 PREFIX8 -> round 19's after/before per-stream ratios (`before` is
main, `after` the CAVLC residual in one pass): the one-thread calls (PREFIX1, pinned) and the
eight-thread calls (PREFIX8, unpinned), discard and write, with a bootstrap 95% interval of the median,
over all 32 streams, per class and over the six CAVLC streams; the bars V-1 to V-5 of
docs/design/h264.md ("Round 19"); and the ratios to FFmpeg with the share of streams faster than it.
V-6 (output) is read from identity.txt and the conformance report, not here."""
import json, random, statistics, sys

d, pre1, pre8 = sys.argv[1], sys.argv[2], sys.argv[3]
classes = ('sd', '720p', '1080p', '4k')
# The training set's CAVLC streams (features.tsv of docs/measurements/h264-rounds/snapshot-outliers).
CAVLC = ('5185721dda11', '6f7799768413', '9db41aa16b5a', 'b71b6b31e54f', 'ec473c99c538', 'f9e973f44570')
OUTLIER = '6f7799768413'


def load(prefix):
    rows = []
    for c in classes:
        rows += json.load(open(f'{d}/{prefix}.{c}.json'))['streams']
    ok = [x for x in rows if x.get('seconds') and 'before_discard' in x['seconds']
          and 'after_discard' in x['seconds']]
    return rows, ok


def ci(v, seed=20261024, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(sel, p):
    v = [x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}'] for x in sel]
    rb = [x['ratio'][f'before:{p}'] for x in sel]
    ra = [x['ratio'][f'after:{p}'] for x in sel]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
            'faster_after': sum(1 for t in v if t < 1),
            'before_vs_ffmpeg': statistics.median(rb), 'after_vs_ffmpeg': statistics.median(ra),
            'faster_than_ffmpeg_before': sum(1 for t in rb if t < 1),
            'faster_than_ffmpeg_after': sum(1 for t in ra if t < 1)}


def table(ok):
    res = {}
    for p in ('discard', 'write'):
        r = {lab: summary([x for x in ok if lab == 'overall' or x['class'] == lab], p)
             for lab in ('overall',) + classes}
        r['cavlc'] = summary([x for x in ok if x['stream'][:12] in CAVLC], p)
        r['per_stream'] = {x['stream']: x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}']
                           for x in ok}
        r['vs_ffmpeg'] = {x['stream']: (x['ratio'][f'before:{p}'], x['ratio'][f'after:{p}']) for x in ok}
        res[p] = r
    return res


t1, t8 = (table(load(p)[1]) for p in (pre1, pre8))
outlier = next((v for k, v in t1['discard']['per_stream'].items() if k.startswith(OUTLIER)), float('nan'))
out = {'failed': {p: [x['stream'] for x in load(p)[0] if x not in load(p)[1]] for p in (pre1, pre8)},
       'threads1': t1, 'threads8': t8, 'outlier': outlier}
out['bars'] = {
    'V-1': t1['discard']['cavlc']['median'] <= 0.975 and t1['discard']['cavlc']['ci95'][1] < 1.000,
    'V-2': outlier <= 0.920,
    'V-3': t1['discard']['overall']['median'] <= 1.005
    and all(t1['discard'][k]['median'] <= 1.005 for k in classes),
    'V-4': t1['write']['overall']['median'] <= 1.005
    and all(t1['write'][k]['median'] <= 1.005 for k in classes),
    'V-5': t8['discard']['overall']['median'] <= 1.005
    and all(t8['discard'][k]['median'] <= 1.005 for k in classes),
}
json.dump(out, open(f'{d}/{pre1}.summary.json', 'w'), indent=1)
print('failed', out['failed'])
for name, t in (('1T', t1), ('8T', t8)):
    for p in ('discard', 'write'):
        for k, v in t[p].items():
            if k in ('per_stream', 'vs_ffmpeg'):
                continue
            print(f"{name} {p:7} {k:8} n={v['n']:2} after/before {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster_after']}/{v['n']} | vs ffmpeg "
                  f"before {v['before_vs_ffmpeg']:.3f} after {v['after_vs_ffmpeg']:.3f}, faster than ffmpeg "
                  f"{v['faster_than_ffmpeg_before']}/{v['n']} then {v['faster_than_ffmpeg_after']}/{v['n']}")
print(f'{OUTLIER} 1T discard after/before {outlier:.4f}')
print('bars:', out['bars'])
print('per stream (1T discard, 1T write, 8T discard; 1T discard vs ffmpeg before -> after):')
for s_ in sorted(t1['discard']['per_stream']):
    g = lambda t, p: t[p]['per_stream'].get(s_, float('nan'))
    fb, fa = t1['discard']['vs_ffmpeg'][s_]
    tag = ' CAVLC' if s_[:12] in CAVLC else ''
    print(f"  {s_} {g(t1, 'discard'):.4f} {g(t1, 'write'):.4f} {g(t8, 'discard'):.4f}  {fb:.3f} -> {fa:.3f}{tag}")
