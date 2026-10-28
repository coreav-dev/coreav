"""python3 -I analyze.py DIR PREFIX -> after/before per-stream ratios, discard and
write, with a bootstrap 95% interval of the median: overall, per class, over the
nine streams with weighted prediction (bars WP-1 and WP-2) and over the others;
and the ratios to FFmpeg with the share of streams faster than it."""
import json, random, statistics, sys

d, pre = sys.argv[1], sys.argv[2]
classes = ('sd', '720p', '1080p', '4k')
# Fixed with the bars, before the run: the streams whose weighted
# predictions (combine calls) are at least 10% of their luma blocks in
# docs/measurements/h264-rounds/luma-mc/calls.txt.
WEIGHTED = {'9db41aa16b5a', '41e324ebb708', 'b41f70a7fa60', '65ec61f95a00', '1d4b1775d78a',
            'dcb262cf0138', '8ae90cadb5bc', '7650ae466748', '278dbf5a63bb'}
rows = []
for c in classes:
    rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
ok = [x for x in rows if x.get('seconds') and 'before_discard' in x['seconds']]


def ci(v, seed=20261021, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def stream_id(x):
    return x['stream'].split('.')[0]


out = {'failed': [x['stream'] for x in rows if x not in ok]}
for p in ('discard', 'write'):
    res = {}
    for lab in ('overall',) + classes + ('weighted', 'other'):
        if lab == 'weighted':
            sel = [x for x in ok if stream_id(x) in WEIGHTED]
        elif lab == 'other':
            sel = [x for x in ok if stream_id(x) not in WEIGHTED]
        else:
            sel = [x for x in ok if lab == 'overall' or x['class'] == lab]
        v = [x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}'] for x in sel]
        rb = [x['ratio'][f'before:{p}'] for x in sel]
        ra = [x['ratio'][f'after:{p}'] for x in sel]
        lo, hi = ci(v)
        res[lab] = {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
                    'faster_after': sum(1 for t in v if t < 1),
                    'before_vs_ffmpeg': statistics.median(rb), 'after_vs_ffmpeg': statistics.median(ra),
                    'faster_than_ffmpeg_before': sum(1 for t in rb if t < 1),
                    'faster_than_ffmpeg_after': sum(1 for t in ra if t < 1)}
    out[p] = res
    out[p]['per_stream'] = {x['stream']: x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}'] for x in ok}
json.dump(out, open(f'{d}/{pre}.summary.json', 'w'), indent=1)
print('failed', out['failed'])
for p in ('discard', 'write'):
    for k, v in out[p].items():
        if k == 'per_stream':
            continue
        print(f"{p:7} {k:8} n={v['n']:2} after/before {v['median']:.4f} [{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] "
              f"faster {v['faster_after']}/{v['n']} | vs ffmpeg before {v['before_vs_ffmpeg']:.3f} after "
              f"{v['after_vs_ffmpeg']:.3f}, faster than ffmpeg {v['faster_than_ffmpeg_after']}/{v['n']}")
