#!/usr/bin/env python3
"""The A/B of docs/measurements/h264-kernels-training.md from the reports of
scripts/h264_parity.py: per stream, a candidate's median time over its
base's (discard pairing); the median over streams with a bootstrap 95%
interval (2,000 resamples, fixed seed), and the median per class.

    ab.py CANDIDATE BASE REPORT.json [REPORT.json ...]
"""
import json
import random
import statistics
import sys


def main():
    cand, base, paths = sys.argv[1], sys.argv[2], sys.argv[3:]
    ratios = {}
    for p in paths:
        for s in json.load(open(p))["streams"]:
            t = s["seconds"]
            ratios.setdefault(s["class"], []).append(t[f"{cand}_discard"] / t[f"{base}_discard"])
    every = [r for rs in ratios.values() for r in rs]
    rng = random.Random(1)
    boot = sorted(statistics.median(rng.choices(every, k=len(every))) for _ in range(2000))
    lo, hi = boot[49], boot[1949]
    print(f"{cand} / {base}: overall median {statistics.median(every):.4f} [{lo:.4f}, {hi:.4f}] "
          f"over {len(every)} streams, {sum(r < 1 for r in every)} faster")
    for cls in ("sd", "720p", "1080p", "4k"):
        if cls in ratios:
            rs = ratios[cls]
            print(f"  {cls}: median {statistics.median(rs):.4f} over {len(rs)}, {sum(r < 1 for r in rs)} faster")


if __name__ == "__main__":
    main()
