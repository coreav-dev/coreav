"""python3 -I analyze.py DIR PREFIX -> round 9's ratios between three builds timed in the same rounds.

The reports are DIR/PREFIX.{sd,720p,1080p,4k}.json from scripts/h264_parity.py with the decoders
`base` (main at 5dcea04), `b` (part B alone) and `ab` (parts A and B). For each pair (ab over
base, b over base, ab over b), each pairing (discard, write), overall and per class: the median
per-stream ratio with a 95% bootstrap interval (2,000 resamples, random.Random(20261021)) and how
many streams are faster; and each build's median ratio to FFmpeg as information.
"""
import json, random, statistics, sys

d, pre = sys.argv[1], sys.argv[2]
classes = ('sd', '720p', '1080p', '4k')
rows = []
for c in classes:
    rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
builds = ('base', 'b', 'ab')
ok = [x for x in rows if x.get('seconds') and all(f'{b}_discard' in x['seconds'] for b in builds)]


def ci(v, seed=20261021, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


out = {'failed': [x['stream'] for x in rows if x not in ok], 'pairs': {}}
for new, old in (('ab', 'base'), ('b', 'base'), ('ab', 'b')):
    key = f'{new}/{old}'
    out['pairs'][key] = {}
    for p in ('discard', 'write'):
        res = {}
        for lab in ('overall',) + classes:
            sel = [x for x in ok if lab == 'overall' or x['class'] == lab]
            v = [x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in sel]
            lo, hi = ci(v)
            res[lab] = {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
                        'faster': sum(1 for t in v if t < 1)}
        res['per_stream'] = {x['stream']: x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in ok}
        out['pairs'][key][p] = res
out['vs_ffmpeg'] = {b: {p: statistics.median([x['ratio'][f'{b}:{p}'] for x in ok]) for p in ('discard', 'write')}
                    for b in builds}
json.dump(out, open(f'{d}/{pre}.summary.json', 'w'), indent=1)
print('failed', out['failed'])
for key, pairs in out['pairs'].items():
    for p, res in pairs.items():
        for lab, v in res.items():
            if lab == 'per_stream':
                continue
            print(f"{key:8} {p:7} {lab:8} n={v['n']:2} median {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
for b, v in out['vs_ffmpeg'].items():
    print(f"{b:4} vs FFmpeg: discard {v['discard']:.3f} write {v['write']:.3f}")
