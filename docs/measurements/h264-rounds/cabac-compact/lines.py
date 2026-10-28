# lines.py CALLGRIND.OUT [N]: self Ir by source file:line (inlined lines attributed to their own file), top N in coreav-h264's src.
import sys, re, collections
names = {}; cost = collections.Counter(); total = 0
fl = fi = None; skip = False; last = 0
def nm(v, kind):
    m = re.match(r'\((\d+)\)(?: (.*))?', v)
    if not m: return v
    k = (kind, m.group(1))
    if m.group(2): names[k] = m.group(2)
    return names.get(k, '?')
for line in open(sys.argv[1]):
    line = line.rstrip('\n')
    if line.startswith(('fl=',)): fl = nm(line[3:], 'f'); fi = None; continue
    if line.startswith(('fi=', 'fe=')): fi = nm(line[3:], 'f'); continue
    if line.startswith('fn='): nm(line[3:], 'n'); continue
    if line.startswith(('cfn=', 'cfi=', 'cfl=', 'cob=', 'ob=')):
        if line.startswith(('cfi=', 'cfl=')): nm(line[4:], 'f')
        if line.startswith('cfn='): nm(line[4:], 'n')
        continue
    if line.startswith('calls='): skip = True; continue
    if not line or not (line[0].isdigit() or line[0] in '+-*'): continue
    parts = line.split()
    p = parts[0]
    if p == '*': ln = last
    elif p[0] in '+-': ln = last + int(p)
    else: ln = int(p)
    last = ln
    if skip: skip = False; continue
    ir = int(parts[1]) if len(parts) > 1 else 0
    total += ir; f = fi or fl
    cost[(f.split('/')[-1] if f else '?', ln)] += ir
n = int(sys.argv[2]) if len(sys.argv) > 2 else 40
byfile = collections.Counter()
for (f, l), c in cost.items(): byfile[f] += c
print('total', total)
for f, c in byfile.most_common(14): print(f'{c/total*100:6.2f}% {f}')
want = set(sys.argv[3].split(',')) if len(sys.argv) > 3 else {'cabac.rs', 'syntax.rs', 'mb.rs'}
for (f, l), c in cost.most_common():
    if f in want and n > 0: print(f'{c/total*100:6.2f}% {f}:{l}'); n -= 1
