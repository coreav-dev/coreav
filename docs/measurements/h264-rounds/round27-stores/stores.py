"""stores.py BIN STREAM NMB [N] [BY]: bytes stored per macroblock by source line (BY=line) or function (BY=fn) of
our code (innermost inline frame in a CoreAV crate, as r26/addr.py), from callgrind per-instruction Dw (cg/STREAM.pkl
of r26) times the store's width read from the disassembly. Also D1/LL write misses per macroblock (lines of 64 bytes
brought in by stores). NMB = macroblocks decoded in the callgrind run."""
import sys, re, os, pickle, subprocess, collections
R = '$HOME/h264work/r26/'; D = '$HOME/h264work/r27/'
B, s, NMB = sys.argv[1], sys.argv[2], int(sys.argv[3]); N = int(sys.argv[4]) if len(sys.argv) > 4 else 40
BY = sys.argv[5] if len(sys.argv) > 5 else 'line'
name = os.path.basename(B)
W = {'byte': 1, 'word': 2, 'dword': 4, 'qword': 8, 'tbyte': 10, 'xmmword': 16, 'ymmword': 32, 'zmmword': 64}
SUF = {'b': 1, 'w': 2, 'd': 4, 'q': 8}

def widths():
    cache = f'{D}{name}.widths'
    if os.path.exists(cache): return pickle.load(open(cache, 'rb'))
    w = {}
    p = subprocess.Popen(['llvm-objdump', '-d', '--no-show-raw-insn', '-M', 'intel', B], stdout=subprocess.PIPE, text=True)
    for l in p.stdout:
        m = re.match(r'\s+([0-9a-f]+):\s+(\S+)\s*(.*)', l)
        if not m: continue
        a, mn, ops = int(m.group(1), 16), m.group(2), m.group(3)
        if mn in ('rep', 'repe', 'repne'):
            mn, _, ops = ops.partition(' ')
        if mn.startswith(('push', 'call')): w[a] = 8; continue
        if mn.startswith(('movs', 'stos')) and mn[-1] in SUF and 'ptr' not in ops: w[a] = SUF[mn[-1]]; continue
        mm = re.search(r'(\w+) ptr \[', ops)
        if mm and mm.group(1) in W: w[a] = W[mm.group(1)]
    pickle.dump(w, open(cache, 'wb')); return w

def key(fr):
    for fn, fl in fr:
        m = re.search(r'(coreav-[a-z0-9-]+)/src/(\S+?):(\d+)', fl)
        if m and m.group(3) != '0':
            f = re.sub(r'<.*', '', fn.split('::')[-1] if '::' in fn else fn)
            if BY == 'fn':
                parts = [x for x in re.sub(r'<[^<>]*>', '', re.sub(r'<[^<>]*>', '', fn)).split('::') if x]
                return '::'.join(parts[-2:]), m.group(2)
            return f'{m.group(2)}:{m.group(3)}', f
    return ('other', '')

C = pickle.load(open(f'{R}cg/{s}.pkl', 'rb')); F = pickle.load(open(f'{R}{name}.a2l', 'rb')); Wd = widths()
agg = collections.defaultdict(lambda: [0, 0, 0, 0]); nowidth = 0; tot = [0, 0, 0, 0]
for a, r in C.items():
    dw = r[2]
    if not dw: continue
    if a < 0: k = ('outside (libc)', ''); wd = 0
    else:
        k = key(F.get(a, [])); wd = Wd.get(a, 0)
        if not wd: nowidth += dw
    row = agg[k]; row[0] += dw; row[1] += dw * wd; row[2] += r[5]; row[3] += r[8]
    for i, v in enumerate((dw, dw * wd, r[5], r[8])): tot[i] += v
print(f'{s}: {NMB} macroblocks; per macroblock: {tot[0] / NMB:.0f} stores, {tot[1] / NMB:.0f} bytes (our code), '
      f'{tot[2] / NMB:.1f} D1 write misses, {tot[3] / NMB:.2f} LL write misses; stores of unknown width {nowidth / max(tot[0], 1):.2%}')
print(f'{"where":40} {"stores":>7} {"bytes":>7} {"D1mw":>6} {"LLmw":>6}   (per macroblock)')
for (k, f), r in sorted(agg.items(), key=lambda kv: -kv[1][1])[:N]:
    print(f'{k[:40]:40} {r[0] / NMB:7.1f} {r[1] / NMB:7.1f} {r[2] / NMB:6.2f} {r[3] / NMB:6.3f}   {f[:30]}')
