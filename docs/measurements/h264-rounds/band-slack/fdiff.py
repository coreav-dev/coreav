"""fdiff.py A B: per-function self instructions, B minus A (callgrind outputs)."""
import re, subprocess, collections, sys
def fns(p):
    out = subprocess.run(['callgrind_annotate','--auto=no','--inclusive=no','--threshold=100',p],capture_output=True,text=True).stdout
    ir = collections.Counter()
    for l in out.splitlines():
        m = re.match(r'^\s*([\d,]+) \([ \d.]+%\)\s+(.*)$', l)
        if not m or 'PROGRAM TOTALS' in m.group(2): continue
        fn = re.sub(r' \[.*\]$','',m.group(2).split(':',1)[-1]); ir[fn[:100]] += int(m.group(1).replace(',',''))
    return ir
a=fns(sys.argv[1]); b=fns(sys.argv[2])
d=sorted(((b[k]-a[k]),k) for k in set(a)|set(b))
for v,k in d[::-1][:6]: print(f'+{v:>11,} {k}')
for v,k in d[:4]: print(f'{v:>12,} {k}')
