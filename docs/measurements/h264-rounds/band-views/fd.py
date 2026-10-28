"""fd.py STREAM...: per-function self Ir at 8T minus 1T, as share of 1T total."""
import re, subprocess, sys, collections
def fns(p):
    out = subprocess.run(['callgrind_annotate', '--auto=no', '--inclusive=no', '--threshold=100', p], capture_output=True, text=True).stdout
    ir = collections.Counter(); tot = 0
    for l in out.splitlines():
        m = re.match(r'^\s*([\d,]+) \([ \d.]+%\)\s+(.*)$', l)
        if not m: continue
        n = int(m.group(1).replace(',', '')); rest = m.group(2)
        if 'PROGRAM TOTALS' in rest: tot = n; continue
        fn = re.sub(r' \[.*\]$', '', rest.split(':', 1)[1] if ':' in rest else rest)
        fn = re.sub(r'::h[0-9a-f]{16}$', '', fn)
        ir[fn[:110]] += n
    return ir, tot
for s in sys.argv[1:]:
    a, ta = fns(f'cg/{s}.t1.out'); b, tb = fns(f'cg/{s}.t8.out')
    print(f'== {s} total 1T {ta:,} 8T/1T {tb/ta:.4f}')
    d = sorted(((b[k] - a[k]) / ta, k) for k in set(a) | set(b))
    for v, k in d[::-1][:14]: print(f'  +{100*v:6.2f}%  {k}')
    for v, k in d[:4]: print(f'  {100*v:7.2f}%  {k}')
