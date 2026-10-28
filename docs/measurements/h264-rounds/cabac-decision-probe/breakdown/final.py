"""final.py: entropy + MB layer instructions per MB by syntax-element group, CoreAV (callgrind, instruction level, inline
chains) against FFmpeg (callgrind, function level), with exact bins per MB from a counting build."""
import sys, pickle, collections, re
sys.path.insert(0, '/tmp/ent')
exec(open('/tmp/ent/report.py').read().split('for mode in (False, True):')[0])  # RULES, cat, ENG, FFC, ffcat, per, chains...
bins = pickle.load(open('/tmp/ent/bins.pkl', 'rb'))
core = collections.defaultdict(lambda: collections.defaultdict(lambda: [0.0, 0.0]))  # recomputed
for (s, c), d in per.items():
    for a, ir in d.items():
        ch = chains.get(a, [])
        st = core_stage([[x[0], x[1].rsplit(':', 2)[0]] for x in ch])
        if st not in ('entropy', 'mb'): continue
        sh = short(ch)
        core[s][cat(ch)][0 if (sh and sh[-1] in ENG) else 1] += ir / dims[s]
G = {'residual (sig/last, levels, signs, cbf, block dispatch)': (['res.sigmap', 'res.levels', 'res.block (cbf bin, layout, dispatch)', 'res.cbf'], ['res']),
     'mvd and ref_idx': (['mvd', 'ref_idx'], ['mvd', 'ref_idx']),
     'MB header (skip, mb_type, sub, t8x8, cbp, qp_delta, intra modes) and the MB loop': (['slice loop (skip flag, end of slice, decode_mb)', 'mb_type/sub/t8x8', 'cbp', 'qp_delta', 'intra modes'], ['mb_cabac', 'skip', 'intra mb_type', 'slice']),
     'MB layer (neighbours, mv prediction, direct, motion store, per-MB setup)': (['mb.mv pred/direct/store (inter.rs)', 'mb.per-MB setup/finish'], ['mb layer']),
     'NAL/RBSP': (['nal/rbsp'], ['nal']),
     'CAVLC syntax': (['cavlc'], ['cavlc'])}
for mode in ('CABAC', 'CAVLC'):
    ss = [s for s in core if (bins[s]['sig'] > 0) == (mode == 'CABAC')]
    n = len(ss)
    m = lambda f: sum(f(s) for s in ss) / n
    print(f'\n=== {mode}: {n} streams, per MB, mean over streams')
    tc = tf = 0
    used = set()
    for g, (cc, fc) in G.items():
        ce = m(lambda s: sum(core[s][k][0] for k in cc)); cg = m(lambda s: sum(core[s][k][1] for k in cc))
        fe = m(lambda s: sum(ffr[s][k][0] for k in fc)); fg = m(lambda s: sum(ffr[s][k][1] for k in fc))
        used |= set(cc)
        tc += ce + cg; tf += fe + fg
        print(f'  {g[:60]:60} CoreAV {ce+cg:7.1f} (engine {ce:6.1f}) FFmpeg {fe+fg:7.1f} (get_cabac {fe:6.1f})  diff {ce+cg-fe-fg:+7.1f}')
    rest = m(lambda s: sum(sum(v) for k, v in core[s].items() if k not in used))
    frest = m(lambda s: sum(sum(v) for k, v in ffr[s].items() if k not in sum((x[1] for x in G.values()), []) and k != '_bins'))
    print(f"  {'unassigned (other)':60} CoreAV {rest:7.1f}                  FFmpeg {frest:7.1f}")
    print(f"  {'total':60} CoreAV {tc+rest:7.1f}                  FFmpeg {tf+frest:7.1f}  diff {tc+rest-tf-frest:+7.1f}")
    if mode == 'CABAC':
        B = lambda ks: m(lambda s: sum(bins[s][k] for k in ks))
        reg = ['mb_skip', 'mb_type', 'sub_mb_type', 't8x8', 'mvd', 'ref_idx', 'qp_delta', 'intra modes', 'cbp', 'cbf', 'res via decision()', 'sig', 'last', 'abs first', 'abs rest']
        print(f"  bins/MB: regular {B(reg):.2f}, bypass {B(['all bypass bins']):.2f}, terminate {B(['terminate']):.2f}; residual regular {B(['cbf','sig','last','abs first','abs rest','res via decision()']):.2f} + signs {B(['sign (levels)']):.2f}")
        eng = m(lambda s: sum(v[0] for v in core[s].values()))
        fc = m(lambda s: sum(n for (a, b), n in ff[(s, [c for (x, c) in ff if x == s][0])][1].items() if b.startswith('get_cabac') and not a.startswith('get_cabac')))
        fi = m(lambda s: sum(n for (a, b), n in ff[(s, [c for (x, c) in ff if x == s][0])][2].items() if b.startswith('get_cabac') and not a.startswith('get_cabac')))
        print(f"  FFmpeg get_cabac/get_cabac_noinline: {fi / fc:.1f} instructions a call (inclusive), {fc:.0f} calls a stream (mean)")
        print(f"  CoreAV engine instructions per bin (regular+bypass+terminate): {eng / B(reg + ['all bypass bins', 'terminate']):.1f}")
        for k, bk in [('res.sigmap', ['sig', 'last']), ('res.levels', ['abs first', 'abs rest', 'sign (levels)']), ('cbp', ['cbp']), ('qp_delta', ['qp_delta']),
                      ('intra modes', ['intra modes']), ('mb_type/sub/t8x8', ['mb_type', 'sub_mb_type', 't8x8']), ('res.block (cbf bin, layout, dispatch)', ['cbf']),
                      ('mvd', ['mvd', 'Cabac::bypass (mvd sign)']), ('slice loop (skip flag, end of slice, decode_mb)', ['mb_skip', 'terminate'])]:
            print(f"    {k[:40]:40} bins {B(bk):6.2f}  engine/bin {m(lambda s: core[s][k][0]) / B(bk):5.1f}  glue/bin {m(lambda s: core[s][k][1]) / B(bk):6.1f}")
        print(f"  coded residual blocks/MB (significance maps): {B(['blocks N=4','blocks N=16','blocks N=64']):.2f}")
