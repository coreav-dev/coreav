"""Median per-stream ratio of callgrind instruction totals: python3 -I ic_ratio.py FILE BASE ARM...

FILE holds `stream bin total` lines as ic32.sh writes them."""
import statistics
import sys

path, base, arms = sys.argv[1], sys.argv[2], sys.argv[3:]
totals = {}
for line in open(path):
    parts = line.split()
    if len(parts) == 3 and parts[2].isdigit():
        totals[(parts[0], parts[1])] = int(parts[2])
streams = sorted({s for s, _ in totals})
classes = ["sd", "720p", "1080p", "4k"]
for arm in arms:
    ratios = {s: totals[(s, arm)] / totals[(s, base)] for s in streams if (s, arm) in totals and (s, base) in totals}
    fewer = sum(r < 1 for r in ratios.values())
    by_class = []
    for c in classes:
        rs = [r for s, r in ratios.items() if s.endswith("." + c)]
        by_class.append(f"{c} {statistics.median(rs):.4f}" if rs else f"{c} -")
    print(f"{arm} / {base}: all {len(ratios)} {statistics.median(ratios.values()):.4f}, {fewer} fewer, "
          f"min {min(ratios.values()):.4f} max {max(ratios.values()):.4f}; " + ", ".join(by_class))
