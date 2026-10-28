"""cgcmp.py A.out B.out [N]: per-function cachegrind totals of two runs; totals and the N largest |delta| in est. cycles.
est = Ir + 12*(D1m) + 150*(LLm) + 15*(Bm) + 12*I1mr  (a rough Ice Lake-like weighting, for ranking only)"""
import sys, re, collections
def load(p):
    fn = None; tot = collections.defaultdict(lambda: [0]*13); ev = None
    for line in open(p):
        if line.startswith('events:'): ev = line.split()[1:]
        elif line.startswith('fn='): fn = line[3:].strip()
        elif line[:1].isdigit() and fn:
            v = [int(x) for x in line.split()[1:]]
            t = tot[fn]
            for i, x in enumerate(v): t[i] += x
    return ev, tot
def short(f):
    f = re.sub(r'::h[0-9a-f]{16}$', '', f); return f[-70:]
def agg(t):
    Ir,I1,IL,Dr,D1r,DLr,Dw,D1w,DLw,Bc,Bcm,Bi,Bim = t
    return dict(Ir=Ir, I1m=I1, D1m=D1r+D1w, LLm=DLr+DLw+IL, Br=Bc+Bi, Bm=Bcm+Bim,
                est=Ir+12*(D1r+D1w)+150*(DLr+DLw+IL)+15*(Bcm+Bim)+12*I1)
ev, a = load(sys.argv[1]); _, b = load(sys.argv[2]); N = int(sys.argv[3]) if len(sys.argv) > 3 else 25
keys = ['Ir','I1m','D1m','LLm','Br','Bm','est']
TA = collections.Counter(); TB = collections.Counter()
for f in a: TA.update(agg(a[f]))
for f in b: TB.update(agg(b[f]))
print('total   ' + '  '.join(f'{k} {TA[k]/1e6:.2f}M->{TB[k]/1e6:.2f}M ({TB[k]/TA[k]:.4f})' for k in keys))
fs = set(a) | set(b); z = [0]*13
rows = []
for f in fs:
    x = agg(a.get(f, z)); y = agg(b.get(f, z)); rows.append((y['est']-x['est'], f, x, y))
rows.sort(key=lambda r: -abs(r[0]))
print(f"{'function':70} " + ' '.join(f'{k:>16}' for k in keys))
for d, f, x, y in rows[:N]:
    print(f'{short(f):70} ' + ' '.join(f"{(y[k]-x[k])/1e3:+16.0f}" for k in keys))
