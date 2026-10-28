"""abcls.py FILE NUM DEN: per-class and overall median of ratio(NUM)/ratio(DEN) from ab3.py output ('base' = 1), bootstrap 95% CI, wins."""
import sys, re, statistics, random
rows = []
for line in open(sys.argv[1]):
    p = line.split()
    if not p or p[0] == 'median' or line.startswith('#'): continue
    r = {'base': 1.0}; r.update({p[i]: float(p[i+1]) for i in range(2, len(p), 2)}); rows.append((p[0].split('.')[2], r))
def q(r): return r[sys.argv[2]] / r[sys.argv[3]]
def ci(v):
    random.seed(1); b = sorted(statistics.median(random.choices(v, k=len(v))) for _ in range(4000)); return b[100], b[3899]
for c in ['sd', '720p', '1080p', '4k', 'all']:
    v = [q(r) for k, r in rows if c in ('all', k)]
    lo, hi = ci(v)
    print(f'{c:6} n={len(v):2} median {statistics.median(v):.4f} [{lo:.4f}, {hi:.4f}] faster {sum(x < 1 for x in v)}/{len(v)}')
