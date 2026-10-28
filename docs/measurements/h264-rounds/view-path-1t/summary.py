"""summary.py, run in run/: vp2 over b2 at one thread, 4k, discard and write, median of the 8 per-stream ratios with
a bootstrap 95% interval (10,000 resamples, seed 1); the null arm; b2 over FFmpeg. Output: run/summary.txt."""
import json, random, statistics as st
d = json.load(open('t1.4k.json'))
r, w, nd, nw, ff = [], [], [], [], []
for s in d['streams']:
    sec = s['seconds']
    r.append(sec['vp2_discard'] / sec['b2_discard']); w.append(sec['vp2_write'] / sec['b2_write'])
    nd.append(s['null']['b2_discard']); nw.append(s['null']['b2_write']); ff.append(s['ratio']['b2:discard'])
def ci(x):
    g = random.Random(1); m = sorted(st.median(g.choices(x, k=len(x))) for _ in range(10000))
    return st.median(x), m[249], m[9749]
print('discard vp2/b2 %.4f [%.4f, %.4f]' % ci(r), f'vp2 faster on {sum(v < 1 for v in r)} of {len(r)}')
print('write   vp2/b2 %.4f [%.4f, %.4f]' % ci(w))
print('null (b2 twice) discard median %+.4f, largest %.4f; write median %+.4f' % (st.median(nd), max(map(abs, nd)), st.median(nw)))
print('b2/FFmpeg one thread 4k discard %.4f' % st.median(ff))
for s, v in zip(d['streams'], r):
    print(f"  {s['stream']} {v:.4f}")
