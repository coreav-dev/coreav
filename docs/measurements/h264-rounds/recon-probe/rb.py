"""rb.py STREAM: the reconstruction scaling probe's bound for one stream (plan.txt), mbs.py of ../mb-layout-probe/
with the removed set R of this plan and without the stores. Per source line of our code (innermost inline frame in a
CoreAV crate): perf cpu-clock time (pd/STREAM.*.data) against callgrind instructions (cg/STREAM.out). Prints one JSON
line. D (env) is the work directory, BIN the profiled binary. Caches beside the inputs."""
import sys, glob, subprocess, collections, re, os, pickle, json
D = os.environ.get("D", ".") + '/'; B = os.environ.get('BIN', D + 'prof-main')
s = sys.argv[1]; name = os.path.basename(B)
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
            ev = line.split()[1:]; idx = [ev.index('Ir'), ev.index('Ir'), ev.index('Ir')]; continue
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

R = ('new', 'apply', 'prepare_level_scales', 'chroma_qp')
def group(a, F):
    """'R:<fn>', 'glue' (the rest of recon.rs), 'K:intra4x4' or None for an address."""
    fr = F.get(a, []) if a >= 0 else []
    if any(short(fn)[0] == 'intra4x4_mb' for fn, _ in fr): return 'K:intra4x4'
    o = ours(fr)
    if not o or o[0] != 'recon.rs': return None
    return 'R:' + o[2] if o[2] in R else 'glue'

T = perf_ips(); C = cg_instr(); F = frames(set(T) | set(C))
TT = sum(T.values()); IT = sum(r[0] for r in C.values())
line = collections.defaultdict(lambda: [0.0, 0, None])  # time ms, Ir, group
for a, v in T.items():
    o = ours(F.get(a, [])) if a >= 0 else None
    g = group(a, F); k = (o[0], o[1], g) if o else ('other', a if a < 0 else 0, None)
    line[k][0] += v; line[k][2] = g
for a, r in C.items():
    o = ours(F.get(a, [])) if a >= 0 else None
    g = group(a, F); k = (o[0], o[1], g) if o else ('other', a if a < 0 else 0, None)
    line[k][1] += r[0]; line[k][2] = g
parts = collections.defaultdict(lambda: [0.0, 0.0, 0.0])  # max(t, i), t, i
for k, L in line.items():
    if not L[2]: continue
    t, i = L[0] / TT, L[1] / IT
    P = parts[L[2]]; P[0] += max(t, i); P[1] += t; P[2] += i
bound = sum(p[0] for g, p in parts.items() if g.startswith('R:'))
print(json.dumps({'stream': s, 'ms': TT, 'bound': 100 * bound,
                  'parts': {g: {'max%': 100 * p[0], 'time%': 100 * p[1], 'instr%': 100 * p[2]} for g, p in sorted(parts.items())}}))
