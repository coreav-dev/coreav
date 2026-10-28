"""miss.py RES PROF_DIR: the eight-thread miss probe's tables (README.md) from rec_miss.py's output RES (stat/, rec/,
ffattr.nm.txt, ffattr.readelf-l.txt) and the CoreAV binaries that were profiled (PROF_DIR/prof-main, prof-ft).

A sample's stage is classified as ../profile-ae609547-pmu/stages_pmu.py does (CoreAV by ../profile-6c9e240/classify.py's
`core_stage` of its inline chain, FFmpeg by the function symbol's name with that script's FF_RULES); a CoreAV item is
the innermost frame in a coreav- crate (`file:function`). Every quantity is a count per macroblock (weights, the summed
sample periods, over the stream's macroblocks), averaged over the rounds and then over the streams of a class (mean,
each stream counting equally), as #528 reports cycles a macroblock. `cycles` is the mean of the groups' estimates.
Δ_total = p8 - p1, Δ_band = p81 - p1, Δ_cross = p8 - p81 (README.md, "Questions and criteria"). Writes RES/miss.json
and prints the tables."""
import ast, bisect, collections, glob, json, os, re, statistics, subprocess, sys

res, prof = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
CLASSES = ['sd', '720p', '1080p', '4k']
STAGES = ['entropy', 'mb', 'inter', 'recon', 'deblock', 'other']

# The classifiers, taken from their files rather than copied.
ns = {'re': re}
for path, names in ((os.path.join(here, '..', 'profile-6c9e240', 'classify.py'), ('core_stage',)),
                    (os.path.join(here, '..', 'profile-ae609547-pmu', 'stages_pmu.py'), ('FF_RULES', 'ff_stage',
                                                                                          'load_delta', 'core_item'))):
    tree = ast.parse(open(path).read())
    body = [n for n in tree.body if (isinstance(n, ast.FunctionDef) and n.name in names) or
            (isinstance(n, ast.Assign) and any(getattr(t, 'id', '') in names for t in n.targets))]
    exec(compile(ast.Module(body=body, type_ignores=[]), path, 'exec'), ns)
core_stage, ff_stage, load_delta, core_item = ns['core_stage'], ns['ff_stage'], ns['load_delta'], ns['core_item']

mbs = {}
for l in open(os.path.join(here, '..', 'profile-ae609547-pmu', 'dims.txt')):
    f, d = l.split()
    w, h, n = (int(x) for x in d.split(','))
    s, c = f.split('.')[0], f.split('.')[2]
    mbs[s] = ((w + 15) // 16) * ((h + 15) // 16) * min(n, FR[c])

syms = []
for l in open(f'{res}/ffattr.nm.txt'):
    p = l.split()
    if len(p) == 3 and p[1] in 'tTwW' and '.' not in p[2]:
        syms.append((int(p[0], 16), p[2]))
syms.sort()
sa = [a for a, _ in syms]
ff_delta = load_delta(open(f'{res}/ffattr.readelf-l.txt'))


def ff_func(a):
    i = bisect.bisect_right(sa, a + ff_delta) - 1
    return syms[i][1] if i >= 0 else '??'


recs = sorted(glob.glob(f'{res}/rec/*.json'))
# CoreAV inline chains, per binary.
chains = {}
for dec in ('main', 'ft'):
    b = os.path.join(prof, f'prof-{dec}')
    addrs = set()
    for p in recs:
        if f'.{dec}.' in os.path.basename(p):
            for rows in json.load(open(p)).values():
                addrs.update(a for d, a, n in rows if d == f'prof-{dec}')
    if not addrs:
        continue
    addrs = sorted(addrs)
    delta = load_delta(subprocess.run(['readelf', '-lW', b], capture_output=True, text=True).stdout.splitlines())
    txt = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', b],
                         input='\n'.join(hex(a + delta) for a in addrs) + '\n', capture_output=True, text=True).stdout
    for a, blk in zip(addrs, txt.split('\n\n')):
        ls = [l for l in blk.strip().split('\n') if l]
        chains[(f'prof-{dec}', a)] = [[ls[i], ls[i + 1].rsplit(':', 2)[0]] for i in range(0, len(ls) - 1, 2)]

# (stream, class, dec, place, event) -> stage -> per MB; and -> item -> per MB within inter.
stage_w = collections.defaultdict(lambda: collections.defaultdict(float))
# (dec, place, round) -> stream -> inter cycles per MB, summed over the groups (for the rounds check).
round_w = collections.defaultdict(lambda: collections.defaultdict(float))
item_w = collections.defaultdict(lambda: collections.defaultdict(float))
nrounds = collections.Counter()
for p in recs:
    s, c, dec, place, g, r = os.path.basename(p)[:-5].split('.')
    nrounds[(s, c, dec, place, g)] += 1
ngroups_cycles = collections.Counter()
for p in recs:
    s, c, dec, place, g, r = os.path.basename(p)[:-5].split('.')
    k = 1 / nrounds[(s, c, dec, place, g)] / mbs[s]
    data = json.load(open(p))
    if any(e.split(':')[0] == 'cycles' for e in data) and r == '0':
        ngroups_cycles[(s, c, dec, place)] += 1
    for ev, rows in data.items():
        ev = ev.split(':')[0]
        for d, a, n in rows:
            if dec in ('main', 'ft'):
                ch = chains.get((d, a), []) if d == f'prof-{dec}' else []
                stage = core_stage(ch) if ch else 'other'
                item = core_item(ch) if ch else f'[{d}]'
            else:
                item = ff_func(a) if d == 'ffattr' else f'[{d}]'
                stage = ff_stage(item) if d == 'ffattr' else 'other'
            stage_w[(s, c, dec, place, ev)][stage] += n * k
            if stage == 'inter' and ev == 'cycles':
                round_w[(dec, place, r)][s] += n / mbs[s]
            if stage == 'inter':
                item_w[(s, c, dec, place, ev)][item] += n * k
# cycles appear in every group: their mean.
for key, cnt in ngroups_cycles.items():
    if cnt > 1:
        for d in (stage_w, item_w):
            for st in d[key + ('cycles',)]:
                d[key + ('cycles',)][st] /= cnt

streams = sorted({(k[0], k[1]) for k in stage_w})
GROUPS_SEEN = {os.path.basename(p).split('.')[4] for p in recs}
events = sorted({k[4] for k in stage_w}, key=lambda e: (e != 'cycles', e != 'instructions', e))
arms = sorted({(k[2], k[3]) for k in stage_w})


def mean_over(cls, f):
    v = [f(s, c) for s, c in streams if cls in ('all', c)]
    v = [x for x in v if x is not None]
    return statistics.mean(v) if v else None


def stage(dec, place, ev, st, cls):
    return mean_over(cls, lambda s, c: stage_w[(s, c, dec, place, ev)].get(st, 0.0)
                     if (s, c, dec, place, ev) in stage_w else None)


def item(dec, place, ev, it, cls):
    return mean_over(cls, lambda s, c: item_w[(s, c, dec, place, ev)].get(it, 0.0)
                     if (s, c, dec, place, ev) in item_w else None)


def sub(a, b):
    return None if a is None or b is None else a - b


def fmt(x, d=1):
    return '-' if x is None else f'{x:.{d}f}'


out = {'arms': arms, 'events': events, 'stage': {}, 'items': {}}
cols = CLASSES + ['all']
print('## Inter, per macroblock, mean over the streams of each class\n')
for ev in events:
    print(f'### {ev}\n')
    print('| decoder | place | ' + ' | '.join(cols) + ' |')
    print('|---|---|' + '---|' * len(cols))
    for dec, place in arms:
        row = [stage(dec, place, ev, 'inter', c) for c in cols]
        out['stage'][f'{dec}.{place}.{ev}.inter'] = row
        print(f'| {dec} | {place} | ' + ' | '.join(fmt(x, 2 if ev.startswith(('mem_load', 'l2_', 'machine')) else 1)
                                                   for x in row) + ' |')
    print()

print('## The decomposition of inter (all 32; per class in miss.json)\n')
print('| event | decoder | Δ_total (p8-p1) | Δ_band (p81-p1) | Δ_cross (p8-p81) | ft-main at p8 |')
print('|---|---|---|---|---|---|')
for ev in events:
    for dec in ('main', 'ffa'):
        v = {pl: [stage(dec, pl, ev, 'inter', c) for c in cols] for pl in ('p1', 'p81', 'p8')}
        if any(x[-1] is None for x in v.values()):
            continue
        dt = [sub(a, b) for a, b in zip(v['p8'], v['p1'])]
        db = [sub(a, b) for a, b in zip(v['p81'], v['p1'])]
        dc = [sub(a, b) for a, b in zip(v['p8'], v['p81'])]
        ft = None
        if dec == 'main' and stage('ft', 'p8', ev, 'inter', 'all') is not None:
            ft = [sub(stage('ft', 'p8', ev, 'inter', c), x) for c, x in zip(cols, v['p8'])]
        out['stage'][f'{dec}.delta.{ev}.inter'] = {'total': dt, 'band': db, 'cross': dc, 'ft_minus_main': ft}
        d = 2 if ev.startswith(('mem_load', 'l2_', 'machine')) else 1
        print(f'| {ev} | {dec} | {fmt(dt[-1], d)} | {fmt(db[-1], d)} | {fmt(dc[-1], d)} | '
              f'{fmt(ft[-1] if ft else None, d)} |')
print()

print('## The decomposition of inter cycles per class\n')
print('| decoder | part | ' + ' | '.join(cols) + ' |')
print('|---|---|' + '---|' * len(cols))
for dec in ('main', 'ffa'):
    dd = out['stage'].get(f'{dec}.delta.cycles.inter')
    for part in ('total', 'band', 'cross', 'ft_minus_main') if dd else ():
        if dd[part]:
            print(f'| {dec} | {part} | ' + ' | '.join(fmt(x) for x in dd[part]) + ' |')
print()

print('## Every stage, cycles per macroblock (all 32)\n')
print('| decoder | place | ' + ' | '.join(STAGES) + ' |')
print('|---|---|' + '---|' * len(STAGES))
for dec, place in arms:
    row = [stage(dec, place, 'cycles', st, 'all') for st in STAGES]
    out['stage'][f'{dec}.{place}.cycles.stages'] = row
    print(f'| {dec} | {place} | ' + ' | '.join(fmt(x) for x in row) + ' |')
print()

print("## CoreAV's inter functions: Δ_cross (p8 - p81) per macroblock, all 32, the 10 largest by cycles\n")
its = sorted({i for k, v in item_w.items() if k[2] == 'main' for i in v})
dcy = {i: (item('main', 'p8', 'cycles', i, 'all') or 0) - (item('main', 'p81', 'cycles', i, 'all') or 0) for i in its}
top = sorted(its, key=lambda i: -dcy[i])[:10]
print('| function | ' + ' | '.join(events) + ' |')
print('|---|' + '---|' * len(events))
for i in top:
    row = []
    for ev in events:
        a, b = item('main', 'p8', ev, i, 'all'), item('main', 'p81', ev, i, 'all')
        row.append(None if a is None or b is None else a - b)
    out['items'][i] = {'cross': row, 'p1': [item('main', 'p1', ev, i, 'all') for ev in events],
                       'p81': [item('main', 'p81', ev, i, 'all') for ev in events],
                       'p8': [item('main', 'p8', ev, i, 'all') for ev in events]}
    print(f'| `{i}` | ' + ' | '.join(fmt(x, 2) for x in row) + ' |')
print()

print('## Wall time and task-clock (perf stat), median per stream over FFmpeg in the same place\n')
st = collections.defaultdict(list)
for p in glob.glob(f'{res}/stat/*.json'):
    s, c, dec, place, r = os.path.basename(p)[:-5].split('.')
    st[(s, c, dec, place)].append(json.load(open(p)))
print('| decoder | place | wall ' + ' | '.join(cols) + ' | task-clock over wall (all) |')
print('|---|---|' + '---|' * (len(cols) + 1))
for dec, place in arms:
    row = []
    for cl in cols:
        v = [statistics.median(x['wall_s'] for x in st[(s, c, dec, place)]) /
             statistics.median(x['wall_s'] for x in st[(s, c, 'ffa', place)])
             for s, c in streams if cl in ('all', c) and st.get((s, c, dec, place)) and st.get((s, c, 'ffa', place))]
        row.append(statistics.median(v) if v else None)
    busy = [statistics.median(x['task-clock'] / 1e3 / x['wall_s'] for x in st[(s, c, dec, place)])
            for s, c in streams if st.get((s, c, dec, place))]
    print(f'| {dec} | {place} | ' + ' | '.join(fmt(x, 3) for x in row) +
          f' | {fmt(statistics.median(busy) if busy else None, 2)} |')
print()
print('## Check: inter cycles a macroblock over the 32, round by round (mean of the groups)\n')
print('| decoder | place | round 0 | round 1 | 1 over 0 |')
print('|---|---|---|---|---|')
for dec, place in arms:
    v = [statistics.mean(round_w[(dec, place, r)].values()) / len(GROUPS_SEEN) for r in ('0', '1')]
    out['stage'][f'{dec}.{place}.cycles.inter.rounds'] = v
    print(f'| {dec} | {place} | {v[0]:.1f} | {v[1]:.1f} | {v[1] / v[0]:.3f} |')
json.dump(out, open(f'{res}/miss.json', 'w'), indent=1)
