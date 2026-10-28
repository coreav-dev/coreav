import sys, pickle, collections, statistics, re
sys.path.insert(0, '/tmp/ent'); from sig import per, chains, core_stage, dims, dims_ff, short
ff = pickle.load(open('/tmp/ent/ff.pkl', 'rb'))
ENG = {'cabac.rs:decision', 'cabac.rs:consume', 'cabac.rs:refill', 'cabac.rs:load', 'cabac.rs:bypass', 'cabac.rs:renormalize',
       'cabac.rs:offset', 'cabac.rs:terminate', 'cabac.rs:last_word', 'cabac.rs:exp_golomb_bypass'}
# first match from the innermost frame outwards
RULES = [('res.sigmap', r'significance_map'), ('res.levels', r':levels|levels_body|exp_golomb_bypass_out'),
         ('res.cbf', r'coded_block_flag_inc|block_context|neighbour_cbf'),
         ('mvd', r'mvd'), ('ref_idx', r'ref_idx'), ('cbp', r':cbp'), ('qp_delta', r'qp_delta'),
         ('intra modes', r'prev_intra_flag|rem_intra_mode|chroma_pred_mode|start_intra|derive_4x4|derive_8x8|predicted_intra|fill_intra_cache'),
         ('mb_type/sub/t8x8', r'mb_type|transform8x8|no_sub_8x8'),
         ('res.block (cbf bin, layout, dispatch)', r'syntax.rs:block$|cabac.rs:residual|parse_residual|parse_4x4|parse_8x8|failed'),
         ('cavlc', r'cavlc|bits.rs'), ('nal/rbsp', r'nal.rs'),
         ('mb.mv pred/direct/store (inter.rs)', r'inter.rs'),
         ('mb.per-MB setup/finish', r'start_mb|reset|finish_mb|derive_neighbours|available|enter_row|store_band|request_dest|decode_skip'),
         ('slice loop (skip flag, end of slice, decode_mb)', r'decode_cabac|decode_mb|decode_slice|terminate'),
         ]
def cat(ch):
    s = short(ch)
    for name, rx in RULES:
        for f in reversed(s):
            if re.search(rx, f): return name
    return 'other: ' + (s[-1] if s else '?')
core = collections.defaultdict(lambda: collections.defaultdict(lambda: [0.0, 0.0]))  # stream -> cat -> [engine, glue]
cav = {}
cache = {}
for (s, c), d in per.items():
    cv = 0
    for a, ir in d.items():
        if a not in cache:
            ch = chains.get(a, [])
            st = core_stage([[x[0], x[1].rsplit(':', 2)[0]] for x in ch])
            sh = short(ch)
            cache[a] = (st, cat(ch) if st in ('entropy', 'mb') else None, bool(sh) and sh[-1] in ENG, any('cavlc.rs' in f for f in sh))
        st, k, eng, isc = cache[a]
        if isc: cv += ir
        if k: core[s][k][0 if eng else 1] += ir / dims[s]
    cav[s] = cv / dims[s] > 50
FFC = [('res', r'residual|significance|last_significant|coeff_abs'), ('mvd', r'mvd'), ('ref_idx', r'mb_ref'), ('skip', r'mb_skip'),
       ('intra mb_type', r'intra_mb_type'), ('mb_cabac', r'decode_mb_cabac'), ('cavlc', r'cavlc|decode_residual|get_bits|golomb|get_ue|get_se|vlc'),
       ('mb layer', r'fill_decode|pred_direct|pred_motion|pred_pskip|decode_mb_skip|fill_rectangle'), ('slice', r'decode_slice$'),
       ('nal', r'h2645|startcode|_nal_|nal_unit|^av_parser_parse2$|^h264_parse$|^parse_nal')]
def ffcat(n):
    for k, rx in FFC:
        if re.search(rx, n): return k
    return None
ffr = {}
for (s, c), (selfc, calls, incl) in ff.items():
    d = collections.defaultdict(lambda: [0.0, 0.0])
    bins = 0
    for fn, ir in selfc.items():
        if fn.startswith('get_cabac'): continue
        k = ffcat(fn)
        if k: d[k][1] += ir / dims_ff[s]
    for (fn, cfn), ir in incl.items():
        if cfn.startswith('get_cabac') and not fn.startswith('get_cabac'):
            k = ffcat(fn) or 'other'
            d[k][0] += ir / dims_ff[s]; bins += calls[(fn, cfn)]
    d['_bins'] = [bins / dims_ff[s], 0]
    ffr[s] = d
for mode in (False, True):
    ss = [s for s in core if cav[s] == mode]
    print(f"\n=== {'CAVLC' if mode else 'CABAC'} streams: {len(ss)}  (instructions per MB, mean over streams; engine = decision/bypass/refill code)")
    cats = sorted({k for s in ss for k in core[s]}, key=lambda k: -sum(sum(core[s][k]) for s in ss))
    te = tg = 0
    for k in cats:
        e = sum(core[s][k][0] for s in ss) / len(ss); g = sum(core[s][k][1] for s in ss) / len(ss); te += e; tg += g
        if e + g > 3: print(f'  {k:48} engine {e:7.1f} glue {g:7.1f} total {e+g:7.1f}')
    print(f"  {'CoreAV entropy+MB total':48} engine {te:7.1f} glue {tg:7.1f} total {te+tg:7.1f}")
    cats = sorted({k for s in ss for k in ffr[s]}, key=lambda k: -sum(sum(ffr[s][k]) for s in ss))
    te = tg = 0
    for k in cats:
        e = sum(ffr[s][k][0] for s in ss) / len(ss); g = sum(ffr[s][k][1] for s in ss) / len(ss)
        if k != '_bins': te += e; tg += g
        print(f'  FF {k:45} get_cabac* {e:7.1f} self {g:7.1f} total {e+g:7.1f}')
    print(f"  {'FFmpeg entropy+MB total':48} get_cabac* {te:7.1f} self {tg:7.1f} total {te+tg:7.1f}")
pickle.dump((dict(core), cav, ffr), open('/tmp/ent/report.pkl', 'wb'))
