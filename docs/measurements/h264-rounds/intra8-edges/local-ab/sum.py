import json,statistics,random,sys,glob
rows=[]
for f in sorted(glob.glob(sys.argv[1]+'/g.*.json')):
    for s in json.load(open(f))['streams']:
        t=s['seconds']; rows.append((s['class'],t['k_discard']/t['base_discard'],t['k_write']/t['base_write']))
def ci(v):
    r=random.Random(1);m=sorted(statistics.median(r.choices(v,k=len(v))) for _ in range(2000));return m[50],m[1949]
for i,p in ((1,'discard'),(2,'write')):
    v=[x[i] for x in rows];lo,hi=ci(v)
    print(f"k/base {p} overall n={len(v)} median {statistics.median(v):.4f} [{lo:.4f},{hi:.4f}] faster {sum(t<1 for t in v)}/{len(v)}")
    print('  '+' '.join(f"{c} {statistics.median([x[i] for x in rows if x[0]==c]):.4f}" for c in ('sd','720p','1080p','4k')))
