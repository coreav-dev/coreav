"""addr.py BIN STREAM [N] [FILTER]: per source line of our code (the innermost inline frame in a coreav crate), perf
cpu-clock time (pd/STREAM.*.data, mean ms a run) against callgrind per instruction (cg/STREAM.out, the same frames: Ir,
D1/LL misses, mispredicts), as shares of the run. Excess = time share - instruction share: time the line's
instructions do not explain. Largest excess first. Caches: pd/STREAM.ips (perf vaddr counts), BIN.a2l (address frames)."""
import sys, glob, subprocess, collections, re, os, pickle
D = '$HOME/h264work/r26/'
B, s = sys.argv[1], sys.argv[2]; N = int(sys.argv[3]) if len(sys.argv) > 3 else 30
FILT = sys.argv[4] if len(sys.argv) > 4 else ''
VG_BASE = 0  # callgrind reports this PIE's addresses as ELF virtual addresses
name = os.path.basename(B)
# Exec segment: vaddr - offset.
seg = [l.split() for l in subprocess.run(['readelf', '-lW', B], capture_output=True, text=True).stdout.splitlines()
       if l.strip().startswith('LOAD') and ' R E ' in l]
DELTA = int(seg[0][2], 16) - int(seg[0][1], 16)

def perf_ips():
    cache = f'{D}pd/{s}.{name}.ips'
    if os.path.exists(cache):
        return pickle.load(open(cache, 'rb'))
    c = collections.Counter(); n = 0
    for p in sorted(glob.glob(f'{D}pd/{s}.*.data')):
        n += 1
        mm = subprocess.run(['perf', 'script', '--show-mmap-events', '-i', p], capture_output=True, text=True).stdout
        m = re.search(r'\[(0x[0-9a-f]+)\((0x[0-9a-f]+)\) @ (0x[0-9a-f]+) [^\]]*\]: r-xp \S*' + re.escape(name), mm)
        start, size, pgoff = (int(x, 16) for x in m.groups())
        out = subprocess.run(['perf', 'script', '-F', 'ip', '-i', p], capture_output=True, text=True).stdout
        for line in out.split():
            ip = int(line, 16)
            if start <= ip < start + size:
                c[ip - start + pgoff + DELTA] += 1
            else:
                c[-1] += 1  # elsewhere: kernel, libc
    r = {k: v / n / 10.0 for k, v in c.items()}  # ms a run at 10 kHz
    pickle.dump(r, open(cache, 'wb')); return r

def cg_instr():
    cache = f'{D}cg/{s}.pkl'
    if os.path.exists(cache):
        return pickle.load(open(cache, 'rb'))
    c = collections.defaultdict(lambda: [0] * 13); ours = False; skip = False; last = 0; other = [0] * 13
    obs = {}
    for line in open(f'{D}cg/{s}.out'):
        if line.startswith(('ob=', 'cob=')):
            m = re.match(r'c?ob=\((\d+)\)(?: (.*))?', line.strip())
            if m.group(2): obs[m.group(1)] = m.group(2)
            if line.startswith('ob='): ours = obs.get(m.group(1), '').endswith(name)
            continue
        if line.startswith('calls='):
            skip = True; continue
        if not line[:1] in '0123456789+-*':
            continue
        p = line.split()
        a = p[0]
        if a.startswith('0x'): addr = int(a, 16)
        elif a.startswith('+'): addr = last + int(a[1:])
        elif a.startswith('-'): addr = last - int(a[1:])
        else: addr = last
        last = addr
        if skip:
            skip = False; continue  # the call's inclusive cost
        row = c[addr - VG_BASE] if ours else other
        for i, v in enumerate(p[2:15]): row[i] += int(v)
    c = dict(c); c[-1] = other
    pickle.dump(c, open(cache, 'wb')); return c

def frames(addrs):
    cache = f'{D}{name}.a2l'
    known = pickle.load(open(cache, 'rb')) if os.path.exists(cache) else {}
    todo = sorted(a for a in addrs if a >= 0 and a not in known)
    if todo:
        out = subprocess.run(['llvm-addr2line', '-a', '-i', '-f', '-C', '-e', B], input='\n'.join(hex(a) for a in todo),
                             capture_output=True, text=True).stdout
        cur = None
        lines = out.splitlines(); i = 0
        while i < len(lines):
            l = lines[i]
            if l.startswith('0x'):
                cur = int(l, 16); known[cur] = []; i += 1; continue
            if l == '':
                i += 1; continue
            fn = l; fl = lines[i + 1] if i + 1 < len(lines) else '??:0'; i += 2
            known[cur].append((fn, fl))
        pickle.dump(known, open(cache, 'wb'))
    return known

def key(fr):
    for fn, fl in fr:
        m = re.search(r'(coreav-[a-z0-9-]+)/src/(\S+?):(\d+)', fl)
        if m and m.group(3) != '0':
            f = re.sub(r'<.*', '', fn.split('::')[-1] if '::' in fn else fn)
            return f'{m.group(2)}:{m.group(3)}', f
    return ('other', '')

if __name__ == '__main__':
    T = perf_ips(); C = cg_instr(); F = frames(set(T) | set(C))
    TT = sum(T.values()); IT = sum(r[0] for r in C.values())
    agg = collections.defaultdict(lambda: [0.0] + [0] * 13)
    for a, v in T.items():
        agg[key(F.get(a, [])) if a >= 0 else ('outside', '')][0] += v
    for a, r in C.items():
        row = agg[key(F.get(a, [])) if a >= 0 else ('outside', '')]
        for i, v in enumerate(r): row[1 + i] += v
    print(f'{s}: {TT:.0f} ms a run; {IT / 1e6:.0f} M instructions; shares in %; in our binary: time '
          f'{100 * (1 - T.get(-1, 0) / TT):.1f}%, instr {100 * (1 - C[-1][0] / IT):.1f}%')
    print(f'{"line":34} {"fn":22} {"time%":>6} {"instr%":>6} {"excess":>6} {"rIPC":>5} {"D1m/kI":>7} {"LLm/kI":>7} {"misp/kI":>7}')
    rows = sorted(agg.items(), key=lambda kv: -(kv[1][0] / TT - kv[1][1] / IT))
    shown = 0
    for (ln, fn), r in rows:
        if FILT and not re.search(FILT, ln): continue
        t, i = r[0] / TT, r[1] / IT; kI = max(r[1], 1) / 1000
        # events: Ir Dr Dw I1mr D1mr D1mw ILmr DLmr DLmw Bc Bcm Bi Bim
        print(f'{ln:34} {fn[:22]:22} {100 * t:6.2f} {100 * i:6.2f} {100 * (t - i):6.2f} {(i / t if t else 0):5.2f} '
              f'{(r[5] + r[6]) / kI:7.1f} {(r[8] + r[9]) / kI:7.2f} {(r[11] + r[13]) / kI:7.1f}')
        shown += 1
        if shown >= N: break
