"""stages_pmu.py RES CORE_BIN (a copy of ../pmu-profile/stages_pmu.py with the parity tables and items of every
stage added): hardware events of CoreAV and FFmpeg 9.0.2 by decoding stage, from rec_pmu.py's
output RES (stat/, rec/, ffattr.nm.txt, ffattr.readelf-l.txt) and the CoreAV binary that was profiled (line tables).

- Totals (perf stat): per class and over the 32 streams, the median per stream of CoreAV over the BtbN build for each
  event, and each decoder's events per macroblock.
- Stages (perf record, weights = summed sample periods): a CoreAV sample's stage is ../profile-6c9e240/classify.py's
  `core_stage` of its inline chain (llvm-addr2line --inlines on CoreAV's own line tables), unchanged; an FFmpeg sample's
  is FF_RULES below on the name of the function symbol at or before it in the attribution build's symbol table (no
  debug information, so a function stands for everything inlined into it). Each stream counts equally: a cycle-like
  event (cycles and the cycle counts of stalls) as a share of that stream's FFmpeg cycles in the same record, a count
  (misses, blocked loads, mispredicts) per macroblock.
- Items: within the MB layer and deblocking, CoreAV's functions (the innermost frame in a coreav- crate) by their share
  of each store and miss event.
Writes RES/stages.json and prints the tables."""
import ast, bisect, collections, glob, json, os, re, statistics, subprocess, sys

res, core_bin = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
STAGES = ['entropy', 'mb', 'inter', 'recon', 'deblock', 'other']
CYCLE_LIKE = ('cycles', 'exe_activity.bound_on_stores', 'resource_stalls.sb', 'cycle_activity.stalls_total',
              'cycle_activity.stalls_l1d_miss', 'cycle_activity.stalls_l2_miss', 'exe_activity.bound_on_loads',
              'resource_stalls.scoreboard', 'l1d_pend_miss.fb_full', 'l1d_pend_miss.l2_stall')

# The CoreAV classifier of the #459 profile, taken from its file rather than copied.
src = open(os.path.join(here, '..', 'profile-6c9e240', 'classify.py')).read()
fn = next(n for n in ast.parse(src).body if isinstance(n, ast.FunctionDef) and n.name == 'core_stage')
ns = {'re': re}
exec(compile(ast.Module(body=[fn], type_ignores=[]), 'classify.py', 'exec'), ns)
core_stage = ns['core_stage']

# FFmpeg: stage by function symbol name, first match wins.
FF_RULES = [
    ('deblock', r'loop_filter|deblock|filter_mb|h264_filter|check_mv|filter_mb_dir|_lf_|v_loop|h_loop'),
    ('inter', r'qpel|chroma_mc|chroma\d*_mc|_pixels|weight|emulated_edge|emu_edge|prefetch|mc_dir_part|mc_part|'
              r'hl_motion|_mc\d|put_h264|avg_h264'),
    ('recon', r'idct|pred4x4|pred8x8|pred16x16|intra_pred|h264_pred_init|pred_dc|pred_plane|pred_vert|pred_hor|'
              r'dequant|luma_dc|hl_decode_mb|add_pixels'),
    ('mb', r'pred_motion|pred_pskip|direct|fill_decode|fill_rectangle|decode_mb_skip|mvpred|xchg_mb_border|'
           r'backup_mb_border|h264_ref|ff_h264_build_ref|pred_16x8|pred_8x16|decode_slice$'),
    ('entropy', r'cabac|cavlc|decode_residual|significance|get_bits|golomb|h2645|nal|start_code|startcode|'
                r'ff_h264_decode_mb|decode_mb_cabac|decode_mb_cavlc|refill|get_ue|get_se|parse'),
]


def ff_stage(name):
    for st, rx in FF_RULES:
        if re.search(rx, name):
            return st
    return 'other'


def load_delta(readelf_lines):
    for l in readelf_lines:
        p = l.split()
        if p and p[0] == 'LOAD' and 'E' in p[6:-1]:
            return int(p[2], 16) - int(p[1], 16)
    return 0


dims = {}
for l in open(os.path.join(here, 'dims.txt')):
    f, d = l.split()
    w, h, n = (int(x) for x in d.split(','))
    s, c = f.split('.')[0], f.split('.')[2]
    dims[s] = ((w + 15) // 16) * ((h + 15) // 16) * min(n, FR[c])

# FFmpeg symbols.
syms = []
for l in open(f'{res}/ffattr.nm.txt'):
    p = l.split()
    # Assembly's local labels (`.loop`, `..@N.branch_instr`) are not functions: a sample there goes to the function
    # before it, as in ../profile/classify.py.
    if len(p) == 3 and p[1] in 'tTwW' and '.' not in p[2]:
        syms.append((int(p[0], 16), p[2]))
syms.sort()
sa = [a for a, _ in syms]
ff_delta = load_delta(open(f'{res}/ffattr.readelf-l.txt'))


def ff_func(a):
    i = bisect.bisect_right(sa, a + ff_delta) - 1
    return syms[i][1] if i >= 0 else '??'


# CoreAV inline chains.
recs = sorted(glob.glob(f'{res}/rec/*.json'))
core_dso = os.path.basename(core_bin)
addrs = set()
for p in recs:
    if '.core.' in p:
        for ev, rows in json.load(open(p)).items():
            addrs.update(a for d, a, n in rows if d == core_dso)
addrs = sorted(addrs)
c_delta = load_delta(subprocess.run(['readelf', '-lW', core_bin], capture_output=True, text=True).stdout.splitlines())
txt = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', core_bin],
                     input='\n'.join(hex(a + c_delta) for a in addrs) + '\n', capture_output=True, text=True).stdout
chains = {}
for a, b in zip(addrs, txt.split('\n\n')):
    ls = [l for l in b.strip().split('\n') if l]
    chains[a] = [[ls[i], ls[i + 1].rsplit(':', 2)[0]] for i in range(0, len(ls) - 1, 2)]


def core_item(ch):
    fr = next((f for f in ch if 'coreav-' in f[1]), None)
    if fr is None:
        return 'other'
    name = fr[0]
    while re.search(r'<[^<>]*>', name):
        name = re.sub(r'<[^<>]*>', '', name)
    return f"{fr[1].rsplit('/', 1)[-1]}:{name.removesuffix('::').split('::')[-1]}"


# Per (stream, class, decoder, event): stage weights, averaged over rounds; items for CoreAV.
st = collections.defaultdict(lambda: collections.defaultdict(float))
items = collections.defaultdict(lambda: collections.defaultdict(float))
per_item = collections.defaultdict(lambda: collections.defaultdict(float))
nrounds = collections.Counter()
for p in recs:
    s, c, dec, g, r = os.path.basename(p)[:-5].split('.')
    nrounds[(s, c, dec, g)] += 1
for p in recs:
    s, c, dec, g, r = os.path.basename(p)[:-5].split('.')
    k = 1 / nrounds[(s, c, dec, g)]
    for ev, rows in json.load(open(p)).items():
        ev = ev.split(':')[0]
        for d, a, n in rows:
            if dec == 'core':
                ch = chains.get(a, []) if d == core_dso else []
                stage = core_stage(ch) if d == core_dso else 'other'
                item = core_item(ch) if d == core_dso else f'[{d}]'
                if stage in ('mb', 'deblock'):
                    items[(ev, stage)][item] += n * k / max(1, len(dims))
            else:
                item = ff_func(a) if d == 'ffattr' else f'[{d}]'
                stage = ff_stage(item) if d == 'ffattr' else 'other'
            st[(s, c, dec, ev)][stage] += n * k
            per_item[(s, c, dec, ev, stage)][item] += n * k
events = sorted({k[3] for k in st})
streams = sorted({(k[0], k[1]) for k in st})
classes = ('sd', '720p', '1080p', '4k')


def norm(s, c, dec, ev):
    w = st.get((s, c, dec, ev), {})
    if ev in CYCLE_LIKE:
        base = sum(st.get((s, c, 'ffa', 'cycles'), {}).values())
        return {x: w.get(x, 0) / base for x in STAGES} if base else None
    return {x: w.get(x, 0) / dims[s] for x in STAGES}


out = {'stages': {}, 'totals': {}, 'items': {}}
print('Stages: cycle-like events as a share of FFmpeg cycles; counts per macroblock. Mean over streams.')
for ev in events:
    for lab in classes + ('all',):
        ss = [(s, c) for s, c in streams if lab in ('all', c)]
        row = {}
        for dec in ('core', 'ffa'):
            v = [norm(s, c, dec, ev) for s, c in ss]
            v = [x for x in v if x]
            row[dec] = {x: statistics.mean(y[x] for y in v) for x in STAGES} if v else {}
        out['stages'][f'{ev}|{lab}'] = row
    print(f'\n{ev}')
    print(f"  {'class':6} {'decoder':5} " + ' '.join(f'{x:>9}' for x in STAGES) + f" {'total':>9}")
    for lab in classes + ('all',):
        for dec in ('core', 'ffa'):
            r = out['stages'][f'{ev}|{lab}'][dec]
            if r:
                print(f"  {lab:6} {dec:5} " + ' '.join(f'{r[x]:9.4f}' for x in STAGES) + f" {sum(r.values()):9.4f}")

# Totals from perf stat, rounds averaged.
tot = collections.defaultdict(lambda: collections.defaultdict(list))
for p in glob.glob(f'{res}/stat/*.json'):
    s, c, dec, r = os.path.basename(p)[:-5].split('.')
    for ev, v in json.load(open(p)).items():
        if '#' not in ev and v is not None:
            tot[(s, c, dec)][ev].append(v)
tev = sorted({ev for d in tot.values() for ev in d})
print('\nTotals (perf stat): median per stream of CoreAV / FFmpeg BtbN; per macroblock: CoreAV, BtbN, attribution build')
for ev in tev:
    for lab in classes + ('all',):
        ss = [(s, c) for s, c in streams if lab in ('all', c)]
        m = lambda s, c, d: statistics.mean(tot[(s, c, d)][ev]) if tot[(s, c, d)][ev] else None
        rat = [m(s, c, 'core') / m(s, c, 'ffb') for s, c in ss if m(s, c, 'core') and m(s, c, 'ffb')]
        pm = {d: statistics.mean(m(s, c, d) / dims[s] for s, c in ss if m(s, c, d) is not None) for d in ('core', 'ffb', 'ffa')
              if any(m(s, c, d) is not None for s, c in ss)}
        out['totals'][f'{ev}|{lab}'] = {'ratio_median': statistics.median(rat) if rat else None, 'per_mb': pm}
        if lab == 'all' or ev in ('cycles', 'instructions'):
            print(f"  {ev:34} {lab:5} core/ffb {statistics.median(rat) if rat else float('nan'):6.3f}  per MB: "
                  + ' '.join(f"{d} {v:10.1f}" for d, v in pm.items()))

print('\nCoreAV items within the MB layer and deblocking (mean per stream, raw weight; top 12 per event):')
for (ev, stage), d in sorted(items.items()):
    tot_w = sum(d.values())
    top = sorted(d.items(), key=lambda x: -x[1])[:12]
    out['items'][f'{ev}|{stage}'] = top
    print(f'\n{ev} / {stage}')
    for name, w in top:
        print(f'  {100 * w / tot_w:5.1f}%  {name}')
if 'instructions' in events and 'cycles' in events:
    # Instructions per cycle of each stage: the summed weights over the class's streams (a run that samples both).
    print('\nInstructions per cycle by stage (sampled together; summed over the streams):')
    print(f"  {'class':6} {'decoder':5} " + ' '.join(f'{x:>9}' for x in STAGES) + f" {'total':>9}")
    for lab in classes + ('all',):
        for dec in ('core', 'ffa'):
            ss = [(s, c) for s, c in streams if lab in ('all', c)]
            ins = {x: sum(st.get((s, c, dec, 'instructions'), {}).get(x, 0) for s, c in ss) for x in STAGES}
            cyc = {x: sum(st.get((s, c, dec, 'cycles'), {}).get(x, 0) for s, c in ss) for x in STAGES}
            ipc = {x: ins[x] / cyc[x] if cyc[x] else 0 for x in STAGES}
            out['stages'][f'ipc|{lab}|{dec}'] = ipc
            print(f"  {lab:6} {dec:5} " + ' '.join(f'{ipc[x]:9.2f}' for x in STAGES)
                  + f" {sum(ins.values()) / sum(cyc.values()):9.2f}")

# Parity: each stage's cycles as a share of FFmpeg's (the attribution build's cycles in the same kind of record, per
# stream, mean over the class's streams), ranked by the excess over parity, CoreAV's share less FFmpeg's.
NAMES = {'entropy': 'entropy (NAL, CABAC/CAVLC, MB syntax)', 'mb': 'MB layer (neighbours, motion derivation, caches)',
         'inter': 'inter prediction and reference fetch', 'recon': 'reconstruction (dequant, IDCT, intra)',
         'deblock': 'deblocking (strengths and filter)', 'other': 'other (setup, output, allocation, kernel, libc)'}
out['parity'] = {}
print('\nParity by stage: cycles as a share of FFmpeg\'s, mean over streams; excess = CoreAV - 1.00 x FFmpeg')
for lab in classes + ('all',):
    ss = [(s, c) for s, c in streams if lab in ('all', c)]
    cyc, ins = out['stages'][f'cycles|{lab}'], out['stages'].get(f'instructions|{lab}', {})
    ipc_c, ipc_f = out['stages'].get(f'ipc|{lab}|core', {}), out['stages'].get(f'ipc|{lab}|ffa', {})
    tc = out['totals'].get(f'cycles|{lab}', {}).get('ratio_median')
    core_t, ff_t = sum(cyc['core'].values()), sum(cyc['ffa'].values())
    exc = {x: cyc['core'][x] - cyc['ffa'][x] for x in STAGES}
    pos = sum(v for v in exc.values() if v > 0)
    print(f"\n{lab} (n={len(ss)}): sampled cycles, CoreAV {core_t:.3f} of FFmpeg's (mean); perf stat cycles, CoreAV over "
          f"the BtbN build {tc if tc is None else round(tc, 3)} (median per stream); excess {core_t - ff_t:+.3f}")
    print(f"  {'stage':50} {'CoreAV':>7} {'FFmpeg':>7} {'ratio':>6} {'excess':>7} {'of +':>5} "
          f"{'ins/MB C':>9} {'ins/MB F':>9} {'IPC C':>6} {'IPC F':>6}")
    rows = []
    for x in sorted(STAGES, key=lambda x: -exc[x]):
        r = {'core': cyc['core'][x], 'ffmpeg': cyc['ffa'][x],
             'ratio': cyc['core'][x] / cyc['ffa'][x] if cyc['ffa'][x] else None, 'excess': exc[x],
             'share_of_positive_excess': exc[x] / pos if pos and exc[x] > 0 else 0.0,
             'ins_per_mb_core': ins.get('core', {}).get(x), 'ins_per_mb_ffmpeg': ins.get('ffa', {}).get(x),
             'ipc_core': ipc_c.get(x), 'ipc_ffmpeg': ipc_f.get(x)}
        rows.append((x, r))
        f = lambda v, w, d: f'{v:{w}.{d}f}' if v is not None else ' ' * (w - 1) + '-'
        print(f"  {NAMES[x]:50} {r['core']:7.3f} {r['ffmpeg']:7.3f} {f(r['ratio'], 6, 2)} {r['excess']:+7.3f} "
              f"{100 * r['share_of_positive_excess']:4.0f}% {f(r['ins_per_mb_core'], 9, 0)} "
              f"{f(r['ins_per_mb_ffmpeg'], 9, 0)} {f(r['ipc_core'], 6, 2)} {f(r['ipc_ffmpeg'], 6, 2)}")
    out['parity'][lab] = dict(rows)
    # Within each stage, the functions of each side by their cycles as a share of FFmpeg's (mean over streams).
    for x, _ in rows:
        side = {}
        for dec in ('core', 'ffa'):
            acc = collections.defaultdict(float)
            for s, c in ss:
                base = sum(st.get((s, c, 'ffa', 'cycles'), {}).values())
                for name, w in per_item.get((s, c, dec, 'cycles', x), {}).items():
                    acc[name] += w / base / len(ss) if base else 0
            side[dec] = sorted(acc.items(), key=lambda y: -y[1])[:8]
        out['parity'][lab][x]['items'] = side
        print(f"    {x}: CoreAV functions | FFmpeg functions (cycles, share of FFmpeg's)")
        for i in range(max(len(side['core']), len(side['ffa']))):
            a = side['core'][i] if i < len(side['core']) else ('', 0)
            b = side['ffa'][i] if i < len(side['ffa']) else ('', 0)
            print(f"      {a[1]:6.4f} {a[0][:56]:56} | {b[1]:6.4f} {b[0][:56]}")
json.dump(out, open(f'{res}/stages.json', 'w'), indent=1)
