#!/usr/bin/env python3
"""Dynamic instruction mix from callgrind (--dump-instr=yes) and objdump:
the share of executed instructions that are conditional branches, compares
or tests, and vector instructions (an xmm/ymm/zmm operand).

    mix.py BINARY BASE CALLGRIND.out [...]
"""
import collections
import re
import subprocess
import sys

INSN = re.compile(r"^\s*([0-9a-f]+):\s+(\S+)\s*(.*)$")


def classes(binary):
    out = subprocess.run(["objdump", "-d", "--no-show-raw-insn", binary], capture_output=True, text=True).stdout
    kind = {}
    for line in out.splitlines():
        m = INSN.match(line)
        if not m:
            continue
        op, args = m.group(2), m.group(3)
        if op.startswith("j") and op != "jmp":
            k = "cond branch"
        elif op.startswith(("cmp", "test")) and "mm" not in args:
            k = "compare/test"
        elif re.search(r"%[xyz]mm", args):
            k = "vector"
        else:
            k = "other"
        kind[int(m.group(1), 16)] = k
    return kind


def mix(path, kind, base):
    counts = collections.Counter()
    pos, skip = [0, 0], False
    with open(path, errors="replace") as f:
        for line in f:
            if line.startswith("calls="):
                skip = True
                continue
            c = line[:1]
            if not (c.isdigit() or c in "+-*"):
                continue
            tok = line.split()
            for i in range(2):
                t = tok[i]
                if t != "*":
                    pos[i] = pos[i] + int(t, 0) if t[0] in "+-" else int(t, 0)
            if skip:
                skip = False
                continue
            ir = int(tok[2]) if len(tok) > 2 else 0
            counts[kind.get(pos[0] - base, "outside the binary")] += ir
    return counts


def main():
    binary, base, outs = sys.argv[1], int(sys.argv[2], 0), sys.argv[3:]
    kind = classes(binary)
    total = collections.Counter()
    for p in outs:
        total.update(mix(p, kind, base))
    n = sum(total.values())
    print({k: f"{100 * v / n:.1f}%" for k, v in total.most_common()}, f"of {n / 1e9:.2f} G")


if __name__ == "__main__":
    main()
