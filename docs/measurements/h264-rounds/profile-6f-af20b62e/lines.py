"""lines.py RES CORE STREAM SYMPAT [N]: CoreAV cycles and instructions by innermost source line inside symbols
matching SYMPAT."""
import json, bisect, collections, glob, subprocess, sys, re
res, core, stream, pat = sys.argv[1:5]; N = int(sys.argv[5]) if len(sys.argv) > 5 else 30
MB = 2160000  # 6f7799768413: 640x480, 40 x 30 macroblocks, 1800 frames
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
agg = {'cycles': collections.Counter(), 'instructions': collections.Counter()}
files = sorted(glob.glob(f'{res}/t1/rec/{stream}.core.ipc.*.json'))
for p in files:
    for ev, rows in json.load(open(p)).items():
        for dso, a, n in rows:
            if not dso.startswith('prof'): continue
            i = bisect.bisect_right(sa, a + cd) - 1
            if i >= 0 and pat in s[i][1]: agg[ev][a + cd] += n / len(files)
addrs = sorted(set(agg['cycles']) | set(agg['instructions']))
txt = subprocess.run(['llvm-addr2line', '--output-style=GNU', '-i', '-f', '-C', '-e', core],
                     input='\n'.join(hex(a) for a in addrs) + '\n', capture_output=True, text=True).stdout
# One query per address; addr2line separates them only by order, so ask again one at a time where needed.
by = {'cycles': collections.Counter(), 'instructions': collections.Counter()}
for a in addrs:
    out = subprocess.run(['llvm-addr2line', '-f', '-C', '-e', core, hex(a)], capture_output=True, text=True).stdout.split('\n')
    loc = out[1].rsplit('/', 1)[-1] if len(out) > 1 else '?'
    for ev in by: by[ev][loc] += agg[ev][a]
tc = sum(by['cycles'].values())
print(f'{tc / MB:.1f} cycles a macroblock in {pat}')
for k, c in by['cycles'].most_common(N):
    print(f'{c / MB:6.1f} cyc {by["instructions"][k] / MB:6.1f} ins  {k}')
