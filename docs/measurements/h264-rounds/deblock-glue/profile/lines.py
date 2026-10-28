"""lines.py CORE_DIR CLASSES... (CORE_BIN: the profiled binary): deblock.rs/views.rs/lib.rs lines (innermost coreav frame) of deblocking-stage samples, with
the function, in time over each stream's BtbN FFmpeg total, mean per stream over the classes named."""
import collections, glob, json, os, subprocess, sys
cd = sys.argv[1]; want = set(sys.argv[2:])
binary = os.environ.get('CORE_BIN', '/tmp/dbp/prof-main')
R = json.load(open(f'{cd}/addr.prof-main.json'))
sys.argv = [sys.argv[0], cd, os.environ.get('FF_DIR', 'ff'), os.environ.get('FFMPEG_G', 'ffmpeg_g')]
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), '../../profile/classify.py')).read().split('syms = []')[0])
bt = collections.Counter(); bn = collections.Counter()
for p in glob.glob(f'{cd}/*.ff.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    bt[(s, c)] += sum(n for _, _, n in json.load(open(p))['samples']) / 4000; bn[(s, c)] += 1
bt = {k: v / bn[k] for k, v in bt.items()}
addrs = collections.Counter(); nk = set()
for p in glob.glob(f'{cd}/*.core.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    if c not in want: continue
    nk.add((s, c))
    for name, a, n in json.load(open(p))['samples']:
        if name != 'prof-main': continue
        if core_stage(R.get(str(a), [])) == 'deblock':
            addrs[a] += n / 4000 / bt[(s, c)] / bn[(s, c)]
delta = 0
for l in subprocess.run(['readelf', '-lW', binary], capture_output=True, text=True).stdout.splitlines():
    q = l.split()
    if q and q[0] == 'LOAD' and 'E' in q[6:-1]: delta = int(q[2], 16) - int(q[1], 16)
A = sorted(addrs)
out = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', binary],
                     input='\n'.join(hex(a + delta) for a in A) + '\n', capture_output=True, text=True).stdout
lines = collections.Counter()
for a, b in zip(A, out.split('\n\n')):
    ls = [l for l in b.strip().split('\n') if l]
    fr = [(ls[i], ls[i + 1]) for i in range(0, len(ls) - 1, 2)]
    f = next((x for x in fr if 'coreav-' in x[1]), None)
    if f is None: continue
    loc = f[1].split('/src/')[-1]
    lines[(loc.rsplit(':', 1)[0], f[0].split('::')[-1][:40])] += addrs[a]
n = len(nk)
for (loc, fn), v in sorted(lines.items(), key=lambda x: -x[1])[:45]:
    print(f'{v / n:.4f} {loc:30} {fn}')
