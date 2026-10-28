"""calls.py OUT PATTERN...: calls into and self/inclusive Ir of functions whose name contains a pattern."""
import sys, re, collections
out, pats = sys.argv[1], sys.argv[2:]
names = {}; calls = collections.Counter(); incl = collections.Counter(); selfir = collections.Counter()
cur = None; cfn = None; pending = None
def nm(kind, s):
    m = re.match(r'\((\d+)\)(?: (.*))?', s)
    if not m: return s
    if m.group(2): names[m.group(1)] = m.group(2)
    return names.get(m.group(1), '?')
for line in open(out, errors='replace'):
    if line.startswith('fn='): cur = nm('fn', line[3:].strip()); continue
    if line.startswith('cfn='): cfn = nm('fn', line[4:].strip()); continue
    if line.startswith('calls='): pending = int(line[6:].split()[0]); continue
    if line[:1] in '0123456789+-*':
        parts = line.split()
        if len(parts) < 2: continue
        try: ir = int(parts[-1]) if len(parts) >= 2 else 0
        except ValueError: continue
        if pending is not None:
            calls[cfn] += pending; incl[cfn] += ir; pending = None
        else: selfir[cur] += ir
for f in set(list(calls) + list(selfir)):
    if any(p in f for p in pats):
        print(f'{calls[f]:>10} calls  self {selfir[f]:>12}  self/call {selfir[f]/max(calls[f],1):7.1f}  {f[:140]}')
