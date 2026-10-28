"""analyze.py RUN_DIR PROF_BIN: plan S2's decomposition (factors.txt) and our growth from one thread to eight by item
(items.txt), from a run of job.sh. Prints both; writes RUN_DIR/factors.txt, RUN_DIR/items.txt, RUN_DIR/items.json."""
import collections, glob, json, os, re, statistics as st, subprocess, sys

run, prof = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
CLASSES = ['sd', '720p', '1080p', '4k']
med = st.median
tim = json.load(open(f'{run}/timing.json'))
streams = sorted(tim, key=lambda f: (CLASSES.index(f.split('.')[2]), f))
cls = {f.split('.')[0]: f.split('.')[2] for f in streams}
ids = [f.split('.')[0] for f in streams]
CLASSES = [c for c in CLASSES if c in cls.values()]

# Timing factors.
fac = {}
for f in streams:
    s = f.split('.')[0]
    m = {a: (med(x[0] for x in v), med(x[1] + x[2] for x in v), med(x[3] for x in v), med(x[4] for x in v),
             med(x[2] for x in v)) for a, v in tim[f].items()}
    w1u, c1u = m['us1'][:2]; w1f, c1f = m['ff1'][:2]; w8u, c8u = m['us8'][:2]; w8f, c8f = m['ff8'][:2]
    d = {'r1': w1u / w1f, 'r8': w8u / w8f, 'W1': c1u / c1f, 'I': (c8u / c1u) / (c8f / c1f),
         'B': (c8f / w8f) / (c8u / w8u), 'busy_us': c8u / w8u, 'busy_ff': c8f / w8f,
         'scale_us': w1u / w8u, 'scale_ff': w1f / w8f, 'minflt8_us': m['us8'][2], 'minflt8_ff': m['ff8'][2],
         'csw8_us': m['us8'][3], 'csw8_ff': m['ff8'][3], 'sys8_us': m['us8'][4] / c8u, 'sys8_ff': m['ff8'][4] / c8f}
    d['IB'] = d['I'] * d['B']
    sts = {a: [json.load(open(p)) for p in sorted(glob.glob(f'{run}/stat/{s}.{a}.*.json'))] for a in tim[f]}
    if all(sts.values()) and all('cycles:u' in v[0] for v in sts.values()):
        cyc = {a: st.mean(x['cycles:u'] + x['cycles:k'] for x in v) for a, v in sts.items()}
        ins = {a: st.mean(x['instructions:u'] + x['instructions:k'] for x in v) for a, v in sts.items()}
        ghz = {a: st.mean((x['cycles:u'] + x['cycles:k']) / x['task-clock'] / 1e6 for x in v) for a, v in sts.items()}
        d['I_c'] = (cyc['us8'] / cyc['us1']) / (cyc['ff8'] / cyc['ff1'])
        d['I_i'] = (ins['us8'] / ins['us1']) / (ins['ff8'] / ins['ff1'])
        d['I_cpi'] = d['I_c'] / d['I_i']
        d['ins8/1_us'] = ins['us8'] / ins['us1']; d['ins8/1_ff'] = ins['ff8'] / ins['ff1']
        d['cyc8/1_us'] = cyc['us8'] / cyc['us1']; d['cyc8/1_ff'] = cyc['ff8'] / cyc['ff1']
        d['ghz1_us'], d['ghz8_us'], d['ghz1_ff'], d['ghz8_ff'] = ghz['us1'], ghz['us8'], ghz['ff1'], ghz['ff8']
    fac[s] = d
cols = ['r1', 'r8', 'W1', 'I', 'B', 'IB', 'I_c', 'I_i', 'I_cpi', 'cyc8/1_us', 'cyc8/1_ff', 'ins8/1_us', 'ins8/1_ff',
        'busy_us', 'busy_ff', 'scale_us', 'scale_ff', 'ghz1_us', 'ghz8_us', 'ghz1_ff', 'ghz8_ff', 'sys8_us', 'sys8_ff',
        'minflt8_us', 'minflt8_ff', 'csw8_us', 'csw8_ff']
cols = [c for c in cols if all(c in fac[s] for s in ids)]
out = ['class stream ' + ' '.join(cols)]
for s in ids:
    out.append(f'{cls[s]} {s} ' + ' '.join(f'{fac[s][c]:.3f}' for c in cols))
for c in CLASSES + ['all']:
    sel = [s for s in ids if c == 'all' or cls[s] == c]
    out.append(f'MEDIAN {c} ' + ' '.join(f'{med(fac[s][k] for s in sel):.3f}' for k in cols))
for c in CLASSES + ['all']:
    sel = [s for s in ids if c == 'all' or cls[s] == c]
    out.append(f'{c}: r8 faster {sum(fac[s]["r8"] < 1 for s in sel)}/{len(sel)}, r1 faster '
               f'{sum(fac[s]["r1"] < 1 for s in sel)}/{len(sel)}')
out.append('outliers (I*B >= 1.10): ' + (' '.join(f'{s}({cls[s]}) {fac[s]["IB"]:.3f}' for s in ids
                                                  if fac[s]['IB'] >= 1.10) or 'none'))
open(f'{run}/factors.txt', 'w').write('\n'.join(out) + '\n')
print('\n'.join(out))

# Samples by item.
recs = collections.defaultdict(list)
for p in sorted(glob.glob(f'{run}/rec/*.json')):
    s, a, _ = os.path.basename(p).split('.')[:3]
    recs[(s, a)].append(json.load(open(p)))
if not recs:
    sys.exit()
pname = os.path.basename(prof)
addrs = sorted({k for v in recs.values() for r in v for l in r.values() for o, k, _ in l if o == pname and k >= 0})


def load_delta(lines):
    for l in lines:
        p = l.split()
        if p and p[0] == 'LOAD' and 'E' in p[6:-1]:
            return int(p[2], 16) - int(p[1], 16)
    return 0


delta = load_delta(subprocess.run(['readelf', '-lW', prof], capture_output=True, text=True).stdout.splitlines())
txt = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', prof],
                     input='\n'.join(hex(a + delta) for a in addrs) + '\n', capture_output=True, text=True).stdout
chains = {}
for a, b in zip(addrs, txt.split('\n\n')):
    ls = [l for l in b.strip().split('\n') if l]
    chains[a] = [[ls[i], ls[i + 1].rsplit(':', 2)[0]] for i in range(0, len(ls) - 1, 2)]

# The stage rule of ../profile-6c9e240/classify.py (`core_stage`), read from that file so that it is the same code.
src = open(os.path.join(here, '..', 'profile-6c9e240', 'classify.py')).read()
ns = {'re': re}
exec(src[src.index('def core_stage'):src.index('\n', src.index("return 'other'\n", src.index('if f == \'deblock.rs\''))) + 1], ns)
core_stage = ns['core_stage']

ITEMS = ['aprons', 'band views', 'windows leaving a band', 'colocated', 'publish and waits (user)',
         'libc copies and allocation', 'kernel: first touch and memory', 'kernel: other', 'libc and loader: other']
STAGES = ['entropy', 'mb', 'inter', 'recon', 'deblock', 'other']
KMEM = re.compile(r'fault|clear_page|alloc_pages|folio|rmqueue|lru|pte|pmd|zap_|free_pages|free_unref|tlb|memcg|mm_|'
                  r'page|mmap|munmap|vma|brk')


def item_of(obj, key):
    if obj == pname:
        ch = chains.get(key)
        if not ch:
            return 'stage other'
        for fn, f in ch:
            if re.search(r'copy_top|refresh_top', fn) or re.search(r'Canvas>::enter$', fn):
                return 'aprons'
        for fn, f in ch:
            if 'reference_band' in fn or (fn.endswith('::band') and f.endswith(('predict.rs', 'picture.rs'))):
                return 'band views'
        for fn, f in ch:
            if re.search(r'predict_list_any|edge_window|Windows>::fit|BandData>::fit', fn):
                return 'windows leaving a band'
        for fn, f in ch:
            if 'colocated' in fn:
                return 'colocated'
        for fn, f in ch:
            if (re.search(r'wait_for|publish', fn) or f.endswith(('/work.rs', '/picture.rs'))
                    or re.match(r'<?std::(sync|thread)', fn) or re.search(r'library/std/src/(sync|thread)', f)):
                return 'publish and waits (user)'
        return 'stage ' + core_stage(ch)
    if obj == '[kernel.kallsyms]':
        return 'kernel: first touch and memory' if KMEM.search(str(key)) else 'kernel: other'
    if obj.startswith('libc'):
        k = str(key)
        if re.match(r'(pthread_|__pthread|__lll_|futex|__futex|__GI___clone|clone|start_thread)', k):
            return 'publish and waits (user)'
        if re.match(r'(__)?(memcpy|memmove|memset|malloc|free|_int_|calloc|realloc|cfree|__libc_malloc|__libc_free)', k):
            return 'libc copies and allocation'
    return 'libc and loader: other'


def totals(s, a):
    """{event: {item: mean weight over rounds}}."""
    res = collections.defaultdict(collections.Counter)
    for r in recs[(s, a)]:
        for ev, l in r.items():
            e = 'cycles' if ev.startswith(('cycles', 'cpu-clock')) else 'instructions' if ev.startswith('instr') else ev
            for o, k, n in l:
                if a.startswith('us'):
                    res[e][item_of(o, k)] += n / len(recs[(s, a)])
                else:
                    res[e]['kernel' if o == '[kernel.kallsyms]' else 'user'] += n / len(recs[(s, a)])
    return res


rows = {}
for s in ids:
    t = {a: totals(s, a) for a in ('us1', 'us8', 'ff1', 'ff8')}
    c1 = sum(t['us1']['cycles'].values())
    i1 = sum(t['us1']['instructions'].values())
    r = {}
    for it in ITEMS + ['stage ' + x for x in STAGES]:
        r[it] = (t['us8']['cycles'][it] - t['us1']['cycles'][it]) / c1
    for x in STAGES:
        it = 'stage ' + x
        cpi1 = t['us1']['cycles'][it] / max(t['us1']['instructions'][it], 1)
        r[it + ' (more instructions)'] = (t['us8']['instructions'][it] - t['us1']['instructions'][it]) * cpi1 / c1
        r[it + ' (slower)'] = r[it] - r[it + ' (more instructions)']
    r['ours, all'] = sum(t['us8']['cycles'].values()) / c1 - 1
    r['ours, instructions'] = sum(t['us8']['instructions'].values()) / max(i1, 1) - 1
    f1 = sum(t['ff1']['cycles'].values())
    r['FFmpeg, all'] = sum(t['ff8']['cycles'].values()) / f1 - 1
    r['FFmpeg, kernel'] = (t['ff8']['cycles']['kernel'] - t['ff1']['cycles']['kernel']) / f1
    r['FFmpeg, user'] = (t['ff8']['cycles']['user'] - t['ff1']['cycles']['user']) / f1
    r['ours over FFmpeg, 1T cycles'] = c1 / f1
    rows[s] = r
keys = list(rows[ids[0]])
cm = {c: {k: st.mean(rows[s][k] for s in ids if cls[s] == c) for k in keys} for c in CLASSES}
allm = {k: st.mean(cm[c][k] for c in CLASSES) for k in keys}
out = ['Growth from one thread to eight, cycles (user and kernel, all threads), as % of our one-thread cycles;',
       'class: mean over its streams; all: mean of the class means. FFmpeg rows: % of FFmpeg\'s one-thread cycles.',
       f'{"item":46s} {"all":>7s} ' + ' '.join(f'{c:>7s}' for c in CLASSES)]
rank = sorted(ITEMS + ['stage ' + x for x in STAGES], key=lambda k: -allm[k])
for k in rank + [k for k in keys if k not in rank]:
    if k == 'ours over FFmpeg, 1T cycles':
        out.append(f'{k:46s} {allm[k]:7.3f} ' + ' '.join(f'{cm[c][k]:7.3f}' for c in CLASSES))
    else:
        out.append(f'{k:46s} {100 * allm[k]:7.2f} ' + ' '.join(f'{100 * cm[c][k]:7.2f}' for c in CLASSES))
open(f'{run}/items.txt', 'w').write('\n'.join(out) + '\n')
json.dump(rows, open(f'{run}/items.json', 'w'), indent=0)
print('\n'.join(out))
