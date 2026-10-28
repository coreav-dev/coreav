"""python3 -I analyze.py DIR PREFIX1 PREFIX8 -> the reference-hint round's after/before per-stream ratios
(`before` is main, `after` the reference's lines requested ahead, cheaply): the one-thread calls (PREFIX1,
pinned) and the eight-thread calls (PREFIX8, unpinned), discard and write, with a bootstrap 95% interval of
the median, overall, per class and for the 1080p and 4k classes together; the bars MA-1 to MA-5 of
docs/design/h264.md ("Motion round: the reference's lines requested ahead, cheaply"); and the ratios to
FFmpeg with the share of streams faster than it. MA-6 (output) is read from identity.txt and the
conformance report, not here. A stream either arm failed is listed and left out; the bars need n = 32."""
import json, random, statistics, sys

d, pre1, pre8 = sys.argv[1], sys.argv[2], sys.argv[3]
classes = ('sd', '720p', '1080p', '4k')
groups = ('overall',) + classes + ('large',)


def load(prefix):
    rows = []
    for c in classes:
        rows += json.load(open(f'{d}/{prefix}.{c}.json'))['streams']
    ok = [x for x in rows if x.get('seconds') and all(f'{a}_{p}' in x['seconds']
          for a in ('before', 'after') for p in ('discard', 'write'))]
    return rows, ok


def ci(v, seed=20261024, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def member(x, g):
    return g == 'overall' or x['class'] == g or (g == 'large' and x['class'] in ('1080p', '4k'))


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
        r = {g: summary([x for x in ok if member(x, g)], p) for g in groups}
        r['per_stream'] = {x['stream']: x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}'] for x in ok}
        res[p] = r
    return res


(r1, ok1), (r8, ok8) = load(pre1), load(pre8)
t1, t8 = table(ok1), table(ok8)
out = {'failed': {pre1: [x['stream'] for x in r1 if x not in ok1], pre8: [x['stream'] for x in r8 if x not in ok8]},
       'threads1': t1, 'threads8': t8}
full = len(ok1) == 32 and len(ok8) == 32
out['bars'] = {
    'MA-1': full and t1['discard']['overall']['median'] <= 1.000 and t1['discard']['overall']['ci95'][1] < 1.005,
    'MA-2': full and t1['write']['overall']['median'] <= 1.000 and t1['write']['overall']['ci95'][1] < 1.005,
    'MA-3': full and t1['discard']['4k']['median'] <= 0.975 and t1['discard']['large']['median'] <= 0.985
    and t1['discard']['large']['ci95'][1] < 1.000,
    'MA-4': full and all(t1[p][k]['median'] <= 1.005 for p in ('discard', 'write') for k in classes),
    'MA-5': full and t8['discard']['overall']['median'] <= 1.000
    and all(t8['discard'][k]['median'] <= 1.010 for k in classes),
}
json.dump(out, open(f'{d}/{pre1}.summary.json', 'w'), indent=1)
print('failed', out['failed'])
for name, t in (('1T', t1), ('8T', t8)):
    for p in ('discard', 'write'):
        for k, v in t[p].items():
            if k == 'per_stream':
                continue
            print(f"{name} {p:7} {k:8} n={v['n']:2} after/before {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster_after']}/{v['n']} | vs ffmpeg "
                  f"before {v['before_vs_ffmpeg']:.3f} after {v['after_vs_ffmpeg']:.3f}, faster than ffmpeg "
                  f"{v['faster_than_ffmpeg_before']}/{v['n']} then {v['faster_than_ffmpeg_after']}/{v['n']}")
print('bars:', out['bars'])
print('per stream (1T discard, 1T write, 8T discard, 8T write):')
for s_ in sorted(t1['discard']['per_stream']):
    g = lambda t, p: t[p]['per_stream'].get(s_, float('nan'))
    print(f"  {s_} {g(t1, 'discard'):.4f} {g(t1, 'write'):.4f} {g(t8, 'discard'):.4f} {g(t8, 'write'):.4f}")
