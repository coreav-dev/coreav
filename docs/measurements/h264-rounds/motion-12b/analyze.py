"""python3 -I analyze.py DIR PREFIX -> the motion round's ratios between two builds timed in the same rounds.

The reports are DIR/PREFIX.{sd,720p,1080p,4k}.json from scripts/h264_parity.py with the decoders
`base` (main, the commit the branch is based on) and `a` (the branch tip). For a over base, each
pairing (discard, write): the median per-stream ratio with a 95% bootstrap interval (2,000
resamples, random.Random(20261023)) and how many streams are faster, overall, per class, over the
streams with B slices (b-streams.txt beside this file) and over the others; then bars M12-1 to
M12-5 (docs/design/h264.md, "Motion round: a block's motion in 12 bytes"). Each build's median
ratio to FFmpeg and the streams faster than it, overall and per class, are information. Run it
once on the default run and once on the mapped one (MALLOC_MMAP_THRESHOLD_=131072), whose
M12-1, M12-3 and M12-4 are bar M12-6.
"""
import json, os, random, statistics, sys

d, pre = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
b_streams = {line.split()[0] for line in open(os.path.join(here, 'b-streams.txt')) if line.strip()}
classes = ('sd', '720p', '1080p', '4k')
rows = []
for c in classes:
    rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
builds = ('base', 'a')
ok = [x for x in rows if x.get('seconds') and all(f'{b}_discard' in x['seconds'] for b in builds)]


def stream_id(x):
    return os.path.basename(x['stream']).split('.')[0]


def ci(v, seed=20261023, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(p, sel):
    v = [x['seconds'][f'a_{p}'] / x['seconds'][f'base_{p}'] for x in sel]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
            'faster': sum(1 for t in v if t < 1)}


groups = {'overall': lambda x: True, 'b': lambda x: stream_id(x) in b_streams,
          'p': lambda x: stream_id(x) not in b_streams}
groups.update({c: (lambda c: lambda x: x['class'] == c)(c) for c in classes})
out = {'failed': [x['stream'] for x in rows if x not in ok], 'a/base': {}}
for p in ('discard', 'write'):
    res = {lab: summary(p, [x for x in ok if f(x)]) for lab, f in groups.items()}
    res['per_stream'] = {stream_id(x): x['seconds'][f'a_{p}'] / x['seconds'][f'base_{p}'] for x in ok}
    out['a/base'][p] = res
out['vs_ffmpeg'] = {}
for b in builds:
    out['vs_ffmpeg'][b] = {}
    for p in ('discard', 'write'):
        res = {}
        for lab in ('overall',) + classes:
            v = [x['ratio'][f'{b}:{p}'] for x in ok if lab == 'overall' or x['class'] == lab]
            res[lab] = {'n': len(v), 'median': statistics.median(v), 'faster': sum(1 for t in v if t < 1)}
        out['vs_ffmpeg'][b][p] = res
r = out['a/base']
bars = {
    'M12-1': r['discard']['overall']['median'] <= 0.995 and r['discard']['overall']['ci95'][1] < 1.0,
    'M12-2': r['write']['overall']['median'] <= 0.995,
    'M12-3': r['discard']['b']['median'] <= 0.985 and r['discard']['b']['ci95'][1] < 1.0,
    'M12-4': all(r[p]['p']['median'] <= 1.005 for p in ('discard', 'write')),
    'M12-5': all(r[p][c]['median'] <= 1.005 for p in ('discard', 'write') for c in classes),
}
out['bars'] = bars
json.dump(out, open(f'{d}/{pre}.summary.json', 'w'), indent=1)
print('failed', out['failed'])
for p, res in r.items():
    for lab, v in res.items():
        if lab == 'per_stream':
            continue
        print(f"a/base {p:7} {lab:8} n={v['n']:2} median {v['median']:.4f} "
              f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
for b, pairs in out['vs_ffmpeg'].items():
    for p, res in pairs.items():
        print(f"{b:4} vs FFmpeg {p:7} " + ' '.join(
            f"{lab} {v['median']:.3f} ({v['faster']}/{v['n']} faster)" for lab, v in res.items()))
for k, v in bars.items():
    print(k, 'passes' if v else 'FAILS')
