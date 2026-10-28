import sys, pickle, collections, re
sys.path.insert(0, '/tmp/ent')
exec(open('/tmp/ent/report.py').read().split('for mode in (False, True):')[0])
bins = pickle.load(open('/tmp/ent/bins.pkl', 'rb'))
def cl(n):
    while re.search(r'<[^<>]*>', n): n = re.sub(r'<[^<>]*>', '', n)
    return n.removesuffix('::').split('::')[-1]
for mode in ('CABAC', 'CAVLC'):
    ss = [k for k in per if (bins[k[0]]['sig'] > 0) == (mode == 'CABAC')]
    F = collections.Counter()
    for k in ss:
        for a, ir in per[k].items():
            st, kk, eng, isc = cache[a]
            if not kk: continue
            fr = [f for f in chains.get(a, []) if 'coreav-' in f[1]]
            F[f"{fr[0][1].rsplit('/', 1)[-1].split(':')[0]}:{cl(fr[0][0])}" if fr else '?'] += ir / dims[k[0]] / len(ss)
    print(f'\n{mode} CoreAV entropy+MB by innermost function (inlined code under its own function), per MB, top 30 of {sum(F.values()):.0f}')
    for f, v in F.most_common(30): print(f'  {v:7.1f} {f}')
    G = collections.Counter()
    for k in ss:
        selfc = ff[k][0]
        for fn, ir in selfc.items():
            if ffcat(fn) or fn.startswith('get_cabac'): G[fn] += ir / dims_ff[k[0]] / len(ss)
    print(f'{mode} FFmpeg entropy+MB functions (self, function level), per MB, top 20 of {sum(G.values()):.0f}')
    for f, v in G.most_common(20): print(f'  {v:7.1f} {f}')
