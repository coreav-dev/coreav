"""instr.py instructions.txt: per class, each build's instructions over main's (median, min, max)."""
import collections, statistics as st, sys
d = collections.defaultdict(dict)
for l in open(sys.argv[1]):
    if l.startswith('#'): continue
    s, b, n = l.split(); d[s][b] = int(n)
for b in ('r33-ct', 'r33-ct2'):
    r = {s: v[b] / v['m-b366429'] for s, v in d.items()}
    for c in ('sd', '720p', '1080p', '4k'):
        x = [v for s, v in r.items() if s.endswith('.' + c)]
        print(f'{b:8} {c:6} median {st.median(x):.4f} min {min(x):.4f} max {max(x):.4f}')
    print(f'{b:8} all    median {st.median(r.values()):.4f} max {max(r.values()):.4f}, {sum(v < 1 for v in r.values())} of {len(r)} below 1')
