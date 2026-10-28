import statistics, random, pickle, sys
b = pickle.load(open('/tmp/ent/bins.pkl', 'rb'))
rows = []
for l in open(sys.argv[1]):
    p = l.split()
    if p[0] == 'median': continue
    rows.append((p[0][:12], p[0].split('.')[2], {p[i]: float(p[i + 1]) for i in range(2, len(p), 2)}))
def ci(v):
    random.seed(1); bs = sorted(statistics.median(random.choices(v, k=len(v))) for _ in range(4000)); return bs[100], bs[3899]
for arm in rows[0][2]:
    print(arm)
    for lab, sel in [('CABAC', lambda s, c: b[s]['sig'] > 0), ('CAVLC', lambda s, c: b[s]['sig'] == 0), ('all', lambda s, c: True)] + [(c, (lambda cc: lambda s, c: c == cc)(c)) for c in ('sd', '720p', '1080p', '4k')]:
        v = [r[arm] for s, c, r in rows if sel(s, c)]
        lo, hi = ci(v)
        print(f'  {lab:6} n={len(v):2} median {statistics.median(v):.4f} [{lo:.4f}, {hi:.4f}] faster {sum(x < 1 for x in v)}/{len(v)}')
