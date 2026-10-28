"""cgtop.py A.out [N]: per-function cachegrind totals, by est cycles; shares of est from each event class."""
import sys, re, collections
sys.path.insert(0, '$HOME/h264work/r25/tools')
from cgcmp_lib import load, agg, short
_, a = load(sys.argv[1]); N = int(sys.argv[2]) if len(sys.argv) > 2 else 30
rows = sorted(((agg(t), f) for f, t in a.items()), key=lambda r: -r[0]['est'])
T = collections.Counter()
for x, _ in rows: T.update(x)
print(f"total est {T['est']/1e6:.1f}M  Ir {T['Ir']/T['est']:.3f}  I1m {12*T['I1m']/T['est']:.3f}  D1m {12*T['D1m']/T['est']:.3f}  LLm {150*T['LLm']/T['est']:.3f}  Bm {15*T['Bm']/T['est']:.3f} (shares of est)")
print(f"{'function':60} {'est%':>6} {'Ir%':>6} | per 1000 Ir: {'I1m':>6} {'D1m':>6} {'LLm':>6} {'Bm':>6} | stall share")
for x, f in rows[:N]:
    ir = max(x['Ir'], 1); st = (x['est'] - x['Ir']) / max(x['est'], 1)
    print(f"{short(f)[-60:]:60} {100*x['est']/T['est']:6.2f} {100*x['Ir']/T['Ir']:6.2f} | {1000*x['I1m']/ir:6.2f} {1000*x['D1m']/ir:6.2f} {1000*x['LLm']/ir:6.3f} {1000*x['Bm']/ir:6.2f} | {st:.2f}")
