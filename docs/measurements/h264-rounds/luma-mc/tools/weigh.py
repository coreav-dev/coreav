"""weigh.py SHAPES CALLS [COLUMN]: the per-shape counts of shapes.txt (column
COLUMN after the bar: 0 main, 1 A, 2 C) weighted by the luma call counts of
calls.txt (stream, kind 0 luma_mc / 1 luma_mc_avg / 2 combine, w, h, xFrac,
yFrac, calls; for combine xFrac is the mode)."""
import collections
import sys

column = int(sys.argv[3]) if len(sys.argv) > 3 else 0
cost = {}
for line in open(sys.argv[1]):
    if line.startswith("#") or not line.strip():
        continue
    shape, counts = line.split("|")
    w, h, xf, yf, avg = map(int, shape.split())
    cost[(avg, w, h, xf, yf)] = int(counts.split()[column])
weighted = collections.Counter()
total = 0
for line in open(sys.argv[2]):
    _, kind, w, h, xf, yf, calls = map(lambda v: v, line.split())
    kind, w, h, xf, yf, calls = map(int, (kind, w, h, xf, yf, calls))
    if kind > 1:
        continue
    c = cost[(kind, w, h, xf, yf)] * calls
    total += c
    weighted[(kind, w, "full" if (xf, yf) == (0, 0) else "fractional")] += c
print(f"total {total / 1e9:.2f} G instructions (training set, class frame counts)")
for key, v in sorted(weighted.items(), key=lambda x: -x[1]):
    print(key, f"{v / 1e9:.2f} G {100 * v / total:.1f}%")
