"""resolve2.py RUNDIR BIN DSO -> RUNDIR/addr.<DSO>.json: each sampled file address of BIN (dso name DSO) -> its inline
chain [[function, file], ...], innermost first, by llvm-addr2line --inlines (debug info only)."""
import glob, json, os, subprocess, sys
d, binary, dso = sys.argv[1:4]
addrs = set()
for f in glob.glob(f'{d}/*.json'):
    if os.path.basename(f).startswith('addr.'): continue
    for name, a, n in json.load(open(f))['samples']:
        if name == dso: addrs.add(a)
addrs = sorted(addrs)
delta = 0
for l in subprocess.run(['readelf', '-lW', binary], capture_output=True, text=True).stdout.splitlines():
    p = l.split()
    if p and p[0] == 'LOAD' and 'E' in p[6:-1]:
        delta = int(p[2], 16) - int(p[1], 16)
out = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', binary],
                     input='\n'.join(hex(a + delta) for a in addrs) + '\n', capture_output=True, text=True).stdout
res = {}
for a, b in zip(addrs, out.split('\n\n')):
    ls = [l for l in b.strip().split('\n') if l]
    res[str(a)] = [[ls[i], ls[i + 1].rsplit(':', 2)[0]] for i in range(0, len(ls) - 1, 2)]
json.dump(res, open(f'{d}/addr.{dso}.json', 'w'))
print(dso, len(addrs), len(res))
