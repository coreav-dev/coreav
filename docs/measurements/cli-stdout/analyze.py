#!/usr/bin/env python3
"""Reads times.json from bench.py and prints the ratios the bars in
docs/design/cli-stdout.md are judged on: per input the median of the
rounds of each arm, the ratios of those medians, their median over the
inputs, and a bootstrap 95% interval (inputs and rounds both resampled,
10,000 resamples, seed 407).

    analyze.py DIR [--exclude NAME]...

`--exclude` leaves inputs out (names as in inputs.json); the summary is
then written to summary-without-<first excluded name, 12 characters>.md.
"""
import json
import pathlib
import random
import statistics
import sys

RATIOS = [
    ("new-pipe / base-pipe (B1)", "new-pipe", "base-pipe"),
    ("base-pipe / base-file", "base-pipe", "base-file"),
    ("new-pipe / new-file (B2)", "new-pipe", "new-file"),
    ("new-file / base-file", "new-file", "base-file"),
    ("new-pipe / ffmpeg-pipe (B3)", "new-pipe", "ffmpeg-pipe"),
    ("base-pipe / ffmpeg-pipe", "base-pipe", "ffmpeg-pipe"),
]


def main():
    d = pathlib.Path(sys.argv[1])
    excluded = [x for i, x in enumerate(sys.argv[2:]) if i % 2 == 1 and sys.argv[2 + i - 1] == "--exclude"]
    rows = [r for r in json.loads((d / "times.json").read_text())
            if not r["void"] and r["input"] not in excluded]
    assert all(r["code"] == 0 for r in rows)
    inputs = sorted({r["input"] for r in rows},
                    key=lambda n: next(i["size"] for i in json.loads((d / "inputs.json").read_text())["inputs"] if i["name"] == n))
    t = {}
    for r in rows:
        t.setdefault((r["input"], r["arm"]), []).append(r["time"])
    rng = random.Random(407)
    out = ["| Ratio | " + " | ".join(n[:12] for n in inputs) + " | Median (95% CI) |",
           "|---|" + "---|" * len(inputs) + "---|"]
    for label, num, den in RATIOS:
        per = [statistics.median(t[(i, num)]) / statistics.median(t[(i, den)]) for i in inputs]
        boots = []
        for _ in range(10000):
            sample = [rng.choice(inputs) for _ in inputs]
            ratios = []
            for i in sample:
                a, b = t[(i, num)], t[(i, den)]
                ratios.append(statistics.median(rng.choices(a, k=len(a))) /
                              statistics.median(rng.choices(b, k=len(b))))
            boots.append(statistics.median(ratios))
        boots.sort()
        lo, hi = boots[249], boots[9749]
        out.append(f"| {label} | " + " | ".join(f"{x:.3f}" for x in per) +
                   f" | {statistics.median(per):.3f} ({lo:.3f} to {hi:.3f}) |")
    out.append("")
    out.append("| Median time (ms) | " + " | ".join(n[:12] for n in inputs) + " |")
    out.append("|---|" + "---|" * len(inputs))
    for arm in ("base-pipe", "base-file", "new-pipe", "new-file", "ffmpeg-pipe"):
        out.append(f"| {arm} | " + " | ".join(f"{1000 * statistics.median(t[(i, arm)]):.2f}" for i in inputs) + " |")
    text = "\n".join(out) + "\n"
    name = f"summary-without-{excluded[0][:12]}.md" if excluded else "summary.md"
    (d / name).write_text(text)
    print(text)


if __name__ == "__main__":
    main()
