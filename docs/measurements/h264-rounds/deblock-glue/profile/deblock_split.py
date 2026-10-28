"""deblock_split.py CORE_DIR FF_DIR FFMPEG_G: the deblocking stage split into filter kernels (luma, chroma) and glue
(boundary strengths, thresholds, edge loop and plane addressing, edge views), per class and over all 32 streams, in
time over each stream's FFmpeg total (mean per stream, both rounds), with the items of each part.

CoreAV, by the innermost frame in a coreav- crate: the kernels crate's arithmetic files are the kernels (luma or chroma
by the functions of the inline chain), its views.rs, lib.rs and simd/mod.rs (EdgeMut construction and checks, the
dispatch) are edge views; in deblock.rs, closures go to the function that holds them, then strengths, edge_strengths,
differing_motion_strength, same_motion, spread, column and row are strengths; Thresholds::new, params, luma_qp and
chroma_qp thresholds; the rest (deblock_row, deblock_rows) the edge loop.

FFmpeg 9.0.2 (symbol table and debug information only, ADR 0037): the deblock_* and loop_filter assembly but the
strength one are the kernels (luma or chroma by the symbol), loop_filter_strength and check_mv and the fill_filter_caches
functions strengths, backup_mb_border the border copy, and every other deblocking function the edge loop (its alpha,
beta and tc0 lookups are inlined there, so FFmpeg has no thresholds part of its own)."""
import collections, glob, json, os, re, sys
cd, fd, ffg = sys.argv[1:4]
sys.argv = [sys.argv[0], cd, fd, ffg, 'deblock']
src = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), '../../profile/items.py')).read().split('def agg(')[0].replace("'classify.py'", "'../../profile/classify.py'")
exec(src)
PARTS = ['k-luma', 'k-chroma', 'strength', 'thresholds', 'loop', 'views', 'border', 'other']
STRENGTH = {'strengths', 'edge_strengths', 'differing_motion_strength', 'same_motion', 'spread', 'column', 'row'}
THRESH = {'new', 'params', 'luma_qp', 'chroma_qp'}
def bare(fn):
    m = fn
    while re.search(r'<[^<>]*>', m):
        m = re.sub(r'<[^<>]*>', '', m)
    return m.split('::')[-1]
def core_part(ch):
    i = next((i for i, f in enumerate(ch) if 'coreav-' in f[1]), None)
    if i is None:
        return 'other', '?'
    fn, path = ch[i][0], ch[i][1]
    f = path.rsplit('/', 1)[-1]
    if 'coreav-kernels-h264' in path:
        if f in ('views.rs', 'lib.rs') or path.endswith('simd/mod.rs'):
            return 'views', f + ' ' + bare(fn)
        names = ' '.join(c[0] for c in ch)
        kind = 'k-chroma' if 'chroma' in names else 'k-luma' if 'luma' in names else 'k-luma'
        return kind, f + ' ' + bare(fn)
    if f != 'deblock.rs':
        return 'other', f + ' ' + bare(fn)
    # A closure belongs to the deblock.rs function that holds it.
    for fn2, p2 in ch[i:]:
        b = bare(fn2)
        if p2.rsplit('/', 1)[-1] != 'deblock.rs' or b.startswith('{closure'):
            continue
        if b in STRENGTH: return 'strength', b
        if b in THRESH: return 'thresholds', b
        return 'loop', b
    return 'loop', bare(fn)
def ff_part(ch, a):
    it = ff_item(ch, a)
    if it.startswith('asm '):
        s = it[4:]
        if 'strength' in s: return 'strength', s
        if 'chroma' in s: return 'k-chroma', s
        if re.search(r'deblock|loop_filter', s): return 'k-luma', s
        return 'other', s
    fn = it.split(' ', 1)[-1]
    if re.search(r'fill_filter_caches|check_mv|strength', fn): return 'strength', it
    if 'backup_mb_border' in fn: return 'border', it
    if re.search(r'_c$|_c_|template', it) and re.search(r'luma|chroma', fn):
        return ('k-chroma' if 'chroma' in fn else 'k-luma'), it
    return 'loop', it
def per_stream(d, dso, stagef, partf, kind):
    R = json.load(open(f'{d}/addr.{dso}.json'))
    out = collections.defaultdict(collections.Counter); items = collections.Counter(); nr = collections.Counter()
    for p in sorted(glob.glob(f'{d}/*.{kind}.*.json')):
        s, c, _, r = os.path.basename(p)[:-5].split('.'); nr[(s, c)] += 1
        for name, a, n in json.load(open(p))['samples']:
            if name != dso: continue
            ch = R.get(str(a), [])
            if stagef(ch, a) != 'deblock': continue
            q, it = partf(ch, a)
            out[(s, c)][q] += n / 4000
            items[(q, it, s, c)] += n / 4000
    return ({k: {q: v / nr[k] for q, v in cnt.items()} for k, cnt in out.items()},
            {k: v / nr[(k[2], k[3])] for k, v in items.items()})
def totals(d, kind):
    t = collections.Counter(); n = collections.Counter()
    for p in glob.glob(f'{d}/*.{kind}.*.json'):
        s, c, _, r = os.path.basename(p)[:-5].split('.')
        t[(s, c)] += sum(x for _, _, x in json.load(open(p))['samples']) / 4000; n[(s, c)] += 1
    return {k: v / n[k] for k, v in t.items()}
bt = totals(cd, 'ff')     # BtbN build, the timing reference
own = totals(fd, 'ff')    # the attribution build
C, CI = per_stream(cd, 'prof-main', lambda ch, a: core_stage(ch), lambda ch, a: core_part(ch), 'core')
F, FI = per_stream(fd, 'ffmpeg_g', ff_stage, ff_part, 'ff')
res = {}
print('time over FFmpeg total (BtbN), mean per stream; CoreAV / FFmpeg')
print(f"{'class':6} " + ' '.join(f'{q:>15}' for q in PARTS) + f" {'total':>15}")
for lab in ('sd', '720p', '1080p', '4k', 'all'):
    ks = [k for k in bt if k in own and (lab == 'all' or k[1] == lab)]
    res[lab] = {q: (sum(C.get(k, {}).get(q, 0) / bt[k] for k in ks) / len(ks),
                    sum(F.get(k, {}).get(q, 0) / own[k] for k in ks) / len(ks)) for q in PARTS}
    tc = sum(a for a, _ in res[lab].values()); tf = sum(b for _, b in res[lab].values())
    print(f'{lab:6} ' + ' '.join(f'{a:.4f}/{b:.4f}' for a, b in res[lab].values()) + f' {tc:.4f}/{tf:.4f}')
ks = [k for k in bt if k in own]
for name, I, tot in (('CoreAV', CI, bt), ('FFmpeg', FI, own)):
    agg = collections.Counter()
    for (q, it, s, c), v in I.items():
        agg[(q, it)] += v / tot[(s, c)] / len(ks)
    print(f'\n{name} items (all 32):')
    for (q, it), v in sorted(agg.items(), key=lambda x: -x[1])[:30]:
        print(f'  {v:.4f} {q:10} {it[:70]}')
json.dump(res, open(f'{cd}/deblock_split.json', 'w'), indent=1)
