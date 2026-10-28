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
