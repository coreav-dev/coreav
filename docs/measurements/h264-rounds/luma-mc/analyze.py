"""python3 -I analyze.py DIR : paired a/base, c/base, c/a (discard, write), per class, bootstrap 95% (2,000 resamples, seed 20261021), ratio to FFmpeg."""
import json, random, statistics, sys
d = sys.argv[1]; rows = []
for c in ('sd', '720p', '1080p', '4k'): rows += json.load(open(f'{d}/g.{c}.json'))['streams']
def ci(v, seed=20261021, n=2000):
    rng = random.Random(seed); m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n)); return m[int(.025*n)], m[int(.975*n)-1]
out = {}
for n, b in [('a', 'base'), ('c', 'base'), ('c', 'a')]:
    for p in ('discard', 'write'):
        v = [x['seconds'][f'{n}_{p}'] / x['seconds'][f'{b}_{p}'] for x in rows]; lo, hi = ci(v)
        pc = {c: statistics.median([x['seconds'][f'{n}_{p}'] / x['seconds'][f'{b}_{p}'] for x in rows if x['class'] == c]) for c in ('sd', '720p', '1080p', '4k')}
        out[f'{n}/{b} {p}'] = {'median': statistics.median(v), 'ci95': [lo, hi], 'faster': sum(t < 1 for t in v), 'classes': pc}
        print(f"{n}/{b} {p}: {statistics.median(v):.4f} [{lo:.4f},{hi:.4f}] faster {sum(t < 1 for t in v)}/32 | " + ' '.join(f'{k}={t:.3f}' for k, t in pc.items()))
for n in ('base', 'a', 'c'):
    for lab in ('overall', 'sd', '720p', '1080p', '4k'):
        sel = [x for x in rows if lab == 'overall' or x['class'] == lab]
        out[f'{n} vs ffmpeg {lab}'] = {p: {'median': statistics.median([x['ratio'][f'{n}:{p}'] for x in sel]), 'faster': sum(x['ratio'][f'{n}:{p}'] < 1 for x in sel), 'n': len(sel)} for p in ('discard', 'write')}
json.dump(out, open(f'{d}/g.summary.json', 'w'), indent=1)
