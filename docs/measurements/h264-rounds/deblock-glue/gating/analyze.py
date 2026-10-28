"""python3 -I analyze.py DIR PREFIX [PREFIX8] -> the deblocking glue round's after/before per-stream ratios
(`before` is main, `after` the thresholds from shifted tables), discard and write,
with a bootstrap 95% interval of the median: overall and per class; with PREFIX8, the
eight-thread call's discard ratio; the bars D-1 to D-4 of docs/design/h264.md
("Deblocking round: thresholds from shifted tables"); and the ratios to FFmpeg with the share of streams faster than it. D-5
(output) is read from identity.txt and the conformance report, not here."""
import json, random, statistics, sys

d, pre = sys.argv[1], sys.argv[2]
pre8 = sys.argv[3] if len(sys.argv) > 3 else None
classes = ('sd', '720p', '1080p', '4k')


def load(prefix):
    rows = []
    for c in classes:
        rows += json.load(open(f'{d}/{prefix}.{c}.json'))['streams']
    return rows, [x for x in rows if x.get('seconds') and 'before_discard' in x['seconds']]


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


rows, ok = load(pre)
out = {'failed': [x['stream'] for x in rows if x not in ok]}
for p in ('discard', 'write'):
    res = {lab: summary([x for x in ok if lab == 'overall' or x['class'] == lab], p)
           for lab in ('overall',) + classes}
    res['per_stream'] = {x['stream']: x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}'] for x in ok}
    out[p] = res
dis, wri = out['discard'], out['write']
out['bars'] = {
    'D-1': dis['overall']['ci95'][1] < 1.000,
    'D-2': wri['overall']['median'] <= 1.000,
    'D-3': all(dis[k]['median'] <= 1.005 for k in classes),
}
if pre8:
    rows8, ok8 = load(pre8)
    out['failed8'] = [x['stream'] for x in rows8 if x not in ok8]
    out['threads8'] = summary(ok8, 'discard')
    out['bars']['D-4'] = out['threads8']['median'] <= 1.005
json.dump(out, open(f'{d}/{pre}.summary.json', 'w'), indent=1)
print('failed', out['failed'], 'failed8', out.get('failed8'))
lines = [(p, k, v) for p in ('discard', 'write') for k, v in out[p].items() if k != 'per_stream']
if pre8:
    lines.append(('8T dis', 'overall', out['threads8']))
for p, k, v in lines:
    print(f"{p:7} {k:8} n={v['n']:2} after/before {v['median']:.4f} [{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] "
          f"faster {v['faster_after']}/{v['n']} | vs ffmpeg before {v['before_vs_ffmpeg']:.3f} after "
          f"{v['after_vs_ffmpeg']:.3f}, faster than ffmpeg {v['faster_than_ffmpeg_after']}/{v['n']}")
print('bars:', out['bars'])
print('per stream (discard, write):')
for x in ok:
    s_ = x['stream']
    print(f"  {s_} {x['class']:5} {dis['per_stream'][s_]:.4f} {wri['per_stream'][s_]:.4f}")
