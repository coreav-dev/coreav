"""cg_ff.py: FFmpeg callgrind (function level): self Ir per function, and calls+inclusive Ir of get_cabac* per caller."""
import re, glob, os, collections, bisect, pickle
syms = []
for l in os.popen('nm /tmp/ent/ffsym'):
    p = l.split()
    if len(p) == 3 and p[1] in 'tTwW' and '.' not in p[2]: syms.append((int(p[0], 16), p[2]))
syms.sort(); sa = [a for a, _ in syms]
def fix(n):
    m = re.match(r'0x([0-9a-f]+)$', n)
    if m:
        i = bisect.bisect_right(sa, int(m.group(1), 16)) - 1
        return syms[i][1]
    return re.sub(r"'\d+$", '', n.split('.')[0]) if not n.startswith('..') else n
res = {}
for p in sorted(glob.glob('/tmp/ent/cg/*.ff.out')):
    s, c = os.path.basename(p).split('.')[:2]
    names = {}; selfc = collections.Counter(); calls = collections.Counter(); incl = collections.Counter()
    ob = cob = fn = cfn = None; skip = False; ok = False
    def nm(v, kind):
        m = re.match(r'\((\d+)\)(?: (.*))?', v.strip())
        if not m: return v.strip()
        k = (kind, m.group(1))
        if m.group(2): names[k] = m.group(2)
        return names.get(k, '?')
    for line in open(p):
        if line.startswith('ob='): ob = nm(line[3:], 'o'); continue
        if line.startswith('cob='): cob = nm(line[4:], 'o'); continue
        if line.startswith('fn='): fn = fix(nm(line[3:], 'n')); cob = None; continue
        if line.startswith('cfn='): cfn = fix(nm(line[4:], 'n')); continue
        if line.startswith('calls='): skip = int(line.split()[0][6:]); continue
        if not line[:1].isdigit() and line[:1] not in '+-*': continue
        pa = line.split()
        if len(pa) < 2: continue
        if skip is not False:
            if 'ffsym' in ob: calls[(fn, cfn)] += skip; incl[(fn, cfn)] += int(pa[1])
            skip = False; continue
        if 'ffsym' in ob: selfc[fn] += int(pa[1])
    res[(s, c)] = (selfc, calls, incl)
pickle.dump(res, open('/tmp/ent/ff.pkl', 'wb'))
k = next(iter(res)); print(k, res[k][0].most_common(8)); print([(a, b) for a, b in res[k][1].items() if 'get_cabac' in a[1]][:10])
