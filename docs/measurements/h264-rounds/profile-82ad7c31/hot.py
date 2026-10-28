"""hot.py RES_T CORE SYMPAT [STREAMGLOB]: CoreAV cycles by instruction address inside the symbols matching SYMPAT
(rec_pmu.py records RES_T/rec, the profiled binary CORE), each symbol's disassembly annotated with its share."""
import json, bisect, collections, glob, subprocess, sys
res, core, pat = sys.argv[1:4]; sg = sys.argv[4] if len(sys.argv) > 4 else '*'
s = []
for l in subprocess.run(['nm', '-C', '--defined-only', core], capture_output=True, text=True).stdout.splitlines():
    p = l.split(' ', 2)
    if len(p) == 3 and p[1] in 'tTwW': s.append((int(p[0], 16), p[2]))
s.sort(); sa = [a for a, _ in s]
def delta(lines):
    for l in lines:
        p = l.split()
        if p and p[0] == 'LOAD' and 'E' in p[6:-1]: return int(p[2], 16) - int(p[1], 16)
cd = delta(subprocess.run(['readelf', '-lW', core], capture_output=True, text=True).stdout.splitlines())
agg = collections.Counter(); tot = 0; syms = set()
for p in sorted(glob.glob(f'{res}/rec/{sg}.core.*.json')):
    for ev, rows in json.load(open(p)).items():
        if not ev.startswith('cycles'): continue
        for dso, a, n in rows:
            if not dso.startswith('prof'): continue
            tot += n
            i = bisect.bisect_right(sa, a + cd) - 1
            if i >= 0 and pat in s[i][1]: agg[a + cd] += n; syms.add(i)
sub = sum(agg.values()); print(f'share of CoreAV cycles in {pat}: {sub / tot:.4f}')
for i in sorted(syms):
    lo, hi = s[i][0], s[i + 1][0]
    print('==', s[i][1][:150], hex(lo), hi - lo, 'bytes', f'{sum(v for a, v in agg.items() if lo <= a < hi) / sub:.3f}')
    dis = subprocess.run(['objdump', '-d', '--no-show-raw-insn', f'--start-address={lo}', f'--stop-address={hi}', core],
                         capture_output=True, text=True).stdout.splitlines()
    for l in dis:
        p = l.strip().split(':', 1)
        try: a = int(p[0], 16)
        except ValueError: continue
        v = agg.get(a, 0)
        print(f'{100 * v / sub:5.1f} {l.strip()[:110]}')
