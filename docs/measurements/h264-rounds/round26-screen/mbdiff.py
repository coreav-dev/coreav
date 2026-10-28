"""mbdiff.py STREAM A B [N]: per coreav line (inline-folded), ms a run in A and B (pdp1/), largest |B-A| first."""
import sys, collections
s, A, Bn = sys.argv[1:4]; N = int(sys.argv[4]) if len(sys.argv) > 4 else 15
src = open('$HOME/h264work/r26/addr.py').read().split("if __name__")[0]
src = src.replace("f'{D}pd/{s}.*.data'", "f'{D}pdp1/{s}.{name}.*.data'").replace("f'{D}pd/{s}.{name}.ips'", "f'{D}pdp1/{s}.{name}.ips'")
out = []
for b in (A, Bn):
    sys.argv = ['x', '$HOME/h264work/bins/' + b, s]; g = {'__name__': 'x'}; exec(src, g)
    T = g['perf_ips'](); F = g['frames'](set(T)); agg = collections.Counter()
    for a, v in T.items(): agg[(g['key'](F.get(a, [])) if a >= 0 else ('outside', ''))[0]] += v
    out.append(agg)
a, b = out; print(f'{s}: {sum(a.values()):.0f} -> {sum(b.values()):.0f} ms')
for k in sorted(set(a) | set(b), key=lambda k: -abs(b[k] - a[k]))[:N]:
    print(f'  {k:34} {a[k]:7.1f} {b[k]:7.1f} {b[k] - a[k]:+7.1f}')
