"""mbline.py STREAM PATTERN BIN...: ms a run per coreav line (inline-folded) matching PATTERN, per build (pdp1/)."""
import sys, collections, re
s, pat, bins = sys.argv[1], sys.argv[2], sys.argv[3:]
src = open('$HOME/h264work/r26/addr.py').read().split("if __name__")[0]
src = src.replace("f'{D}pd/{s}.*.data'", "f'{D}pdp1/{s}.{name}.*.data'").replace("f'{D}pd/{s}.{name}.ips'", "f'{D}pdp1/{s}.{name}.ips'")
for b in bins:
    sys.argv = ['x', '$HOME/h264work/bins/' + b, s]; g = {'__name__': 'x'}; exec(src, g)
    T = g['perf_ips'](); F = g['frames'](set(T)); agg = collections.Counter()
    for a, v in T.items(): agg[g['key'](F.get(a, []))[0] if a >= 0 else 'outside'] += v
    print(b, f'{sum(T.values()):.0f} ms', ' '.join(f'{k}={v:.1f}' for k, v in sorted(agg.items()) if re.match(pat, k)))
