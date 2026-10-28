"""byfn_pmu.py RES CORE_BIN: instructions/cycles per MB of the entropy+MB stages, by function, CoreAV vs FFmpeg."""
import ast, bisect, collections, glob, json, os, re, statistics, subprocess, sys
res, core_bin = sys.argv[1], sys.argv[2]
here = '$HOME/coreav/docs/measurements/h264-rounds/pmu-profile'
src = open(os.path.join(here, 'stages_pmu.py')).read()
# reuse definitions up to the chains step
cut = src.index('# Per (stream, class, decoder, event)')
sys.argv = ['x', res, core_bin]
g = {'__file__': os.path.join(here, 'stages_pmu.py')}
exec(compile(src[:cut], 'stages_pmu.py', 'exec'), g)
chains, core_stage, ff_stage, ff_func, dims, recs, core_dso = (g[k] for k in
    ('chains', 'core_stage', 'ff_stage', 'ff_func', 'dims', 'recs', 'core_dso'))

def clean(name):
    while re.search(r'<[^<>]*>', name):
        name = re.sub(r'<[^<>]*>', '', name)
    return name.removesuffix('::')

def core_fn(ch, depth):
    fr = [f for f in ch if 'coreav-' in f[1]]
    if not fr:
        return 'other'
    f = fr[min(depth, len(fr) - 1)]
    return f"{f[1].rsplit('/', 1)[-1]}:{'::'.join(clean(f[0]).split('::')[-2:])}"

depth = int(os.environ.get('DEPTH', '0'))
nr = collections.Counter()
for p in recs:
    s, c, dec, gname, r = os.path.basename(p)[:-5].split('.')
    nr[(s, dec)] += 1
W = collections.defaultdict(lambda: collections.defaultdict(float))  # (dec, ev) -> fn -> per-MB mean over streams
streams = set()
for p in recs:
    s, c, dec, gname, r = os.path.basename(p)[:-5].split('.')
    streams.add(s)
    for ev, rows in json.load(open(p)).items():
        ev = ev.split(':')[0]
        for d, a, n in rows:
            if dec == 'core':
                if d != core_dso: continue
                ch = chains.get(a, [])
                if core_stage(ch) not in ('entropy', 'mb'): continue
                key = core_fn(ch, depth)
            else:
                if d != 'ffattr': continue
                fnm = ff_func(a)
                if ff_stage(fnm) not in ('entropy', 'mb'): continue
                key = fnm
            W[(dec, ev)][key] += n / nr[(s, dec)] / dims[s]
ns = len(streams)
for dec in ('core', 'ffa'):
    ins, cyc = W[(dec, 'instructions')], W[(dec, 'cycles')]
    ti = sum(ins.values()) / ns
    print(f'\n== {dec}: entropy+mb instructions/MB {ti:.0f}, cycles/MB {sum(cyc.values())/ns:.0f}')
    for k, v in sorted(ins.items(), key=lambda x: -x[1])[:45]:
        print(f'  {v/ns:8.1f} {cyc.get(k,0)/ns:8.1f}  {k}')
