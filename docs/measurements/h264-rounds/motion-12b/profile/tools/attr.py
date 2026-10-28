#!/usr/bin/env python3
"""Self cost per function, per (function, inlined file:line) and per (function, address)
from callgrind outs written with --branch-sim=yes --dump-instr=yes. Sums several outs.
usage: attr.py side out.json files..."""
import collections, json, re, sys
sys.path.insert(0, '/tmp/mbc/tools')
NAMED = re.compile(r"^\((\d+)\)(?: (.*))?$")
def parse(path, fnc, linec, addrc):
    names = {"fl": {}, "fn": {}, "ob": {}}
    def named(kind, v):
        m = NAMED.match(v)
        if not m: return v
        if m.group(2) is not None: names[kind][m.group(1)] = m.group(2)
        return names[kind].get(m.group(1), v)
    events = None; fn = fl = fi = None; pos = [0, 0]; skip = False
    with open(path, errors="replace") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line: continue
            c = line[0]
            if c.isdigit() or c in "+-*":
                tok = line.split()
                for i in range(2):
                    t = tok[i]
                    if t == "*": pass
                    elif t[0] in "+-": pos[i] += int(t, 0)
                    else: pos[i] = int(t, 0)
                if skip: skip = False; continue
                v = [int(x) for x in tok[2:]] + [0] * (len(events) - len(tok[2:]))
                src = (fi or fl or "?").split("/src/")[-1]
                for d, k in ((fnc, fn), (linec, (fn, f"{src}:{pos[1]}")), (addrc, (fn, pos[0]))):
                    acc = d[k]
                    for i, x in enumerate(v): acc[i] += x
            elif line.startswith("events:"): events = line.split()[1:]
            elif line.startswith("fl="): fl = named("fl", line[3:]); fi = None
            elif line.startswith(("fi=", "fe=")): fi = named("fl", line[3:])
            elif line.startswith("fn="): fn = named("fn", line[3:]); fi = None
            elif line.startswith("cfi=") or line.startswith("cfl="): named("fl", line[4:])
            elif line.startswith("cfn="): named("fn", line[4:])
            elif line.startswith("cob="): named("ob", line[4:])
            elif line.startswith("ob="): named("ob", line[3:])
            elif line.startswith("calls="): skip = True
    return events
side = sys.argv[1]; out = sys.argv[2]
z = lambda: [0] * 13
fnc = collections.defaultdict(z); linec = collections.defaultdict(z); addrc = collections.defaultdict(z)
for p in sys.argv[3:]:
    ev = parse(p, fnc, linec, addrc)
from stages import stage
st = collections.defaultdict(z)
for k, v in fnc.items():
    s = stage(k or "?", side)
    for i, x in enumerate(v): st[s][i] += x
json.dump({"events": ev, "stage": st,
           "fn": {k: v for k, v in fnc.items()},
           "line": [[k[0], k[1], v] for k, v in linec.items() if v[0] > 1e5 or v[2] > 1e3],
           "addr": [[k[0], k[1], v] for k, v in addrc.items() if v[2] > 2e3]}, open(out, "w"))
