"""hints.py RES PROF_DIR: the banded hints' part-only probe's tables (README.md) from rec_hints.py's output RES (stat/,
rec/) and the CoreAV binaries that were profiled (PROF_DIR/prof-<arm>).

Samples are classified as ../miss-8t/miss.py does (../profile-6c9e240/classify.py's `core_stage` of the inline chain,
the item ../profile-ae609547-pmu/stages_pmu.py's `core_item`); every quantity is a count per macroblock, averaged over
the rounds and then over the streams of a class, each stream counting equally; `cycles` is the mean of the groups'
estimates. Writes RES/hints.json and prints the tables."""
import ast, collections, glob, json, os, re, statistics, subprocess, sys

res, prof = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
CLASSES = ['sd', '720p', '1080p', '4k']
COLS = CLASSES + ['all']
STAGES = ['entropy', 'mb', 'inter', 'recon', 'deblock', 'other']

# The classifiers, taken from their files rather than copied.
ns = {'re': re}
for path, names in ((os.path.join(here, '..', 'profile-6c9e240', 'classify.py'), ('core_stage',)),
                    (os.path.join(here, '..', 'profile-ae609547-pmu', 'stages_pmu.py'), ('load_delta', 'core_item'))):
    tree = ast.parse(open(path).read())
    body = [n for n in tree.body if (isinstance(n, ast.FunctionDef) and n.name in names) or
            (isinstance(n, ast.Assign) and any(getattr(t, 'id', '') in names for t in n.targets))]
    exec(compile(ast.Module(body=body, type_ignores=[]), path, 'exec'), ns)
core_stage, load_delta, core_item = ns['core_stage'], ns['load_delta'], ns['core_item']

mbs = {}
for l in open(os.path.join(here, '..', 'profile-ae609547-pmu', 'dims.txt')):
    f, d = l.split()
    w, h, n = (int(x) for x in d.split(','))
    s, c = f.split('.')[0], f.split('.')[2]
    mbs[s] = ((w + 15) // 16) * ((h + 15) // 16) * min(n, FR[c])

recs = sorted(glob.glob(f'{res}/rec/*.json'))
arms = []
for p in recs:
    a = os.path.basename(p).split('.')[2]
    if a not in arms:
        arms.append(a)
arms.sort(key=lambda a: (a != 'base', len(a), a))
chains = {}
for dec in arms:
    b = os.path.join(prof, f'prof-{dec}')
    addrs = set()
    for p in recs:
        if os.path.basename(p).split('.')[2] == dec:
            for rows in json.load(open(p)).values():
                addrs.update(a for d, a, n in rows if d == f'prof-{dec}')
    addrs = sorted(addrs)
    delta = load_delta(subprocess.run(['readelf', '-lW', b], capture_output=True, text=True).stdout.splitlines())
    txt = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', b],
                         input='\n'.join(hex(a + delta) for a in addrs) + '\n', capture_output=True, text=True).stdout
    for a, blk in zip(addrs, txt.split('\n\n')):
        ls = [l for l in blk.strip().split('\n') if l]
        chains[(f'prof-{dec}', a)] = [[ls[i], ls[i + 1].rsplit(':', 2)[0]] for i in range(0, len(ls) - 1, 2)]

stage_w = collections.defaultdict(lambda: collections.defaultdict(float))
item_w = collections.defaultdict(lambda: collections.defaultdict(float))
round_w = collections.defaultdict(lambda: collections.defaultdict(float))
nrounds = collections.Counter()
for p in recs:
    s, c, dec, place, g, r = os.path.basename(p)[:-5].split('.')
    nrounds[(s, c, dec, g)] += 1
ngroups_cycles = collections.Counter()
for p in recs:
    s, c, dec, place, g, r = os.path.basename(p)[:-5].split('.')
    k = 1 / nrounds[(s, c, dec, g)] / mbs[s]
    data = json.load(open(p))
    if any(e.split(':')[0] == 'cycles' for e in data) and r == '0':
        ngroups_cycles[(s, c, dec)] += 1
    for ev, rows in data.items():
        ev = ev.split(':')[0]
        for d, a, n in rows:
            ch = chains.get((d, a), []) if d == f'prof-{dec}' else []
            stage = core_stage(ch) if ch else 'other'
            stage_w[(s, c, dec, ev)][stage] += n * k
            if stage == 'inter':
                item_w[(s, c, dec, ev)][core_item(ch)] += n * k
                if ev == 'cycles':
                    round_w[(dec, r)][s] += n / mbs[s]
for key, cnt in ngroups_cycles.items():
    if cnt > 1:
        for d in (stage_w, item_w):
            for st in d[key + ('cycles',)]:
                d[key + ('cycles',)][st] /= cnt

streams = sorted({(k[0], k[1]) for k in stage_w})
groups_seen = {os.path.basename(p).split('.')[4] for p in recs}
events = sorted({k[3] for k in stage_w}, key=lambda e: (e != 'cycles', e != 'instructions', e))


def mean_over(cls, f):
    v = [f(s, c) for s, c in streams if cls in ('all', c)]
    v = [x for x in v if x is not None]
    return statistics.mean(v) if v else None


def stage(dec, ev, st, cls):
    return mean_over(cls, lambda s, c: stage_w[(s, c, dec, ev)].get(st, 0.0) if (s, c, dec, ev) in stage_w else None)


def item(dec, ev, it, cls):
    return mean_over(cls, lambda s, c: item_w[(s, c, dec, ev)].get(it, 0.0) if (s, c, dec, ev) in item_w else None)


def fmt(x, d=1):
    return '-' if x is None else f'{x:.{d}f}'


def digits(ev):
    return 2 if ev.startswith(('mem_load', 'l2_', 'machine')) else 1


out = {'arms': arms, 'events': events, 'inter': {}, 'stages': {}, 'items': {}, 'wall': {}, 'rounds': {}}
print('## Inter at eight threads (p8), per macroblock, mean over the streams of each class\n')
for ev in events:
    print(f'### {ev}\n')
    print('| arm | ' + ' | '.join(COLS) + ' | ' + ' | '.join(f'Δ {c}' for c in COLS) + ' |')
    print('|---|' + '---|' * 2 * len(COLS))
    base = [stage('base', ev, 'inter', c) for c in COLS]
    for dec in arms:
        row = [stage(dec, ev, 'inter', c) for c in COLS]
        delta = [None if a is None or b is None else a - b for a, b in zip(row, base)]
        out['inter'][f'{dec}.{ev}'] = {'value': row, 'minus_base': delta}
        print(f'| {dec} | ' + ' | '.join(fmt(x, digits(ev)) for x in row) + ' | ' +
              ' | '.join(fmt(x, digits(ev)) for x in delta) + ' |')
    print()

print('## Every stage, cycles per macroblock (all 32)\n')
print('| arm | ' + ' | '.join(STAGES) + ' | sum |')
print('|---|' + '---|' * (len(STAGES) + 1))
for dec in arms:
    row = [stage(dec, 'cycles', st, 'all') for st in STAGES]
    out['stages'][dec] = row
    print(f'| {dec} | ' + ' | '.join(fmt(x) for x in row) + f' | {fmt(sum(x or 0 for x in row))} |')
print()

print('## Inter functions, cycles per macroblock (all 32): arm - base, the 12 largest in base\n')
its = sorted({i for k, v in item_w.items() if k[2] == 'base' and k[3] == 'cycles' for i in v},
             key=lambda i: -(item('base', 'cycles', i, 'all') or 0))[:12]
print('| function | base | ' + ' | '.join(a for a in arms if a != 'base') + ' |')
print('|---|---|' + '---|' * (len(arms) - 1))
for i in its:
    b = item('base', 'cycles', i, 'all') or 0
    row = [(item(a, 'cycles', i, 'all') or 0) - b for a in arms if a != 'base']
    out['items'][i] = {'base': b, 'minus_base': row}
    print(f'| `{i}` | {b:.1f} | ' + ' | '.join(f'{x:+.1f}' for x in row) + ' |')
print()

st = collections.defaultdict(list)
for p in glob.glob(f'{res}/stat/*.json'):
    s, c, dec, place, r = os.path.basename(p)[:-5].split('.')
    st[(s, c, dec)].append(json.load(open(p)))


def med(s, c, dec, key):
    v = st.get((s, c, dec))
    return statistics.median(x[key] for x in v) if v else None


print('## perf stat: median per stream of the wall time over base\'s and over FFmpeg\'s (streams faster)\n')
print('| arm | over | ' + ' | '.join(COLS) + ' | task-clock over base (all) |')
print('|---|---|' + '---|' * (len(COLS) + 1))
for dec in arms:
    for ref in ('base', 'ff'):
        row = []
        for cl in COLS:
            v = [med(s, c, dec, 'wall_s') / med(s, c, ref, 'wall_s') for s, c in streams
                 if cl in ('all', c) and med(s, c, dec, 'wall_s') and med(s, c, ref, 'wall_s')]
            row.append((statistics.median(v), sum(x < 1 for x in v), len(v)) if v else None)
        tc = [med(s, c, dec, 'task-clock') / med(s, c, 'base', 'task-clock') for s, c in streams
              if med(s, c, dec, 'task-clock') and med(s, c, 'base', 'task-clock')]
        out['wall'][f'{dec}.{ref}'] = row
        tcs = f'{statistics.median(tc):.3f}' if tc else '-'
        print(f'| {dec} | {ref} | ' + ' | '.join('-' if x is None else f'{x[0]:.3f} ({x[1]}/{x[2]})' for x in row)
              + f' | {tcs} |')
print()

print('## Check: inter cycles a macroblock over the 32, round by round (mean of the groups)\n')
print('| arm | round 0 | round 1 | 1 over 0 |')
print('|---|---|---|---|')
for dec in arms:
    v = [statistics.mean(round_w[(dec, r)].values()) / len(groups_seen) for r in ('0', '1')]
    out['rounds'][dec] = v
    print(f'| {dec} | {v[0]:.1f} | {v[1]:.1f} | {v[1] / v[0]:.3f} |')
json.dump(out, open(f'{res}/hints.json', 'w'), indent=1)
