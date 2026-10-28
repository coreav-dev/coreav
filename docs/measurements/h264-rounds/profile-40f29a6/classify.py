"""classify.py CORE_DIR FF_DIR FFMPEG_G -> stage table: CoreAV and FFmpeg 9.0.2 cpu-clock samples by decoding stage, per
class and over all 32 streams, each stream weighted equally (stage time over that stream's FFmpeg total), averaged over
the rounds recorded. Stage of a CoreAV sample: the innermost frame of its inline chain in a coreav- crate (the kernels
crate's by the symbol that holds it); of an FFmpeg sample: the innermost frame's file and function, and for assembly the
nearest preceding function symbol. Writes CORE_DIR/stages.json and prints the tables."""
import bisect, collections, glob, json, os, re, subprocess, sys
cd, fd, ffg = sys.argv[1:4]
STAGES = ['entropy', 'mb', 'inter', 'recon', 'deblock', 'other']
NAMES = {'entropy': 'entropy (NAL, CABAC/CAVLC, MB syntax)', 'mb': 'MB layer (neighbours, motion derivation, caches)',
         'inter': 'inter prediction and reference fetch', 'recon': 'reconstruction (dequant, IDCT, intra)',
         'deblock': 'deblocking (strengths and filter)', 'other': 'other (setup, output, allocation, kernel, libc)'}
def core_stage(ch):
    fr = next((f for f in ch if 'coreav-' in f[1]), None)
    if fr is None:
        return 'other'
    fn, f = fr[0], fr[1].rsplit('/', 1)[-1]
    sym = ch[-1][0]
    m = fn
    while re.search(r'<[^<>]*>', m):
        m = re.sub(r'<[^<>]*>', '', m)
    m = m.split('::')[-1]
    if 'coreav-kernels-h264' in fr[1]:
        if re.search(r'luma_mc|chroma_mc|combine|weigh|interpolat|copy_rows|predict|Average|Put', sym): return 'inter'
        if re.search(r'idct|dequant|intra|pred(4|8|16)|dc_', sym): return 'recon'
        if re.search(r'deblock|filter|luma_columns|luma_rows|chroma_pair|edge', sym): return 'deblock'
        if f == 'prefetch.rs': return 'inter'
        return 'other'
    if f in ('cabac.rs', 'cavlc.rs', 'bits.rs', 'syntax.rs', 'nal.rs'): return 'entropy'
    if f == 'mb.rs':
        if re.match(r'(parse_|store_mvd|coded_block_flag_inc|is_skip|decode_cabac|decode_mb$|spec_next)', m): return 'entropy'
        return 'mb'
    if f == 'inter.rs':
        if re.match(r'(predict_fast|reference_band|predict_whole|predict_list|edge_window|window|motion_compensate|inter_pred|combine_predictions|implicit_weights|fetch)', m): return 'inter'
        return 'mb'
    if f == 'recon.rs': return 'recon'
    if f == 'deblock.rs': return 'deblock'
    return 'other'
syms = []
for l in subprocess.run(['nm', '-n', '--defined-only', ffg], capture_output=True, text=True).stdout.splitlines():
    p = l.split()
    if len(p) == 3 and p[1] in 'tT' and '.' not in p[2] and not p[2].startswith('..'):
        syms.append((int(p[0], 16), p[2]))
sa = [s[0] for s in syms]
delta = 0
for l in subprocess.run(['readelf', '-lW', ffg], capture_output=True, text=True).stdout.splitlines():
    p = l.split()
    if p and p[0] == 'LOAD' and 'E' in p[6:-1]: delta = int(p[2], 16) - int(p[1], 16)
GENERIC = ('emmintrin.h', 'xmmintrin.h', 'smmintrin.h', 'tmmintrin.h', 'intreadwrite.h', 'common.h', 'attributes.h', 'intmath.h', 'mem.h')
def ff_stage(ch, a):
    if not ch: return 'other'
    k = next((i for i, fr in enumerate(ch) if fr[1].rsplit('/', 1)[-1] not in GENERIC), 0)
    fn, f = ch[k][0], ch[k][1].rsplit('/', 1)[-1]
    if fn == 'mid_pred': return 'mb'
    outer = ch[-1][0]
    if f.endswith('.asm') or fn == '??':
        i = bisect.bisect_right(sa, a + delta) - 1
        s = syms[i][1] if i >= 0 else ''
        if re.search(r'qpel|chroma_mc|pixels|weight|prefetch|emulated_edge|emu_edge|mc\d', s): return 'inter'
        if re.search(r'idct|pred4x4|pred8x8|pred16x16|dequant|luma_dc|intrapred', s): return 'recon'
        if re.search(r'deblock|loop_filter', s): return 'deblock'
        if re.search(r'cabac|decode_significance', s): return 'entropy'
        return 'other'
    if f in ('h264_cabac.c', 'cabac.h', 'cabac_functions.h', 'h264_cavlc.c', 'get_bits.h', 'golomb.h', 'h2645_parse.c',
             'cabac.c', 'h264_parse.c', 'startcode.c', 'h264_parser.c') or re.match(r'(get_cabac|decode_cabac|decode_significance|decode_residual)', fn):
        if fn in ('fill_rectangle',): return 'mb'
        return 'entropy'
    if f in ('h264_mvpred.h', 'h264_direct.c', 'rectangle.h', 'h264_refs.c'): return 'mb'
    if f == 'h264_slice.c':
        if re.match(r'(loop_filter|fill_filter_caches|backup_mb_border)', fn): return 'deblock'
        return 'mb'
    if f == 'h264_loopfilter.c' or 'loop_filter' in fn or 'deblock' in fn: return 'deblock'
    if f in ('h264_mc_template.c', 'videodsp_template.c', 'pel_template.c', 'videodsp.c', 'h264qpel_template.c', 'h264chroma_template.c') or re.match(r'(mc_|prefetch_motion|hl_motion)', fn): return 'inter'
    if f in ('h264_mb.c', 'h264_mb_template.c', 'h264idct_template.c', 'h264pred_template.c', 'h264pred.c'): return 'recon'
    if f.startswith('h264') : return 'mb'
    return 'other'
def load(d, dso, stagef, kind, only=None):
    R = json.load(open(f'{d}/addr.{dso}.json'))
    per = collections.defaultdict(lambda: collections.Counter()); nr = collections.Counter()
    for p in sorted(glob.glob(f'{d}/*.{kind}.*.json')):
        s, c, _, r = os.path.basename(p)[:-5].split('.')
        if only is not None and r != only: continue
        nr[(s, c)] += 1
        for name, a, n in json.load(open(p))['samples']:
            st = stagef(R.get(str(a), []), a) if name == dso else 'other'
            per[(s, c)][st] += n
    return {k: {st: v / nr[k] / 4000 for st, v in cnt.items()} for k, cnt in per.items()}
ONLY = os.environ.get('ROUND')
core = load(cd, 'prof-40f29a6', lambda ch, a: core_stage(ch), 'core', ONLY)
ff = load(fd, 'ffmpeg_g', ff_stage, 'ff', ONLY)
# Times are scaled to the BtbN binary's (the timing reference): each stream's stage times over its BtbN total
# (CORE_DIR/*.ff.*.json), FFmpeg's stages as fractions of the attribution build's total.
bt = collections.Counter(); bn = collections.Counter()
for p in glob.glob(f'{cd}/*.ff.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    if ONLY is not None and r != ONLY: continue
    bt[(s, c)] += sum(n for _, _, n in json.load(open(p))['samples']) / 4000; bn[(s, c)] += 1
bt = {k: v / bn[k] for k, v in bt.items()}
rows = {}
for lab in ('sd', '720p', '1080p', '4k', 'all'):
    ks = [k for k in core if k in ff and (lab == 'all' or k[1] == lab)]
    def mean(f): return sum(f(k) for k in ks) / len(ks)
    cs = {st: mean(lambda k: core[k].get(st, 0) / bt[k]) for st in STAGES}
    fs = {st: mean(lambda k: ff[k].get(st, 0) / sum(ff[k].values())) for st in STAGES}
    import statistics
    med = statistics.median(sum(core[k].values()) / bt[k] for k in ks)
    rows[lab] = {'n': len(ks), 'core': cs, 'ff': fs, 'median_ratio': med}
json.dump(rows, open(f'{cd}/stages{".r" + ONLY if ONLY else ""}.json', 'w'), indent=1)
for lab, r in rows.items():
    C, F = sum(r['core'].values()), sum(r['ff'].values())
    E = C - 1.10 * F
    print(f"\n{lab} (n={r['n']}): CoreAV/FFmpeg mean {C/F:.3f}, median {r['median_ratio']:.3f}; excess over 1.10 = {E:.3f} of FFmpeg's time")
    print(f"{'stage':52} {'CoreAV':>7} {'FFmpeg':>7} {'ratio':>6} {'core-1.1ff':>10} {'share of excess':>15}")
    for st in sorted(STAGES, key=lambda s: -(r['core'][s] - 1.10 * r['ff'][s])):
        c, f = r['core'][st], r['ff'][st]
        print(f"{NAMES[st]:52} {c:7.3f} {f:7.3f} {c/f if f else 0:6.2f} {c-1.10*f:10.3f} {100*(c-1.10*f)/E if E else 0:14.0f}%")
