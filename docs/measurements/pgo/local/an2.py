# an2.py FILE BASE ARM...: median per-stream ratio ARM/BASE, 95% bootstrap, per class (classes present only).
import sys, json, statistics as st, random, collections
rows = [json.loads(l) for l in open(sys.argv[1])]; A = sys.argv[2]
t = collections.defaultdict(lambda: collections.defaultdict(list))
for r in rows: t[r['stream']][r['arm']].append(r['s'])
def ci(xs):
    rng = random.Random(7); m = sorted(st.median(rng.choices(xs, k=len(xs))) for _ in range(2000)); return m[50], m[1949]
for B in sys.argv[3:]:
    rat = {s: st.median(v[B]) / st.median(v[A]) for s, v in t.items() if v[A] and v[B]}
    out = []
    for c in ['all', 'sd', '720p', '1080p', '4k']:
        xs = [v for s, v in rat.items() if c == 'all' or s.split('.')[2] == c]
        if xs: lo, hi = ci(xs); out.append(f'{c} {st.median(xs):.4f} [{lo:.3f},{hi:.3f}] {sum(x < 1 for x in xs)}/{len(xs)}')
    print(f'{B:10s} ' + ' | '.join(out))
