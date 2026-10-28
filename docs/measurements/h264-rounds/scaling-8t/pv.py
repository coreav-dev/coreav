"""pv.py, run in pv-run1/: probe PV's ratios, pv2 over base and each over FFmpeg, median of the 32 with a bootstrap 95%
interval (10,000 resamples, seed 1), and per class. Output: pv-run1/pv-summary.txt."""
import json, random, statistics as st, sys
rows = []
for c in ('sd', '720p', '1080p', '4k'):
    d = json.load(open(f't8.{c}.json'))
    for s in d['streams']:
        sec = s['seconds']
        rows.append((c, s['stream'], sec['pv2_discard'] / sec['base_discard'], sec['pv2_write'] / sec['base_write'],
                     s['ratio']['base:discard'], s['ratio']['pv2:discard']))
    if c == 'sd':
        print('null keys:', [k for k in d['streams'][0] if 'null' in k.lower()], d['summary'].keys() if isinstance(d['summary'], dict) else '')
def ci(x):
    g = random.Random(1); m = sorted(st.median(g.choices(x, k=len(x))) for _ in range(10000))
    return st.median(x), m[249], m[9749]
for i, n in ((2, 'discard pv2/base'), (3, 'write pv2/base'), (4, 'base/FFmpeg discard'), (5, 'pv2/FFmpeg discard')):
    x = [r[i] for r in rows]
    m, lo, hi = ci(x)
    print(f'{n:22s} all {m:.4f} [{lo:.4f}, {hi:.4f}] ' + ' '.join(f'{c} {st.median(r[i] for r in rows if r[0]==c):.4f}' for c in ('sd','720p','1080p','4k')) + f'  below1 {sum(v<1 for v in x)}/32')
