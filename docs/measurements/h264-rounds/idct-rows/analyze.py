"""python3 -I analyze.py DIR PREFIX -> the transform round's ratios between three builds timed in the same rounds.

The reports are DIR/PREFIX.{sd,720p,1080p,4k}.json from scripts/h264_parity.py with the decoders
`base` (main at 540c770c), `q` (the four-block kernel alone, 80ddbf19) and `a` (both kernels, the
branch tip). For each pair (a over base, q over base, a over q), each pairing (discard, write),
overall and per class: the median per-stream ratio with a 95% bootstrap interval (2,000 resamples,
random.Random(20261022)) and how many streams are faster. `a over q` is also read on the streams of
streams-8x8.txt whose PPS enables the 8x8 transform (bar TR-5). Each build's median ratio to FFmpeg
and the streams faster than it, overall and per class, are information.
"""
import json, os, random, statistics, sys

d, pre = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
classes = ('sd', '720p', '1080p', '4k')
rows = []
for c in classes:
    rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
builds = ('base', 'q', 'a')
ok = [x for x in rows if x.get('seconds') and all(f'{b}_discard' in x['seconds'] for b in builds)]
t8 = {line.split()[0] for line in open(f'{here}/streams-8x8.txt')
      if not line.startswith('#') and line.split()[1:] == ['1']}


def ci(v, seed=20261022, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(new, old, p, sel):
    v = [x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in sel]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
            'faster': sum(1 for t in v if t < 1)}


out = {'failed': [x['stream'] for x in rows if x not in ok], 'pairs': {}}
for new, old in (('a', 'base'), ('q', 'base'), ('a', 'q')):
    key = f'{new}/{old}'
    out['pairs'][key] = {}
    for p in ('discard', 'write'):
        res = {lab: summary(new, old, p, [x for x in ok if lab == 'overall' or x['class'] == lab])
               for lab in ('overall',) + classes}
        if key == 'a/q':
            res['8x8'] = summary(new, old, p, [x for x in ok if x['stream'] in t8])
        res['per_stream'] = {x['stream']: x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in ok}
        out['pairs'][key][p] = res
out['vs_ffmpeg'] = {}
for b in builds:
    out['vs_ffmpeg'][b] = {}
    for p in ('discard', 'write'):
        res = {}
        for lab in ('overall',) + classes:
            v = [x['ratio'][f'{b}:{p}'] for x in ok if lab == 'overall' or x['class'] == lab]
            res[lab] = {'n': len(v), 'median': statistics.median(v), 'faster': sum(1 for t in v if t < 1)}
        out['vs_ffmpeg'][b][p] = res
json.dump(out, open(f'{d}/{pre}.summary.json', 'w'), indent=1)
print('failed', out['failed'], 'streams with the 8x8 transform', len([x for x in ok if x['stream'] in t8]))
for key, pairs in out['pairs'].items():
    for p, res in pairs.items():
        for lab, v in res.items():
            if lab == 'per_stream':
                continue
            print(f"{key:8} {p:7} {lab:8} n={v['n']:2} median {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
for b, pairs in out['vs_ffmpeg'].items():
    for p, res in pairs.items():
        print(f"{b:4} vs FFmpeg {p:7} " + ' '.join(
            f"{lab} {v['median']:.3f} ({v['faster']}/{v['n']} faster)" for lab, v in res.items()))
