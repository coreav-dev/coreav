#!/usr/bin/env python3
"""Per-function self costs from a callgrind.out file written with
--dump-instr=yes: {function: {event: count}}.

Code callgrind names by address alone (the hand-written assembly of a
binary whose symbols carry no size) is attributed to the nearest symbol at
or below the address, from `nm -n` of its object; `objects` maps an
object's path to the offset at which it was loaded (0 for a non-PIE
executable).
"""
import bisect
import collections
import re
import subprocess

NAMED = re.compile(r"^\((\d+)\)(?: (.*))?$")


def symbols(path):
    out = subprocess.run(["nm", "-n", "--defined-only", path], capture_output=True, text=True).stdout
    addrs, names = [], []
    for line in out.splitlines():
        parts = line.split(None, 2)
        # Local labels of the assembly (`..@N.name`, `.loop`) are not
        # functions: an address under one belongs to the function above it.
        if len(parts) == 3 and parts[1].lower() in ("t", "w") and not parts[2].startswith("."):
            addrs.append(int(parts[0], 16))
            names.append(parts[2])
    return addrs, names


def per_function(path, objects=None):
    objects = objects or {}
    tables = {}
    names = {"ob": {}, "fn": {}}
    events, ob, fn = None, None, None
    pos = [0, 0]
    skip_next = False
    totals = collections.defaultdict(collections.Counter)

    def named(kind, value):
        m = NAMED.match(value)
        if not m:
            return value
        if m.group(2) is not None:
            names[kind][m.group(1)] = m.group(2)
        return names[kind].get(m.group(1), value)

    with open(path, errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line:
                continue
            c = line[0]
            if c.isdigit() or c in "+-*" or line.startswith("0x"):
                tok = line.split()
                for i in range(2):
                    t = tok[i]
                    if t == "*":
                        pass
                    elif t[0] in "+-":
                        pos[i] += int(t, 0)
                    else:
                        pos[i] = int(t, 0)
                if skip_next:
                    skip_next = False
                    continue
                key = fn
                if fn is not None and (fn.startswith("0x") or fn == "???"):
                    base = objects.get(ob)
                    if base is not None:
                        if ob not in tables:
                            tables[ob] = symbols(ob)
                        addrs, syms = tables[ob]
                        k = bisect.bisect_right(addrs, pos[0] - base) - 1
                        key = syms[k] if k >= 0 else fn
                cost = totals[key]
                for e, v in zip(events, tok[2:]):
                    cost[e] += int(v)
            elif line.startswith("events:"):
                events = line.split()[1:]
            elif line.startswith("ob="):
                ob = named("ob", line[3:])
            elif line.startswith("fn="):
                fn = named("fn", line[3:])
            elif line.startswith("cob="):
                named("ob", line[4:])
            elif line.startswith("cfn="):
                named("fn", line[4:])
            elif line.startswith("calls="):
                skip_next = True
    return totals
