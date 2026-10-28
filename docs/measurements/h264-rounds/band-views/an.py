"""an.py S1.json: per-class decomposition r8 = W1 * I * B (plan-scaling.md, written before the run)."""
import json, statistics as st, sys
d = json.load(open(sys.argv[1])); M = lambda v, i: st.median(x[i] for x in v)
rows = []
for f, a in sorted(d.items(), key=lambda kv: ['sd', '720p', '1080p', '4k'].index(kv[0].split('.')[2])):
    if not a['us1']: continue
    g = {k: [M(v, i) for i in range(6)] for k, v in a.items()}
    w1 = g['us1'][1] / g['ff1'][1]; I = (g['us8'][1] / g['us1'][1]) / (g['ff8'][1] / g['ff1'][1])
    B = (g['ff8'][1] / g['ff8'][0]) / (g['us8'][1] / g['us8'][0]); r8 = g['us8'][0] / g['ff8'][0]; r1 = g['us1'][0] / g['ff1'][0]
    rows.append((f.split('.')[2], f[:12], r1, r8, w1, I, B, I * B, g['us8'][1] / g['us8'][0], g['ff8'][1] / g['ff8'][0],
                 g['us8'][1] / g['us1'][1], g['ff8'][1] / g['ff1'][1], g['us1'][0] / g['us8'][0], g['ff1'][0] / g['ff8'][0],
                 g['us8'][3] - g['us1'][3], g['us8'][4], g['us8'][2] / g['us8'][1]))
H = 'class stream r1 r8 W1 I B IB busy_us busy_ff infl_us infl_ff scale_us scale_ff dfault_us csw_us sys_us'
print(H)
for r in rows: print(r[0], r[1], ' '.join(f'{x:.3f}' if isinstance(x, float) else str(x) for x in r[2:]))
for c in ['sd', '720p', '1080p', '4k', 'all']:
    s = [r for r in rows if c in ('all', r[0])]
    print('MEDIAN', c, ' '.join(f'{st.median(r[i] for r in s):.3f}' for i in range(2, 17)))
