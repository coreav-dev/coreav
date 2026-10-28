"""bound.py STREAM NMB: the deblocking setup probe's bound for one stream (plan.txt). Per source line of our code
(innermost inline frame in a CoreAV crate): perf cpu-clock time (pd/STREAM.*.data, AVX2 kernels; pdd/STREAM.*.data,
detected kernels, total only) against callgrind per instruction (cg/STREAM.out: Ir, Dw, D1mw). Prints one JSON line.
D (env) is the work directory, BIN the profiled binary. Caches beside the inputs. Adapted from the MB layout probe's
mbs.py (its readers of perf, callgrind and addr2line output are unchanged)."""
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

def short(fn):
    fn = re.sub(r'<[^<>]*>', '', re.sub(r'<[^<>]*>', '', re.sub(r'<[^<>]*>', '', fn)))
    return [x for x in fn.split('::') if x][-1:] or ['?']

def ours(fr):
    """(file, line, innermost fn) of the innermost frame in a CoreAV crate."""
    for fn, fl in fr:
        m = re.search(r'(coreav-[a-z0-9-]+)/src/(\S+?):(\d+)', fl)
        if m and m.group(3) != '0':
            return m.group(2), int(m.group(3)), short(fn)[0]
    return None

# plan.txt: R by the line of the deblock_row frame (deblock.rs at 145232d), K by kernel function or line.
R_LINES = {**{l: 'R:luma windows' for l in (566, 571, 572, 575, 576)}, 567: 'R:luma params',
           **{l: 'R:chroma params' for l in (591, 613, 614, 615)},
           **{l: 'R:chroma windows' for l in (622, 630, 631, 632)}}
K_FNS = {('x86.rs', f) for f in ('segment_bytes', 'spread8', 'spread16', 'per_line8', 'per_line16', 'lane_values')} | \
        {('x86/chroma_mb.rs', f) for f in ('lanes', 'filters')}
K_LINES = {('x86.rs', l) for l in (3927, 3928, 4228, 4229, 4230, 4231, 4232, 4233, 4234, 4235, 4345, 4346)}

def group(a, F):
    """'R:<part>', 'K' or None for an address."""
    fr = F.get(a, []) if a >= 0 else []
    o = ours(fr)
    if not o: return None
    f, ln, fn = o
    for name, fl in fr:
        m = re.search(r'coreav-h264/src/deblock\.rs:(\d+)', fl)
        if short(name)[0] == 'deblock_row' and m:
            return R_LINES.get(int(m.group(1)))
    # out-of-line instances count in full
    if f == 'views.rs' and fn == 'new' and (246 <= ln <= 286): return 'R:luma windows'
    if f == 'views.rs' and fn == 'new' and (395 <= ln <= 413): return 'R:chroma windows'
    if f == 'deblock.rs' and fn == 'params': return 'R:luma params'
    if f.startswith('simd/') and ((f[5:], fn) in K_FNS or (f[5:], ln) in K_LINES): return 'K'
    return None

def detected_ms():
    tot = 0; n = 0
    for p in sorted(glob.glob(f'{D}pdd/{s}.*.data')):
        n += 1
        c = subprocess.run(['perf', 'script', '-F', 'ip', '-i', p], capture_output=True, text=True).stdout.count('\n')
        tot += c
    return tot / n / 10.0

T = perf_ips(); C = cg_instr(); F = frames(set(T) | set(C))
TT = sum(T.values()); IT = sum(r[0] for r in C.values()); TD = detected_ms()
line = collections.defaultdict(lambda: [0.0, 0, 0, 0, None])  # time ms, Ir, Dw, D1mw, group
for a, v in T.items():
    o = ours(F.get(a, [])) if a >= 0 else None
    g = group(a, F)
    k = (o[0], o[1], g) if o else ('other', a if a < 0 else 0, None)
    line[k][0] += v; line[k][4] = g
for a, r in C.items():
    o = ours(F.get(a, [])) if a >= 0 else None
    g = group(a, F)
    k = (o[0], o[1], g) if o else ('other', a if a < 0 else 0, None)
    L = line[k]; L[1] += r[0]; L[2] += r[1]; L[3] += r[2]; L[4] = g
tot_mw = sum(L[3] for L in line.values())
parts = collections.defaultdict(lambda: [0.0, 0.0, 0.0, 0, 0])  # instr share, excess+, time share, Ir, D1mw
store_exc = 0.0
for k, L in line.items():
    t, i = L[0] / TT, L[1] / IT; e = max(0.0, t - i)
    if L[2] > 0: store_exc += e
    if L[4]:
        P = parts[L[4]]; P[0] += i; P[1] += e; P[2] += t; P[3] += L[1]; P[4] += L[3]
I = sum(p[0] for p in parts.values()); Exc = sum(p[1] for p in parts.values()); Mw = sum(p[4] for p in parts.values())
W = Mw / tot_mw if tot_mw else 0.0
S = max(Exc, W * store_exc); scale = TT / TD
out = {'stream': s, 'ms': TT, 'ms_detected': TD, 'nmb': NMB, 'I': 100 * I, 'S': 100 * S, 'bound': 100 * (I + S),
       'bound_detected': 100 * (I + S) * scale, 'bound_no_store_term_detected': 100 * (I + Exc) * scale,
       'W': W, 'excess': 100 * Exc, 'store_excess_all': 100 * store_exc,
       'parts': {g: {'instr%': 100 * p[0], 'excess%': 100 * p[1], 'time%': 100 * p[2],
                     'detected%': 100 * (p[0] + p[1]) * scale, 'ir_mb': p[3] / NMB, 'd1mw_mb': p[4] / NMB}
                 for g, p in sorted(parts.items())}}
print(json.dumps(out))
