"""python3 -I analyze.py DIR PREFIX -> per-stream ratios for the pairs a/base, c/base and c/a,
overall and per class, discard and write, with a bootstrap 95% interval; and ratios to FFmpeg."""
import json, random, statistics, sys
d, pre = sys.argv[1], sys.argv[2]
classes = ('sd', '720p', '1080p', '4k')
rows = []
for c in classes:
    rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
names = ('base', 'a', 'c')
ok = [x for x in rows if x.get('seconds') and all(f'{n}_discard' in x['seconds'] for n in names)]

def ci(v, seed=20261021, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]

out = {'failed': [x['stream'] for x in rows if x not in ok], 'pairs': {}, 'vs_ffmpeg': {}}
for num, den in (('a', 'base'), ('c', 'base'), ('c', 'a')):
    key = f'{num}/{den}'
    out['pairs'][key] = {}
    for p in ('discard', 'write'):
        res = {}
        for lab in ('overall',) + classes:
            sel = [x for x in ok if lab == 'overall' or x['class'] == lab]
            v = [x['seconds'][f'{num}_{p}'] / x['seconds'][f'{den}_{p}'] for x in sel]
            lo, hi = ci(v)
            res[lab] = {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
                        'faster': sum(1 for t in v if t < 1)}
        res['per_stream'] = {x['stream']: x['seconds'][f'{num}_{p}'] / x['seconds'][f'{den}_{p}'] for x in ok}
        out['pairs'][key][p] = res
for n in names:
    out['vs_ffmpeg'][n] = {}
    for p in ('discard', 'write'):
        r = [x['ratio'][f'{n}:{p}'] for x in ok]
        out['vs_ffmpeg'][n][p] = {'median': statistics.median(r), 'faster_than_ffmpeg': sum(1 for t in r if t < 1), 'n': len(r)}
json.dump(out, open(f'{d}/{pre}.summary.json', 'w'), indent=1)
print('failed', out['failed'])
for key, byp in out['pairs'].items():
    for p, res in byp.items():
        for lab, v in res.items():
            if lab == 'per_stream':
                continue
            print(f"{key:7} {p:7} {lab:8} n={v['n']:2} {v['median']:.4f} [{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
for n, byp in out['vs_ffmpeg'].items():
    for p, v in byp.items():
        print(f"vs ffmpeg {n:4} {p:7} median {v['median']:.3f} faster than ffmpeg {v['faster_than_ffmpeg']}/{v['n']}")
