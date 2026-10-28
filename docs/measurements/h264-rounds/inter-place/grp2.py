"""grp2.py PKL: inter's cycles (share of FFmpeg's, mean per stream) and instructions a MB by group, per class."""
import pickle, sys, collections, re
d = pickle.load(open(sys.argv[1], 'rb'))
pi, st, dims = d['per_item'], d['st'], d['dims']
streams = sorted({(k[0], k[1]) for k in st})
def g_core(n):
    f, fn = n.split(':', 1) if ':' in n else (n, '')
    if f.endswith('prefetch.rs') or fn in ('hint_read', 'hint_rows', 'request', 'hint_write'): return 'prefetch'
    if f.startswith('coreav-h264/') or 'fast/mod.rs' in f: return 'decoder'
    if f == 'coreav-kernels-h264/src/predict.rs' or f.endswith('views.rs') or f.endswith('lib.rs'): return 'place'
    if f.startswith('simd/x86/'): return 'drivers'
    return 'kernels'
def g_ff(n):
    if 'prefetch' in n: return 'prefetch'
    if re.search(r'mc_part|hl_motion|mc_dir_part|decode_slice', n): return 'decoder'
    return 'kernels'
for lab in ('sd', '720p', '1080p', '4k', 'all'):
    ss = [(s, c) for s, c in streams if lab in ('all', c)]
    out = {}
    for dec, g in (('core', g_core), ('ffa', g_ff)):
        cyc, ins = collections.defaultdict(float), collections.defaultdict(float)
        for s, c in ss:
            base = sum(st.get((s, c, 'ffa', 'cycles'), {}).values())
            for n, w in pi.get((s, c, dec, 'cycles', 'inter'), {}).items(): cyc[g(n)] += w / base / len(ss)
            for n, w in pi.get((s, c, dec, 'instructions', 'inter'), {}).items(): ins[g(n)] += w / dims[s] / len(ss)
        out[dec] = (cyc, ins)
    print(f'{lab:5}\n  ' + '\n  '.join(f"{k}: {out['core'][0][k]:.4f}/{out['ffa'][0][k]:.4f} ({out['core'][0][k]-out['ffa'][0][k]:+.4f}) ins {out['core'][1][k]:.0f}/{out['ffa'][1][k]:.0f}" for k in ('prefetch', 'decoder', 'place', 'drivers', 'kernels')))
