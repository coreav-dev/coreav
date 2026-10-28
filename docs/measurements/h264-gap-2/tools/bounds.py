#!/usr/bin/env python3
"""Bounds-check guards of a Rust binary: conditional jumps whose target
block calls one of the panics an out-of-bounds index or slice raises.

    bounds.py BINARY [CALLGRIND.out ...]

Statically: guards per function. With callgrind outputs written with
--dump-instr=yes: how many times the guards executed, against all
instructions. A guard is taken to cost its compare and its jump (two
instructions); that and anything about cycles is inference, and checks
written as `get(..)` with a fallback, which branch to no panic, are not
counted at all.
"""
import collections
import re
import subprocess
import sys

PANICS = re.compile(r"panic_bounds_check|slice_index_order_fail|slice_start_index_len_fail|slice_end_index_len_fail|slice_index_fail|slice_end_index_overflow_fail")
INSN = re.compile(r"^\s*([0-9a-f]+):\s+(\S+)\s*(.*)$")
FUNC = re.compile(r"^([0-9a-f]+) <(.*)>:$")


def disassemble(binary):
    out = subprocess.run(["objdump", "-d", "--no-show-raw-insn", "-C", binary], capture_output=True, text=True).stdout
    insns, func_of = [], {}
    func = None
    for line in out.splitlines():
        m = FUNC.match(line)
        if m:
            func = m.group(2)
            continue
        m = INSN.match(line)
        if m and func:
            addr = int(m.group(1), 16)
            insns.append((addr, m.group(2), m.group(3), func))
    return insns


def guards(insns):
    index = {a: i for i, (a, *_rest) in enumerate(insns)}
    found = collections.defaultdict(list)
    for a, op, args, func in insns:
        if not op.startswith("j") or op == "jmp":
            continue
        m = re.match(r"([0-9a-f]+)", args)
        if not m:
            continue
        t = index.get(int(m.group(1), 16))
        if t is None:
            continue
        for _, op2, args2, _f in insns[t : t + 8]:
            if op2.startswith("call"):
                if PANICS.search(args2):
                    found[func].append(a)
                break
            if op2.startswith("j") or op2.startswith("ret"):
                break
    return found


def executed(path, addrs, base):
    """Executions of the instructions at `addrs` (link-time addresses), from a
    callgrind.out with --dump-instr=yes whose code was loaded at `base`."""
    want = {a + base for a in addrs}
    count, total = 0, 0
    pos, skip, events = [0, 0], False, None
    with open(path, errors="replace") as f:
        for line in f:
            if line.startswith("events:"):
                events = line.split()[1:]
                continue
            if line.startswith("calls="):
                skip = True
                continue
            c = line[:1]
            if not (c.isdigit() or c in "+-*"):
                continue
            tok = line.split()
            for i in range(2):
                t = tok[i]
                if t == "*":
                    continue
                pos[i] = pos[i] + int(t, 0) if t[0] in "+-" else int(t, 0)
            if skip:
                skip = False
                continue
            ir = int(tok[2]) if len(tok) > 2 else 0
            total += ir
            if pos[0] in want:
                count += ir
    return count, total


def main():
    binary, outs = sys.argv[1], sys.argv[2:]
    insns = disassemble(binary)
    found = guards(insns)
    n = sum(len(v) for v in found.values())
    print(f"{n} bounds-check guards in {len(found)} functions")
    for func, a in sorted(found.items(), key=lambda kv: -len(kv[1]))[:15]:
        print(f"  {len(a):4d} {func[:110]}")
    all_addrs = [a for v in found.values() for a in v]
    for path in outs:
        # callgrind writes a PIE's addresses relative to the object.
        c, t = executed(path, all_addrs, 0)
        print(f"{path.rsplit('/', 1)[-1]}: guards executed {c:,} of {t:,} instructions ({100 * 2 * c / t:.2f}% at two instructions a guard)")


if __name__ == "__main__":
    main()
