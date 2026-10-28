"""recon_split.py CORE_DIR FF_DIR FFMPEG_G: reconstruction split into intra prediction, transforms, scaling and glue per class."""
import collections, glob, json, os, sys, re, bisect
cd, fd, ffg = sys.argv[1:4]
sys.argv = sys.argv[:4]
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'items.py')).read().split('def agg(')[0])
def part(i):
    l = i.lower()
    if re.search(r'intra|pred(4|8|16)|pred_|predict_luma|xchg', l): return 'intra'
    if re.search(r'idct|transform|dc_add', l): return 'idct'
    if re.search(r'apply|scale|dequant|level_scales|recon.rs new', l): return 'scale'
    return 'glue'
tot = collections.Counter(); nr = collections.Counter()
for p in glob.glob(f'{fd}/*.ff.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    tot[(s, c)] += sum(n for _, _, n in json.load(open(p))['samples']) / 4000; nr[(s, c)] += 1
tot = {k: v / nr[k] for k, v in tot.items()}
def agg(d, dso, stagef, itemf, kind):
    R = json.load(open(f'{d}/addr.{dso}.json')); out = collections.defaultdict(collections.Counter); nf = collections.Counter(); items = collections.defaultdict(collections.Counter)
    for p in sorted(glob.glob(f'{d}/*.{kind}.*.json')):
        s, c, _, r = os.path.basename(p)[:-5].split('.'); nf[c] += 1; nf['all'] += 1
        for name, a, n in json.load(open(p))['samples']:
            if name != dso: continue
            ch = R.get(str(a), [])
            if stagef(ch, a) == 'recon':
                it = itemf(ch, a); q = part(it); v = n / 4000 / tot[(s, c)]
                out[c][q] += v; out['all'][q] += v; items[q][it] += v
    return {c: {q: v / nf[c] for q, v in o.items()} for c, o in out.items()}, items, nf['all']
co, ci, cn = agg(cd, 'prof-main', lambda ch, a: core_stage(ch), lambda ch, a: core_item(ch), 'core')
fo, fi, fn_ = agg(fd, 'ffmpeg_g', ff_stage, ff_item, 'ff')
for c in ('sd', '720p', '1080p', '4k', 'all'):
    print(c, '  '.join(f'{q} {co[c].get(q,0):.4f}/{fo[c].get(q,0):.4f}' for q in ('intra', 'idct', 'scale', 'glue')))
for q in ('intra', 'glue'):
    print('--', q, 'CoreAV:', ', '.join(f'{i} {v/cn:.4f}' for i, v in ci[q].most_common(8)))
    print('--', q, 'FFmpeg:', ', '.join(f'{i} {v/fn_:.4f}' for i, v in fi[q].most_common(10)))
