"""cg_core.py: CoreAV callgrind (instr level) -> chains; dump pickle of per-stream {addr: Ir} and chains."""
import sys, glob, os, pickle, subprocess
sys.path.insert(0, '/tmp/ent'); import cgparse
BIN = '/tmp/pmu/up/prof-b3664291'
per = {}
for p in sorted(glob.glob('/tmp/ent/cg/*.core.out')):
    s, c = os.path.basename(p).split('.')[:2]
    per[(s, c)] = cgparse.parse(p, 'prof-b3664291')
addrs = sorted({a for d in per.values() for a in d})
txt = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', BIN],
                     input='\n'.join(hex(a) for a in addrs) + '\n', capture_output=True, text=True).stdout
chains = {}
for a, b in zip(addrs, txt.split('\n\n')):
    ls = [l for l in b.strip().split('\n') if l]
    chains[a] = [(ls[i], ls[i + 1]) for i in range(0, len(ls) - 1, 2)]
pickle.dump((per, chains), open('/tmp/ent/core.pkl', 'wb'))
print(len(per), len(addrs))
