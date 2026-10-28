"""counts.py counts.txt: per stream, banded lookups at 8T and declines by reason (top/bottom slack, other)."""
import sys, collections
rows = collections.OrderedDict(); cur = None
for l in open(sys.argv[1]):
    if l.startswith('=='): cur = l.split()[1]; rows[cur] = collections.Counter(); continue
    _, k, n = l.split(); rows[cur][int(k)] += int(n)
print(f"{'stream':28s} {'banded':>8s} {'declined':>8s} {'slack':>8s} {'other':>6s} slack%")
for f, d in rows.items():
    ok = d[0]; dec = {k - 512: v for k, v in d.items() if k >= 512}
    tot = ok + d[256] + sum(dec.values())
    slack = sum(v for k, v in dec.items() if k & 6 and not k & 1)
    other = sum(dec.values()) - slack
    print(f"{f:28s} {tot:8d} {sum(dec.values()):8d} {slack:8d} {other:6d} {100*slack/max(tot,1):5.1f}")
