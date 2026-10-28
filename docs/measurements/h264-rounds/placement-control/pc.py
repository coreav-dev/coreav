"""pc.py AB.TXT: the placement-control bars (plan.txt) from ab.py's output with the arms
d-base f-base d-copy f-copy d-n1 f-n1 d-n2 f-n2 d-dk f-dk (d-base first)."""
import random, statistics, sys
CAVLC = ('5185721dda11', '6f7799768413', '9db41aa16b5a', 'b71b6b31e54f', 'ec473c99c538', 'f9e973f44570')
rows = []
for l in open(sys.argv[1]):
    p = l.split()
    if len(p) < 3 or p[0] == 'median': continue
    r = {p[i]: float(p[i + 1]) for i in range(2, len(p), 2)}; r['d-base'] = 1.0
    rows.append((p[0][:12], r))
def ci(v):
    random.seed(1); bs = sorted(statistics.median(random.choices(v, k=len(v))) for _ in range(4000)); return bs[100], bs[3899]
def pair(m, c):
    """Per-stream ratios of m-c over m-base; dk on the 26 CABAC streams only (its code differs on the CAVLC path)."""
    return [r[f'{m}-{c}'] / r[f'{m}-base'] for s, r in rows if c != 'dk' or s not in CAVLC]
res = {}
for m in ('d', 'f'):
    for c in ('copy', 'n1', 'n2', 'dk'):
        v = pair(m, c); M = statistics.median(v); S = statistics.median(abs(x - 1) for x in v); lo, hi = ci(v)
        res[m, c] = (M, S)
        print(f'{m}-{c:4} over {m}-base: n={len(v):2} median {M:.4f} [{lo:.4f}, {hi:.4f}]  spread S {S:.4f}  '
              f'range {min(v):.4f} to {max(v):.4f}  faster {sum(x < 1 for x in v)}/{len(v)}')
NO = ('n1', 'n2', 'dk')
v = [r['f-base'] for s, r in rows]; lo, hi = ci(v)
print(f'f-base over d-base (information): median {statistics.median(v):.4f} [{lo:.4f}, {hi:.4f}], faster {sum(x < 1 for x in v)}/{len(v)}')
for m in ('d', 'f'):
    v = [r[f'{m}-dk'] / r[f'{m}-base'] for s, r in rows if s in CAVLC]
    print(f'{m}-dk over {m}-base on the six CAVLC streams (information): median {statistics.median(v):.4f}')
Sd = statistics.mean(res['d', c][1] for c in NO); Sf = statistics.mean(res['f', c][1] for c in NO)
Ad, Af = res['d', 'copy'][1], res['f', 'copy'][1]
print(f'mean no-op spread: d {Sd:.4f} (A/A {Ad:.4f}), f {Sf:.4f} (A/A {Af:.4f})')
valid = all(abs(res[m, 'copy'][0] - 1) <= 0.002 for m in ('d', 'f'))
pn0 = Sd >= 1.5 * Ad
pn1 = Sf - Af <= 0.5 * (Sd - Ad)
pn2 = max(abs(res['f', c][0] - 1) for c in NO) <= 0.0025
print(f'validity (A/A medians within 0.002 of 1): {"met" if valid else "NOT met: host fault, void"}')
print(f'PN-0 (premise: d no-op spread >= 1.5 x d A/A): {"met" if pn0 else "not met: inconclusive"}')
print(f'PN-1 (f removes at least half of d excess spread): {"met" if pn1 else "not met"} '
      f'(f excess {Sf - Af:.4f}, half of d excess {0.5 * (Sd - Ad):.4f})')
print(f'PN-2 (each f no-op median within 0.0025 of 1): {"met" if pn2 else "not met"} '
      f'(largest {max(abs(res["f", c][0] - 1) for c in NO):.4f}; d largest {max(abs(res["d", c][0] - 1) for c in NO):.4f})')
print('verdict:', 'void' if not valid else 'inconclusive' if not pn0 else 'placement noise reduced' if pn1 and pn2 else 'not reduced')
