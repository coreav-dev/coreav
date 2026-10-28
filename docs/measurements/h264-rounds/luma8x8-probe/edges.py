# edges.py CALLGRIND.OUT: total Ir, self Ir by function name, and the calls and inclusive Ir of each call edge (caller fn -> callee fn).
import sys, re, collections, json
names = {}; selfc = collections.Counter(); edge = collections.Counter(); ncalls = collections.Counter(); total = 0
fn = None; cfn = None; incall = False
def nm(v):
    m = re.match(r'\((\d+)\)(?: (.*))?', v)
    if not m: return v
    if m.group(2): names[m.group(1)] = m.group(2)
    return names.get(m.group(1), '?')
for line in open(sys.argv[1]):
    if line.startswith('fn='): fn = nm(line[3:].strip()); continue
    if line.startswith('cfn='): cfn = nm(line[4:].strip()); continue
    if line.startswith('calls='): incall = True; ncalls[(fn, cfn)] += int(line[6:].split()[0]); continue
    if not line[:1].isdigit() and line[:1] not in '+-*': continue
    p = line.split()
    ir = int(p[1]) if len(p) > 1 else 0
    if incall: edge[(fn, cfn)] += ir; incall = False
    else: selfc[fn] += ir; total += ir
json.dump({'total': total, 'self': selfc, 'edge': [[a, b, v, ncalls[(a, b)]] for (a, b), v in edge.items()]}, sys.stdout)
