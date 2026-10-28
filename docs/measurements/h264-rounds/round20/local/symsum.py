import sys,collections
A,B=sys.argv[2],sys.argv[3]
c=collections.defaultdict(lambda:[0,0])
for l in open(sys.argv[1]):
    b,n,s=l.rstrip('\n').split('\t',2)
    c[s.strip()][0 if b==A else 1]+=int(n)
ta=sum(v[0] for v in c.values()); tb=sum(v[1] for v in c.values())
print(f"total {A} {ta} {B} {tb} ratio {tb/ta:.4f}")
for s,(a,b) in sorted(c.items(),key=lambda kv:(kv[1][1]-kv[1][0]))[:14]:
    print(f"{b-a:+7d} ({(b-a)/ta:+.4f})  {a:6d} {b:6d} {b/max(a,1):.3f}  {s[:90]}")
print("increases")
for s,(a,b) in sorted(c.items(),key=lambda kv:-(kv[1][1]-kv[1][0]))[:10]:
    print(f"{b-a:+7d} ({(b-a)/ta:+.4f})  {a:6d} {b:6d} {b/max(a,1):.3f}  {s[:90]}")
