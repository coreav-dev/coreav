"""lines.py OUT BIN MBS FILTER: self Ir per MB by innermost source line, for instructions whose inline chain matches FILTER."""
import sys, subprocess, collections, re
sys.path.insert(0, '/tmp/ent'); import cgparse
out, binp, mbs, filt = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
d = cgparse.parse(out, binp.rsplit('/', 1)[-1])
addrs = sorted(d)
txt = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', binp],
                     input='\n'.join(hex(a) for a in addrs) + '\n', capture_output=True, text=True).stdout
acc = collections.Counter(); fnacc = collections.Counter(); tot = 0
for a, b in zip(addrs, txt.split('\n\n')):
    ls = [l for l in b.strip().split('\n') if l]
    ch = [(ls[i], ls[i + 1]) for i in range(0, len(ls) - 1, 2)]
    if not any(re.search(filt, f) for f, _ in ch): continue
    fn, loc = ch[0]
    loc = loc.split(':'); file = loc[0].rsplit('/', 1)[-1]; line = loc[1] if len(loc) > 1 else '?'
    short = re.sub(r'<[^<>]*>', '', re.sub(r'<[^<>]*>', '', fn)).split('::')[-1]
    acc[f'{file}:{line} {short}'] += d[a]; fnacc[f'{file} {short}'] += d[a]; tot += d[a]
print(f'total {tot/mbs:.1f} per MB')
for k, v in fnacc.most_common(25): print(f'{v/mbs:7.1f} {k}')
print('--- lines')
for k, v in acc.most_common(50): print(f'{v/mbs:7.1f} {k}')
