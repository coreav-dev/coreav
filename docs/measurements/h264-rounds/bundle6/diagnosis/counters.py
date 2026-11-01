"""counters.py STAT.jsonl.gz [--streams]: each arm's counts over base's, from run.py's perf stat records.

Per stream and event, the median over rounds of each arm over the median of base; then the median over streams with a
95% bootstrap interval (4,000 resamples), for all 32 training streams, the 26 CABAC streams and the 6 CAVLC streams.
--streams also prints each stream's cycles ratios.
"""
import collections, gzip, json, random, statistics as st, sys

CAVLC = {'5185721dda11', '6f7799768413', '9db41aa16b5a', 'b71b6b31e54f', 'ec473c99c538', 'f9e973f44570'}
d = collections.defaultdict(list)
arms = []
for line in gzip.open(sys.argv[1], 'rt'):
    r = json.loads(line)
    if r['arm'] != 'base' and r['arm'] not in arms:
        arms.append(r['arm'])
    for row in r['rows']:
        p = row.split(',')
        try:
            v = float(p[0])
        except ValueError:
            continue
        d[(r['stream'], r['arm'], p[2])].append(v)
streams = sorted({k[0] for k in d})
events = sorted({k[2] for k in d})


def boot(x, n=4000, rng=random.Random(1)):
    m = sorted(st.median(rng.choices(x, k=len(x))) for _ in range(n))
    return st.median(x), m[int(.025 * n)], m[int(.975 * n)]


def ratio(s, arm, ev):
    return st.median(d[(s, arm, ev)]) / st.median(d[(s, 'base', ev)])


for ev in events:
    print('==', ev)
    for arm in arms:
        out = []
        for name, sel in [('all', streams), ('cabac', [s for s in streams if s[:12] not in CAVLC]),
                          ('cavlc', [s for s in streams if s[:12] in CAVLC])]:
            m, lo, hi = boot([ratio(s, arm, ev) for s in sel])
            out.append(f'{name} {m:.4f} [{lo:.4f},{hi:.4f}]')
        print(f'  {arm:4s}', ' | '.join(out))
if '--streams' in sys.argv:
    print('cycles:u per stream:', *arms)
    for s in streams:
        print(s, *[f'{ratio(s, a, "cycles:u"):.4f}' for a in arms])
