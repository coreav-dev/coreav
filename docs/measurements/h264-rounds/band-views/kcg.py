# kcg.py BASE.out K.out ...: per pair, total instructions, and self instructions and calls of reference_band and Interior::band
import re, sys
def load(p):
    names, cur, selfc, calls, total = {}, None, {}, {}, 0
    cfn = None; pending = False
    for l in open(p, errors='replace'):
        m = re.match(r'(c?fn)=\((\d+)\)(?: (.*))?', l)
        if m:
            k, i, n = m.groups()
            if n: names[i] = n.strip()
            if k == 'fn': cur = names.get(i, i)
            else: cfn = names.get(i, i)
            continue
        if l.startswith('summary:'): total = int(l.split()[1]); continue
        if l.startswith('calls='):
            calls[cfn] = calls.get(cfn, 0) + int(l.split('=')[1].split()[0]); pending = True; continue
        if pending: pending = False; continue  # the cost line of a call is inclusive, not self
        if l[:1].isdigit() or l[:1] in '+-*':
            parts = l.split()
            if len(parts) >= 2 and cur: selfc[cur] = selfc.get(cur, 0) + int(parts[1])
    return total, selfc, calls
def get(d, key): return sum(v for n, v in d.items() if n.endswith(key))
args = sys.argv[1:]
for b, k in zip(args[::2], args[1::2]):
    rows = []
    for p in (b, k):
        t, s, c = load(p)
        rows.append((t, get(s, '>::reference_band'), get(c, '>::reference_band'), get(s, 'Interior>::band'), get(c, 'Interior>::band')))
    (tb, rb, rbn, ib, ibn), (tk, rk, rkn, ik, ikn) = rows
    print(f'{b.split("/")[-1]} vs {k.split("/")[-1]}: total k/base {tk/tb:.3f} ({tb:,} -> {tk:,}); '
          f'reference_band {rb/max(rbn,1):.0f} -> {rk/max(rkn,1):.0f} a call ({rbn:,} -> {rkn:,} calls); '
          f'Interior::band {ib:,} -> {ik:,}')
