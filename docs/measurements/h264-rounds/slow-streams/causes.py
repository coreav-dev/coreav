"""causes.py PER_STREAM_JSON: the training streams ranked by their one-thread ratio to FFmpeg on the bar class
(`../mb-layout-2/gating/analysis.txt`, call `g`, discard, `after` = `main` from the MB-layer storage round on), with their coding features
(`../snapshot-outliers/features.tsv`; bits a macroblock from `../../h264-training/es.json`) and the PMU profile's
excess over FFmpeg by group (per_stream.py's output): entropy and the MB layer together, and each of inter prediction,
reconstruction and deblocking split into the driver (CoreAV's Rust around the kernels; FFmpeg's C functions) and the
kernels (CoreAV's SIMD modules; FFmpeg's assembly). Then the means over the ten slowest and over all 32, and the
correlation of each group's excess with each feature over the 32."""
import collections, csv, json, math, os, re, statistics as S, sys

here = os.path.dirname(os.path.abspath(__file__))
M = os.path.join(here, '..', '..')
ps = json.load(open(sys.argv[1]))
es = json.load(open(os.path.join(M, 'h264-training', 'es.json')))
feat = {r['id']: r for r in csv.DictReader(open(os.path.join(here, '..', 'snapshot-outliers', 'features.tsv')),
                                           delimiter='\t')}
dims = {}
for l in open(os.path.join(here, '..', 'profile-acf3ee2f-pmu', 'dims.txt')):
    f, d = l.split()
    w, h, _ = map(int, d.split(','))
    dims[f.split('.')[0]] = (w, h)
gate = {}
txt = open(os.path.join(here, '..', 'mb-layout-2', 'gating', 'analysis.txt')).read()
for l in txt.split('per stream, g ')[1].split('per stream')[0].splitlines()[1:]:
    p = l.split()
    if len(p) >= 6:
        gate[p[0].split('.')[0]] = float(p[5])


def bucket(stage, side, k):
    if stage in ('entropy', 'mb'):
        return 'entropy+MB'
    if stage == 'other':
        return 'other'
    if side == 'core':
        kernel = k.startswith(('avx512.rs', 'x86.rs', 'chroma_mb.rs', 'neon.rs', 'portable.rs', 'scalar.rs'))
    else:
        kernel = not re.match(r'(loop_filter|ff_h264_filter_mb|ff_h264_loop_filter_strength|filter_mb|check_mv|'
                              r'mc_part|ff_prefetch|mc_dir|hl_motion|hl_decode|ff_h264_hl)', k)
    return f"{stage} {'kernels' if kernel else 'driver'}"


GROUPS = ['entropy+MB', 'deblock driver', 'deblock kernels', 'inter driver', 'inter kernels', 'recon driver',
          'recon kernels', 'other']
B = {}
for s, r in ps.items():
    acc = collections.defaultdict(float)
    for x, d in r['stages'].items():
        for side, sign in (('core', 1), ('ffa', -1)):
            for k, v in d['functions'][side]:
                acc[bucket(x, side, k)] += sign * v
    B[s] = acc


def features(s):
    w, h = dims[s]
    mbs = ((w + 15) // 16) * ((h + 15) // 16)
    e = next(v for f, v in es.items() if f.startswith(s))
    ft = feat[s]
    types = {x[0]: int(x[1:]) for x in ft['slices_by_type'].split()}
    return {'bits_mb': e['es_size'] * 8 / e['frames'] / mbs, 'cavlc': 1 - int(ft['cabac']), 't8x8': int(ft['transform8x8']),
            'b_share': types.get('B', 0) / sum(types.values()),
            'weighted': int(ft['weighted_pred'] != '0' or ft['weighted_bipred_idc'] != '0'),
            'slices': float(ft['slices_per_pic']), 'mbs_pic': mbs, 'dims': f'{w}x{h}', 'refs': ft['refs']}


F = {s: features(s) for s in ps}
order = sorted(ps, key=lambda s: -gate[s])
print('one-thread ratio to FFmpeg on the bar class (g), the PMU host\'s cycle ratio, features, excess by group')
print(f"{'stream':12} {'gate':>5} {'pmu':>5} {'dims':>9} {'bits/MB':>7} {'ent':>5} 8x8 {'B':>4} wp sl | "
      + ' '.join(f'{g[:13]:>13}' for g in GROUPS))
for i, s in enumerate(order):
    f = F[s]
    print(f"{s} {gate[s]:5.3f} {ps[s]['stat_ratio']:5.3f} {f['dims']:>9} {f['bits_mb']:7.1f} "
          f"{'CAVLC' if f['cavlc'] else 'CABAC'} {f['t8x8']:3} {f['b_share']:4.2f} {f['weighted']:2} {f['slices']:2.0f} | "
          + ' '.join(f'{B[s][g]:+13.3f}' for g in GROUPS) + ('  (10 slowest)' if i < 10 else ''))
for lab, ids in (('10 slowest', order[:10]), ('all 32', order)):
    print(f'{lab:12} mean excess {"":40} | ' + ' '.join(f'{S.mean(B[s][g] for s in ids):+13.3f}' for g in GROUPS))


def corr(a, b):
    ma, mb = S.mean(a), S.mean(b)
    return sum((x - ma) * (y - mb) for x, y in zip(a, b)) / math.sqrt(sum((x - ma) ** 2 for x in a) * sum((y - mb) ** 2 for y in b))


keys = ['bits_mb', 'cavlc', 't8x8', 'b_share', 'weighted', 'slices', 'mbs_pic']
print('\ncorrelation over the 32 streams (bits_mb as its logarithm)')
print(f"{'':16}" + ' '.join(f'{k:>9}' for k in keys))
ys = {'ratio (gate)': [gate[s] for s in order]}
ys.update({g: [B[s][g] for s in order] for g in GROUPS})
for y, v in ys.items():
    xs = {k: [math.log(F[s][k]) if k == 'bits_mb' else F[s][k] for s in order] for k in keys}
    print(f'{y[:16]:16}' + ' '.join(f'{corr(xs[k], v):+9.2f}' for k in keys))
