import sys,re,collections
# usage: fnsum.py callgrind.out [regex] -> self and inclusive per function
path=sys.argv[1]; pat=re.compile(sys.argv[2] if len(sys.argv)>2 else '.')
names={}; cur=None; incall=False; ev=None
self_=collections.defaultdict(lambda:None); incl={}
S=collections.defaultdict(lambda:[0]*5); C=collections.defaultdict(lambda:[0]*5)
def name(m):
    i,n=m.group(1),m.group(2)
    if n: names[i]=n
    return names.get(i,i)
for line in open(path):
    if line.startswith('events:'): ev=line.split()[1:]; continue
    if line.startswith('fn='):
        cur=name(re.match(r'fn=\((\d+)\)(?: (.*))?',line.strip())); continue
    if line.startswith('cfn='):
        name(re.match(r'cfn=\((\d+)\)(?: (.*))?',line.strip())); continue
    if line.startswith('calls='): incall=True; continue
    if not line[:1] or not (line[0].isdigit() or line[0] in '+-*'): continue
    parts=line.split()
    vals=[int(v) for v in parts[2:]] if True else []
    tgt=C if incall else S
    incall=False
    for i,v in enumerate(vals[:5]): tgt[cur][i]+=v
tot=sum(v[0] for v in S.values())
print('total Ir',tot,'events',ev)
rows=[(n,S[n],C[n]) for n in set(S)|set(C) if pat.search(n)]
rows.sort(key=lambda r:-(r[1][0]+r[2][0]))
for n,s,c in rows[:40]:
    print(f"{s[0]/1e6:9.3f} self {(s[0]+c[0])/1e6:9.3f} incl  Bc {s[1]/1e6:6.2f} Bcm {s[2]/1e6:5.2f} Bim {s[4]/1e6:5.2f}  {n[:150]}")
