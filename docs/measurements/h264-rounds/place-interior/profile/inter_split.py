"""inter_split.py CORE_DIR FF_DIR FFMPEG_G: inter prediction split into luma kernel, chroma kernel, weighting, edge
fetch, prefetch and glue, per class and over all 32, in time over each stream's FFmpeg total (mean per stream)."""
import collections, glob, json, os, re, sys
cd, fd, ffg = sys.argv[1:4]
sys.argv = [sys.argv[0], cd, fd, ffg, 'inter']
src = open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'items.py')).read().split('def agg(')[0]
exec(src)
def part(item, core):
    i = item.lower()
    if re.search(r'chroma', i): return 'chroma'
    if re.search(r'combine|weigh', i): return 'weight'
    if re.search(r'edge_window| window|emu_edge|emulated', i): return 'edge'
    if re.search(r'prefetch|start_mb', i): return 'prefetch'
    if re.search(r'luma|average>|put>|qpel|pixels|interpolat|copy_rows|\{closure#0\}<8>', i): return 'luma'
    return 'glue'
PARTS = ['luma', 'chroma', 'weight', 'edge', 'prefetch', 'glue']
def per_stream(d, dso, stagef, itemf, kind, core):
    R = json.load(open(f'{d}/addr.{dso}.json')); out = collections.defaultdict(collections.Counter); nr = collections.Counter()
    for p in sorted(glob.glob(f'{d}/*.{kind}.*.json')):
        s, c, _, r = os.path.basename(p)[:-5].split('.'); nr[(s, c)] += 1
        for name, a, n in json.load(open(p))['samples']:
            if name != dso: continue
            ch = R.get(str(a), [])
            if stagef(ch, a) == 'inter': out[(s, c)][part(itemf(ch, a), core)] += n / 4000
    return {k: {q: v / nr[k] for q, v in cnt.items()} for k, cnt in out.items()}
tot = collections.Counter(); nr = collections.Counter()
own = collections.Counter(); onr = collections.Counter()
for p in glob.glob(f'{fd}/*.ff.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    own[(s, c)] += sum(n for _, _, n in json.load(open(p))['samples']) / 4000; onr[(s, c)] += 1
own = {k: v / onr[k] for k, v in own.items()}
for p in glob.glob(f'{cd}/*.ff.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    tot[(s, c)] += sum(n for _, _, n in json.load(open(p))['samples']) / 4000; nr[(s, c)] += 1
tot = {k: v / nr[k] for k, v in tot.items()}
C = per_stream(cd, 'prof-main', lambda ch, a: core_stage(ch), lambda ch, a: core_item(ch), 'core', True)
F = per_stream(fd, 'ffmpeg_g', ff_stage, ff_item, 'ff', False)
res = {}
for lab in ('sd', '720p', '1080p', '4k', 'all'):
    ks = [k for k in tot if lab == 'all' or k[1] == lab]
    res[lab] = {q: (sum(C.get(k, {}).get(q, 0) / tot[k] for k in ks) / len(ks), sum(F.get(k, {}).get(q, 0) / own[k] for k in ks) / len(ks)) for q in PARTS}
    print(lab, '  '.join(f"{q} {a:.3f}/{b:.3f}" for q, (a, b) in res[lab].items()))
json.dump(res, open(f'{cd}/inter_split.json', 'w'), indent=1)
