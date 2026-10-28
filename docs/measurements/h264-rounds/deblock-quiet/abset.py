"""abset.py FILE NAME: from ab3.py's output, the median of NAME's ratio to `base` (bootstrap 95% interval, streams
faster) over the target streams T of docs/design/h264.md ("Deblocking round: macroblocks below the thresholds"), the
other 22 (R), all 32 and each class."""
import random, statistics, sys
T = {'81ed4778ece2', 'ed75c044e944', 'ae16bb7c7192', 'e624cbe0d2be', '4b54de28c2b3', 'f8d92a911e72',
     '6e90422a670a', 'a6af290d9053', '3b1b441e23ae', '36501d448eaa'}
rows = []
for line in open(sys.argv[1]):
    p = line.split()
    if len(p) >= 4 and p[0] != 'median':
        rows.append((p[0][:12], p[0].split('.')[2], dict(zip(p[2::2], map(float, p[3::2])))[sys.argv[2]]))


def ci(v):
    random.seed(1)
    b = sorted(statistics.median(random.choices(v, k=len(v))) for _ in range(4000))
    return b[100], b[3899]


for lab, sel in [('T', lambda s, c: s in T), ('R', lambda s, c: s not in T), ('all', lambda s, c: True)] + \
        [(c, lambda s, c_, c=c: c_ == c) for c in ('sd', '720p', '1080p', '4k')]:
    v = [q for s, c, q in rows if sel(s, c)]
    lo, hi = ci(v)
    print(f'{lab:6} n={len(v):2} median {statistics.median(v):.4f} [{lo:.4f}, {hi:.4f}] faster {sum(x < 1 for x in v)}/{len(v)}')
