"""python3 -I analyze.py DIR PREFIX8 PREFIX1 -> round 17's after/before per-stream ratios
(`before` is main, `after` banded references on the fast path): the eight-thread calls
(PREFIX8, unpinned, `--threads 8`) and the one-thread calls (PREFIX1, pinned), discard and
write, with a bootstrap 95% interval of the median, overall and per class; the bars B-1 to
B-3 of docs/design/h264.md ("Round 17"); the ratios to FFmpeg with the share of streams
faster than it; and P3's quantity, the scaling against FFmpeg's, per stream
(ours_8 / ffmpeg_8) / (ours_1 / ffmpeg_1), median per class, for each build. B-4 (output)
is read from identity.txt and the conformance report, not here."""
import json, random, statistics, sys

d, pre8, pre1 = sys.argv[1], sys.argv[2], sys.argv[3]
classes = ('sd', '720p', '1080p', '4k')


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
        r['per_stream'] = {x['stream']: x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}']
                           for x in ok}
        res[p] = r
    return res


rows8, ok8 = load(pre8)
rows1, ok1 = load(pre1)
out = {'failed8': [x['stream'] for x in rows8 if x not in ok8],
       'failed1': [x['stream'] for x in rows1 if x not in ok1],
       'threads8': table(ok8), 'threads1': table(ok1)}
t8, t1 = out['threads8'], out['threads1']
out['bars'] = {
    'B-1': t8['discard']['overall']['median'] <= 0.980 and t8['discard']['overall']['ci95'][1] < 1.000,
    'B-2': all(t8['discard'][k]['median'] <= 1.005 for k in classes)
    and t8['write']['overall']['median'] <= 1.005,
    'B-3': t1['discard']['overall']['median'] <= 1.005
    and all(t1['discard'][k]['median'] <= 1.005 for k in classes)
    and t1['write']['overall']['median'] <= 1.005,
}
# P3's quantity (information): per stream, the build's ratio to FFmpeg at eight threads over
# its ratio at one, discard.
one = {x['stream']: x for x in ok1}
scaling = {}
for build in ('before', 'after'):
    per = {x['stream']: x['ratio'][f'{build}:discard'] / one[x['stream']]['ratio'][f'{build}:discard']
           for x in ok8 if x['stream'] in one}
    cls = {x['stream']: x['class'] for x in ok8}
    scaling[build] = {lab: statistics.median([v for s, v in per.items() if lab == 'overall' or cls[s] == lab])
                      for lab in ('overall',) + classes}
out['p3_scaling'] = scaling
json.dump(out, open(f'{d}/{pre8}.summary.json', 'w'), indent=1)
print('failed8', out['failed8'], 'failed1', out['failed1'])
for name, t in (('8T', t8), ('1T', t1)):
    for p in ('discard', 'write'):
        for k, v in t[p].items():
            if k == 'per_stream':
                continue
            print(f"{name} {p:7} {k:8} n={v['n']:2} after/before {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster_after']}/{v['n']} | vs ffmpeg "
                  f"before {v['before_vs_ffmpeg']:.3f} after {v['after_vs_ffmpeg']:.3f}, faster than ffmpeg "
                  f"{v['faster_than_ffmpeg_before']}/{v['n']} then {v['faster_than_ffmpeg_after']}/{v['n']}")
for build, s in scaling.items():
    print(f'P3 scaling {build}: ' + ', '.join(f'{k} {v:.3f}' for k, v in s.items()))
print('bars:', out['bars'])
print('per stream (8T discard, 8T write, 1T discard, 1T write):')
for x in ok8:
    s_ = x['stream']
    print(f"  {s_} {x['class']:5} {t8['discard']['per_stream'][s_]:.4f} {t8['write']['per_stream'][s_]:.4f} "
          f"{t1['discard']['per_stream'].get(s_, float('nan')):.4f} {t1['write']['per_stream'].get(s_, float('nan')):.4f}")
