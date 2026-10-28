"""python3 -I mvd_subset.py: round 21's CABAC subset, fixed before the A/B run.

Context-coded mvd bins of each training stream (a value of prefix p codes
min(p + 1, 9) regular bins) from mvd-histograms.txt, per second of the
baseline build's CPU time in local/ab-whole.txt (a copy of round 20's) (main at c17ae06, one
thread). The subset is the half of the 26 CABAC streams with the most bins
per second: the streams where mvd_lX can weigh most."""
import os, re
here = os.path.dirname(os.path.abspath(__file__))
CAVLC = {'5185721dda11', '6f7799768413', '9db41aa16b5a', 'b71b6b31e54f', 'ec473c99c538', 'f9e973f44570'}
base = {}
for l in open(os.path.join(here, 'local', 'ab-whole.txt')):
    p = l.split()
    if p and not l.startswith(('#', 'median')):
        base[p[0][:12]] = float(p[1])
rows = []
for l in open(os.path.join(here, 'mvd-histograms.txt')):
    if l.startswith('#'):
        continue
    s, h = l.split()[0], [int(x) for x in re.findall(r'\d+', l.split('MVDHIST')[1])]
    if s in CAVLC:
        continue
    bins = sum(c * min(p + 1, 9) for p, c in enumerate(h))
    rows.append((bins / base[s] / 1e6, s, bins))
rows.sort(reverse=True)
for rate, s, bins in rows:
    print(f'{s} {bins:9d} bins, {rate:5.2f} M/s')
print('subset', ','.join(s for _, s, _ in rows[:13]))
