# sumx.py DIR A B [mode]: median per-stream ratio B/A over the g.*.json reports of DIR, with a bootstrap 95% interval.
import json,statistics,random,sys,glob
d,A,B=sys.argv[1:4]; modes=sys.argv[4:] or ['discard','write']
rows=[]
for f in sorted(glob.glob(d+'/g.*.json')):
    for s in json.load(open(f))['streams']:
        t=s['seconds']; rows.append((s['class'],{m:t[f'{B}_{m}']/t[f'{A}_{m}'] for m in modes}))
def ci(v):
    r=random.Random(1);m=sorted(statistics.median(r.choices(v,k=len(v))) for _ in range(2000));return m[50],m[1949]
for m in modes:
    v=[x[1][m] for x in rows];lo,hi=ci(v)
    print(f"{B}/{A} {m} n={len(v)} median {statistics.median(v):.4f} [{lo:.4f},{hi:.4f}] faster {sum(t<1 for t in v)}/{len(v)}  "+' '.join(f"{c} {statistics.median([x[1][m] for x in rows if x[0]==c]):.4f}" for c in ('sd','720p','1080p','4k') if any(x[0]==c for x in rows)))
