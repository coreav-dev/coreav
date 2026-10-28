"""klines.py CORE_DIR SYMRE [N] (BIN: the profiled binary): samples of symbols matching SYMRE by innermost frame file:line (and fn), share of the symbol's samples."""
import collections, glob, json, os, re, sys, subprocess
cd, symre = sys.argv[1], re.compile(sys.argv[2]); N = int(sys.argv[3]) if len(sys.argv) > 3 else 40
R = json.load(open(f'{cd}/addr.prof-main.json'))
cnt = collections.Counter(); addrs = collections.Counter()
for p in glob.glob(f'{cd}/*.core.*.json'):
    for name, a, n in json.load(open(p))['samples']:
        if name != 'prof-main': continue
        ch = R.get(str(a), [])
        if ch and symre.search(ch[-1][0]): addrs[a] += n
tot = sum(addrs.values())
binary = os.environ.get('BIN', 'prof-main')
lines = collections.Counter(); fnl = {}
ks = sorted(addrs)
delta = 0
for l in subprocess.run(['readelf', '-lW', binary], capture_output=True, text=True).stdout.splitlines():
    p = l.split()
    if p and p[0] == 'LOAD' and 'E' in p[6:-1]: delta = int(p[2], 16) - int(p[1], 16)
out = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '-f', '-C', '-e', binary], input='\n'.join(hex(a + delta) for a in ks) + '\n', capture_output=True, text=True).stdout
for a, b in zip(ks, out.split('\n\n')):
    ls = [l for l in b.strip().split('\n') if l]
    loc = ls[1].rsplit('/', 1)[-1] if len(ls) > 1 else '?'
    loc = ':'.join(loc.split(':')[:2])
    lines[loc] += addrs[a]; fnl[loc] = re.sub(r'<[^<>]*>', '', ls[0])[-50:]
print('total samples', tot)
for k, v in lines.most_common(N): print(f'{100*v/tot:5.1f}% {k:28} {fnl[k]}')
