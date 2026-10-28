"""stageitems.py CORE_DIR FF_DIR FFMPEG_G STAGE: items of a stage per class (CoreAV and FFmpeg), time over FFmpeg total, mean per stream."""
import collections, glob, json, os, sys, bisect
cd, fd, ffg, st0 = sys.argv[1:5]
sys.argv = sys.argv[:4]
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'items.py')).read().split('def agg(')[0])
def agg(d, dso, stagef, itemf, kind, totals):
    R = json.load(open(f'{d}/addr.{dso}.json')); out = collections.defaultdict(collections.Counter); nf = collections.Counter()
    for p in sorted(glob.glob(f'{d}/*.{kind}.*.json')):
        s, c, _, r = os.path.basename(p)[:-5].split('.'); nf[c] += 1
        for name, a, n in json.load(open(p))['samples']:
            if name != dso: continue
            ch = R.get(str(a), [])
            if stagef(ch, a) == st0: out[c][itemf(ch, a)] += n / 4000 / totals[(s, c)]
    return {c: {i: v / nf[c] for i, v in o.items()} for c, o in out.items()}
tot = collections.Counter(); nr = collections.Counter()
for p in glob.glob(f'{fd}/*.ff.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    tot[(s, c)] += sum(n for _, _, n in json.load(open(p))['samples']) / 4000; nr[(s, c)] += 1
tot = {k: v / nr[k] for k, v in tot.items()}
co = agg(cd, 'prof-main', lambda ch, a: core_stage(ch), lambda ch, a: core_item(ch), 'core', tot)
fo = agg(fd, 'ffmpeg_g', ff_stage, ff_item, 'ff', tot)
for c in ('sd', '720p', '1080p', '4k'):
    print(f'== {c} {st0}: CoreAV {sum(co[c].values()):.3f} FFmpeg {sum(fo[c].values()):.3f}')
    a = sorted(((v, i) for i, v in co[c].items()), reverse=True)[:12]
    b = sorted(((v, i) for i, v in fo[c].items()), reverse=True)[:12]
    for x, y in zip(a + [(0, '')] * 12, b + [(0, '')] * 12):
        if x[0] or y[0]: print(f'  {x[0]:6.4f} {x[1][:55]:55} | {y[0]:6.4f} {y[1][:55]}')
