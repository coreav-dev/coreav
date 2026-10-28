"""map.py RES CORE_BIN STREAM [MB] [N]: cycles and instructions a macroblock by function, CoreAV by its symbols and
FFmpeg by the attribution build's function symbols (assembly's local labels, `.loop` and the like, are not functions:
a sample there goes to the function before it, as in stages_pmu.py)."""
import json, bisect, collections, re, glob, subprocess, sys
res, core, stream = sys.argv[1], sys.argv[2], sys.argv[3]
MB = int(sys.argv[4]) if len(sys.argv) > 4 else 1800 * 1200
def delta(lines):
    for l in lines:
        p = l.split()
        if p and p[0] == 'LOAD' and 'E' in p[6:-1]: return int(p[2], 16) - int(p[1], 16)
    return 0
def table(nm_lines, labels):
    s = []
    for l in nm_lines:
        p = l.rstrip('\n').split(' ', 2)
        if len(p) == 3 and p[1] in 'tTwW' and (labels or '.' not in p[2]): s.append((int(p[0], 16), p[2]))
    s.sort(); return s, [a for a, _ in s]
ff, ffa = table(open(f'{res}/ffattr.nm.txt'), False)
cs, csa = table(subprocess.run(['nm', '-C', '--defined-only', core], capture_output=True, text=True).stdout.splitlines(), True)
fd = delta(open(f'{res}/ffattr.readelf-l.txt'))
cd = delta(subprocess.run(['readelf', '-lW', core], capture_output=True, text=True).stdout.splitlines())
def look(s, sa, a):
    i = bisect.bisect_right(sa, a) - 1; return s[i][1] if i >= 0 else '??'
out = {}
for side in ('core', 'ffa'):
    files = sorted(glob.glob(f'{res}/t1/rec/{stream}.{side}.ipc.*.json'))
    agg = {'cycles': collections.Counter(), 'instructions': collections.Counter()}
    addrs = collections.Counter()
    for p in files:
        for ev, rows in json.load(open(p)).items():
            for dso, a, n in rows:
                if side == 'core' and dso.startswith('prof'):
                    f = look(cs, csa, a + cd); addrs[a] += 0
                elif side == 'ffa' and dso == 'ffattr':
                    f = look(ff, ffa, a + fd)
                else:
                    f = dso
                agg[ev][f] += n / len(files)
    out[side] = agg
    tc = sum(agg['cycles'].values())
    print(f'== {side}: {tc / MB:.0f} cycles a macroblock, {sum(agg["instructions"].values()) / MB:.0f} instructions')
    for f, c in agg['cycles'].most_common(int(sys.argv[5]) if len(sys.argv) > 5 else 30):
        i = agg['instructions'][f]
        g = re.sub(r'coreav_h264::|coreav_kernels_h264::|core::result::Result<[^>]*>, ', '', f)[:95]
        print(f'{c / MB:7.1f} cyc {i / MB:7.1f} ins  ipc {i / max(c, 1):4.2f}  {g}')
