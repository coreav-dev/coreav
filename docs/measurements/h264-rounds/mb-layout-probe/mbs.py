"""mbs.py STREAM NMB: the MB layer layout probe's bound for one stream (plan.txt). Per source line of our code
(innermost inline frame in a CoreAV crate): perf cpu-clock time (pd/STREAM.*.data) against callgrind per instruction
(cg/STREAM.out: Ir, Dw, D1mw), stores' bytes from the disassembly. Prints one JSON line. D (env) is the work
directory, BIN the profiled binary. Caches beside the inputs."""
import sys, glob, subprocess, collections, re, os, pickle, json
D = os.environ.get("D", ".") + '/'; B = os.environ.get('BIN', D + 'prof-main')
s, NMB = sys.argv[1], int(sys.argv[2]); name = os.path.basename(B)
seg = [l.split() for l in subprocess.run(['readelf', '-lW', B], capture_output=True, text=True).stdout.splitlines()
       if l.strip().startswith('LOAD') and ' R E ' in l]
DELTA = int(seg[0][2], 16) - int(seg[0][1], 16)

def perf_ips():
    cache = f'{D}pd/{s}.ips'
    if os.path.exists(cache): return pickle.load(open(cache, 'rb'))
    c = collections.Counter(); n = 0
    for p in sorted(glob.glob(f'{D}pd/{s}.*.data')):
        n += 1
        mm = subprocess.run(['perf', 'script', '--show-mmap-events', '-i', p], capture_output=True, text=True).stdout
        m = re.search(r'\[(0x[0-9a-f]+)\((0x[0-9a-f]+)\) @ (0x[0-9a-f]+) [^\]]*\]: r-xp \S*' + re.escape(name), mm)
        start, size, pgoff = (int(x, 16) for x in m.groups())
        for line in subprocess.run(['perf', 'script', '-F', 'ip', '-i', p], capture_output=True, text=True).stdout.split():
            ip = int(line, 16)
            c[ip - start + pgoff + DELTA if start <= ip < start + size else -1] += 1
    r = {k: v / n / 10.0 for k, v in c.items()}  # ms a run at 10 kHz
    pickle.dump(r, open(cache, 'wb')); return r

def cg_instr():
    cache = f'{D}cg/{s}.pkl'
    if os.path.exists(cache): return pickle.load(open(cache, 'rb'))
    c = collections.defaultdict(lambda: [0, 0, 0]); other = [0, 0, 0]; ours = False; skip = False; last = 0; obs = {}
    idx = None
    for line in open(f'{D}cg/{s}.out'):
        if line.startswith('events:'):
            ev = line.split()[1:]; idx = [ev.index('Ir'), ev.index('Dw'), ev.index('D1mw')]; continue
        if line.startswith(('ob=', 'cob=')):
            m = re.match(r'c?ob=\((\d+)\)(?: (.*))?', line.strip())
            if m.group(2): obs[m.group(1)] = m.group(2)
            if line.startswith('ob='): ours = obs.get(m.group(1), '').endswith(name)
            continue
        if line.startswith('calls='): skip = True; continue
        if line[:1] not in '0123456789+-*': continue
        p = line.split(); a = p[0]
        if a.startswith('0x'): addr = int(a, 16)
        elif a.startswith('+'): addr = last + int(a[1:])
        elif a.startswith('-'): addr = last - int(a[1:])
        else: addr = last
        last = addr
        if skip: skip = False; continue
        v = p[2:]; row = c[addr] if ours else other
        for j, i in enumerate(idx):
            if i < len(v): row[j] += int(v[i])
    c = dict(c); c[-1] = other
    pickle.dump(c, open(cache, 'wb')); return c

def frames(addrs):
    cache = f'{D}{name}.a2l'
    known = pickle.load(open(cache, 'rb')) if os.path.exists(cache) else {}
    todo = sorted(a for a in addrs if a >= 0 and a not in known)
    if todo:
        out = subprocess.run(['llvm-addr2line', '-a', '-i', '-f', '-C', '-e', B], input='\n'.join(hex(a) for a in todo),
                             capture_output=True, text=True).stdout.splitlines()
        cur = None; i = 0
        while i < len(out):
            l = out[i]
            if l.startswith('0x'): cur = int(l, 16); known[cur] = []; i += 1; continue
            if l == '': i += 1; continue
            known[cur].append((l, out[i + 1] if i + 1 < len(out) else '??:0')); i += 2
        pickle.dump(known, open(cache, 'wb'))
    return known

def widths():
    cache = f'{D}{name}.widths'
    if os.path.exists(cache): return pickle.load(open(cache, 'rb'))
    W = {'byte': 1, 'word': 2, 'dword': 4, 'qword': 8, 'tbyte': 10, 'xmmword': 16, 'ymmword': 32, 'zmmword': 64}
    SUF = {'b': 1, 'w': 2, 'd': 4, 'q': 8}; w = {}
    p = subprocess.Popen(['llvm-objdump', '-d', '--no-show-raw-insn', '-M', 'intel', B], stdout=subprocess.PIPE, text=True)
    for l in p.stdout:
        m = re.match(r'\s+([0-9a-f]+):\s+(\S+)\s*(.*)', l)
        if not m: continue
        a, mn, ops = int(m.group(1), 16), m.group(2), m.group(3)
        if mn in ('rep', 'repe', 'repne'): mn, _, ops = ops.partition(' ')
        if mn.startswith(('push', 'call')): w[a] = 8; continue
        if mn.startswith(('movs', 'stos')) and mn[-1] in SUF and 'ptr' not in ops: w[a] = SUF[mn[-1]]; continue
        mm = re.search(r'(\w+) ptr \[', ops)
        if mm and mm.group(1) in W: w[a] = W[mm.group(1)]
    pickle.dump(w, open(cache, 'wb')); return w

def short(fn):
    fn = re.sub(r'<[^<>]*>', '', re.sub(r'<[^<>]*>', '', re.sub(r'<[^<>]*>', '', fn)))
    return [x for x in fn.split('::') if x][-1:] or ['?']

def ours(fr):
    """(file, line, innermost fn, next fn) of the innermost frame in a CoreAV crate."""
    for k, (fn, fl) in enumerate(fr):
        m = re.search(r'(coreav-[a-z0-9-]+)/src/(\S+?):(\d+)', fl)
        if m and m.group(3) != '0':
            nxt = short(fr[k + 1][0])[0] if k + 1 < len(fr) else ''
            return m.group(2), int(m.group(3)), short(fn)[0], nxt
    return None

def group(a, F):
    """'R:<part>', 'C' or None for an address."""
    o = ours(F.get(a, [])) if a >= 0 else None
    if not o: return None
    f, ln, fn, nxt = o
    if f == 'mb.rs' and fn == 'finish_mb': return 'R:finish_mb'
    if f == 'mb.rs' and fn in ('start_mb_body', 'start_mb') and 694 <= ln <= 698: return 'R:record init'
    if f == 'mb.rs' and fn == 'reset_body': return 'R:reset'
    if f == 'inter.rs' and fn == 'store_mb': return 'R:store_mb'
    if f == 'inter.rs' and fn == 'put_row' and nxt in ('put_rows', 'store_mb', 'store_motion'): return 'R:motion rows'
    if f == 'inter.rs' and fn in ('colocated', 'colocated_banded'): return 'C'
    return None

STORE_MB_COPY = 569  # inter.rs at 41ae60dc: `blocks.copy_within(0..4, y * 4)`
T = perf_ips(); C = cg_instr(); F = frames(set(T) | set(C)); Wd = widths()
TT = sum(T.values()); IT = sum(r[0] for r in C.values())
line = collections.defaultdict(lambda: [0.0, 0, 0, 0, 0, None])  # time ms, Ir, Dw, bytes, D1mw, group
for a, v in T.items():
    o = ours(F.get(a, [])) if a >= 0 else None
    g = group(a, F)
    k = (o[0], o[1], g) if o else ('other', a if a < 0 else 0, None)
    line[k][0] += v; line[k][5] = g
for a, r in C.items():
    o = ours(F.get(a, [])) if a >= 0 else None
    g = group(a, F)
    k = (o[0], o[1], g) if o else ('other', a if a < 0 else 0, None)
    L = line[k]; L[1] += r[0]; L[2] += r[1]; L[3] += r[1] * Wd.get(a, 0); L[4] += r[2]; L[5] = g
tot_mw = sum(L[4] for L in line.values()); tot_b = sum(L[3] for L in line.values())
parts = collections.defaultdict(lambda: [0.0, 0.0, 0.0, 0, 0])  # instr share, excess+, time share, bytes, D1mw
store_exc = 0.0
for k, L in line.items():
    t, i = L[0] / TT, L[1] / IT; e = max(0.0, t - i)
    if L[2] > 0: store_exc += e
    if L[5]:
        P = parts[L[5]]; P[0] += i; P[1] += e; P[2] += t; P[3] += L[3]; P[4] += L[4]
R = [p for g, p in parts.items() if g.startswith('R:')]
I = sum(p[0] for p in R); Rexc = sum(p[1] for p in R); Rmw = sum(p[4] for p in R)
W = Rmw / tot_mw if tot_mw else 0.0
S = max(Rexc, W * store_exc); Cb = parts['C'][1] if 'C' in parts else 0.0
out = {'stream': s, 'ms': TT, 'nmb': NMB, 'I': 100 * I, 'S': 100 * S, 'C': 100 * Cb, 'bound': 100 * (I + S + Cb),
       'W': W, 'R_excess': 100 * Rexc, 'store_excess_all': 100 * store_exc,
       'bytes_mb_all': tot_b / NMB, 'mw_mb_all': tot_mw / NMB,
       # store_mb's copy of rows 1 to 3 (144 bytes a call): the share of macroblocks stored with one motion
       'one_motion': sum(L[3] for (f, ln, g), L in line.items() if f == 'inter.rs' and g == 'R:store_mb'
                         and ln == STORE_MB_COPY) / 144 / NMB,
       'parts': {g: {'instr%': 100 * p[0], 'excess%': 100 * p[1], 'time%': 100 * p[2], 'bytes_mb': p[3] / NMB,
                     'd1mw_mb': p[4] / NMB} for g, p in sorted(parts.items())}}
print(json.dumps(out))
