"""python3 -I analyze.py DIR PREFIX1 PREFIXM PREFIX8 -> the motion round's after/before per-stream ratios
(`before` is main, `after` a block's motion in 12 bytes, on the current decoder): the one-thread
calls (PREFIX1, pinned), the same with the mapped allocator (PREFIXM, MALLOC_MMAP_THRESHOLD_=131072)
and the eight-thread calls (PREFIX8, unpinned), discard and write, with a bootstrap 95% interval of
the median, overall and per class; the bars MR-1 to MR-5 of docs/design/h264.md ("Motion round: a
block's motion in 12 bytes, on the current decoder"); and the ratios to FFmpeg with the share of
streams faster than it. MR-6 (output) is read from identity.txt and the conformance report, not here."""
import json, random, statistics, sys

d, pre1, prem, pre8 = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
classes = ('sd', '720p', '1080p', '4k')


def load(prefix):
    rows = []
    for c in classes:
        rows += json.load(open(f'{d}/{prefix}.{c}.json'))['streams']
    ok = [x for x in rows if x.get('seconds') and 'before_discard' in x['seconds']
          and 'after_discard' in x['seconds']]
    return rows, ok


def ci(v, seed=20261025, n=2000):
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
        r['per_stream'] = {x['stream']: x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}']
                           for x in ok}
        res[p] = r
    return res


t1, tm, t8 = (table(load(p)[1]) for p in (pre1, prem, pre8))
out = {'failed': {p: [x['stream'] for x in load(p)[0] if x not in load(p)[1]] for p in (pre1, prem, pre8)},
       'threads1': t1, 'mapped': tm, 'threads8': t8}
out['bars'] = {
    'MR-1': t1['discard']['overall']['median'] <= 0.995 and t1['discard']['overall']['ci95'][1] < 1.000,
    'MR-2': t1['write']['overall']['median'] <= 0.995,
    'MR-3': all(t1['discard'][k]['median'] <= 1.005 for k in classes),
    'MR-4': tm['discard']['overall']['median'] <= 0.995 and tm['discard']['overall']['ci95'][1] < 1.000,
    'MR-5': t8['discard']['overall']['median'] <= 1.005
    and all(t8['discard'][k]['median'] <= 1.010 for k in classes),
}
json.dump(out, open(f'{d}/{pre1}.summary.json', 'w'), indent=1)
print('failed', out['failed'])
for name, t in (('1T', t1), ('1T mapped', tm), ('8T', t8)):
    for p in ('discard', 'write'):
        for k, v in t[p].items():
            if k == 'per_stream':
                continue
            print(f"{name} {p:7} {k:8} n={v['n']:2} after/before {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster_after']}/{v['n']} | vs ffmpeg "
                  f"before {v['before_vs_ffmpeg']:.3f} after {v['after_vs_ffmpeg']:.3f}, faster than ffmpeg "
                  f"{v['faster_than_ffmpeg_before']}/{v['n']} then {v['faster_than_ffmpeg_after']}/{v['n']}")
print('bars:', out['bars'])
print('per stream (1T discard, 1T write, mapped discard, 8T discard):')
for s_ in sorted(t1['discard']['per_stream']):
    g = lambda t, p: t[p]['per_stream'].get(s_, float('nan'))
    print(f"  {s_} {g(t1, 'discard'):.4f} {g(t1, 'write'):.4f} {g(tm, 'discard'):.4f} {g(t8, 'discard'):.4f}")
