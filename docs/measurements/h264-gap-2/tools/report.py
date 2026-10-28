#!/usr/bin/env python3
"""Per-stage totals of docs/measurements/h264-gap-2 from the perf reports
and callgrind outputs of the eight streams.

    report.py EVIDENCE_DIR > stages.json
"""
import collections
import json
import os
import re
import sys

import cg
import stages

SIDES = {"ours": "ours", "ffsimd": "ffmpeg", "ffc": "ffmpeg"}
# The FFmpeg binary the runs used (its symbols attribute the samples).
FFMPEG = os.environ.get("GAP2_FFMPEG", os.path.expanduser("~/ffbuild/ffmpeg-9.0.2/ffmpeg_g"))
PERF_LINE = re.compile(r"^\s+([\d.]+)%\s+(\d+)\s+\[.\]\s+(.*?)(?:\s+-\s+-)?\s*$")
EVENTS = {"Ir": ["Ir"], "D1 misses": ["D1mr", "D1mw"], "LL misses": ["DLmr", "DLmw"],
          "mispredicts": ["Bcm", "Bim"], "cond branches": ["Bc"]}


def main():
    root = sys.argv[1]
    streams = [l.split() for l in open(os.path.join(root, "streams.txt")) if l.strip()]
    out = {"streams": {}, "stages": {}, "other": {}}
    total = {s: collections.defaultdict(collections.Counter) for s in SIDES}
    addrs, names = cg.symbols(FFMPEG)

    def by_address(a):
        import bisect
        k = bisect.bisect_right(addrs, a) - 1
        # Past the executable's text (libc, the kernel), not FFmpeg's code.
        return names[k] if 0 <= k and a < addrs[-1] + 0x100000 else f"{a:#x} (outside ffmpeg)"
    unmapped = {s: collections.Counter() for s in SIDES}
    for sid, cls, frames in streams:
        per = {}
        for side, kind in SIDES.items():
            st = collections.defaultdict(collections.Counter)
            if kind == "ffmpeg":
                # FFmpeg's samples by address: its hand-written assembly has
                # local labels whose names repeat across files, so a sample
                # goes to the function at or below its address.
                with open(os.path.join(root, "perf", f"{sid}.{side}.ips")) as f:
                    for line in f:
                        name = by_address(int(line.split()[0], 16))
                        g = stages.stage(name, kind)
                        st[g]["ms"] += 1
                        if g == "other":
                            unmapped[side][name] += 1
            else:
                with open(os.path.join(root, "perf", f"{sid}.{side}.txt")) as f:
                    for line in f:
                        m = PERF_LINE.match(line)
                        if m:
                            name = m.group(3).strip()
                            g = stages.stage(name, kind)
                            st[g]["ms"] += int(m.group(2))
                            if g == "other":
                                unmapped[side][name] += int(m.group(2))
            fns = cg.per_function(os.path.join(root, "cg", f"{sid}.{side}.out"),
                                  {FFMPEG: 0} if kind == "ffmpeg" else {})
            for name, c in fns.items():
                g = stages.stage(name, kind)
                for ev, parts in EVENTS.items():
                    st[g][ev] += sum(c[p] for p in parts)
            per[side] = {g: dict(v) for g, v in st.items()}
            for g, v in st.items():
                total[side][g].update(v)
        out["streams"][sid] = {"class": cls, "frames": int(frames), "stages": per}
    out["stages"] = {side: {g: dict(v) for g, v in t.items()} for side, t in total.items()}
    out["other"] = {side: c.most_common(15) for side, c in unmapped.items()}
    json.dump(out, sys.stdout, indent=1)


if __name__ == "__main__":
    main()
