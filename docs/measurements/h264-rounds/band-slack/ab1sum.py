"""ab1sum.py ab1.txt: the summary line of an ab3.py run, median per stream with a bootstrap 95% interval
(2,000 resamples, random.Random(20261025)), streams faster, and the median per class."""
import random, statistics as st, sys
q = {r[0]: float(r[3]) for r in (l.split() for l in open(sys.argv[1])) if r and r[0] not in ('#', 'median')}
rng = random.Random(20261025)
m = sorted(st.median(rng.choices(list(q.values()), k=len(q))) for _ in range(2000))
print(f"all {st.median(q.values()):.4f} [{m[50]:.4f}, {m[1949]:.4f}] ({sum(v < 1 for v in q.values())}/{len(q)} faster)",
      ' '.join(f"{c} {st.median(v for f, v in q.items() if f.split('.')[2] == c):.4f}" for c in ('sd', '720p', '1080p', '4k')))
