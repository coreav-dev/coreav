"""items.py CORE_DIR FF_DIR FFMPEG_G STAGE...: within the stages named, the items (CoreAV: innermost coreav frame, the
kernels crate by symbol; FFmpeg: innermost non-generic frame, assembly by symbol) in time over each stream's FFmpeg
total, averaged over all 32 streams and the rounds."""
import collections, glob, json, os, sys, bisect
cd, fd, ffg = sys.argv[1:4]; want = set(sys.argv[4:])
sys.argv = sys.argv[:4]
exec(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'classify.py')).read().split('def load(')[0])
def core_item(ch):
    fr = next((f for f in ch if 'coreav-' in f[1]), None)
    if fr is None: return '?'
    if 'coreav-kernels-h264' in fr[1]: return 'K ' + ch[-1][0].split('::')[-1][:60]
    return fr[1].rsplit('/', 1)[-1] + ' ' + fr[0].split('>>::')[-1][:50]
def ff_item(ch, a):
    if not ch: return '?'
    k = next((i for i, fr in enumerate(ch) if fr[1].rsplit('/', 1)[-1] not in GENERIC), 0)
    fn, f = ch[k][0], ch[k][1].rsplit('/', 1)[-1]
    if f.endswith('.asm') or fn == '??':
        i = bisect.bisect_right(sa, a + delta) - 1
        return 'asm ' + (syms[i][1] if i >= 0 else '?')
    return f + ' ' + fn[:50]
def agg(d, dso, stagef, itemf, kind, totals):
    R = json.load(open(f'{d}/addr.{dso}.json')); out = collections.Counter(); files = sorted(glob.glob(f'{d}/*.{kind}.*.json'))
    for p in files:
        s, c, _, r = os.path.basename(p)[:-5].split('.')
        for name, a, n in json.load(open(p))['samples']:
            if name != dso: continue
            ch = R.get(str(a), []); st = stagef(ch, a)
            if st in want: out[(st, itemf(ch, a))] += n / 4000 / totals[(s, c)]
    return out, len(files)
tot = collections.Counter(); nr = collections.Counter()
for p in glob.glob(f'{fd}/*.ff.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    tot[(s, c)] += sum(n for _, _, n in json.load(open(p))['samples']) / 4000; nr[(s, c)] += 1
tot = {k: v / nr[k] for k, v in tot.items()}
co, nc = agg(cd, 'prof-40f29a6', lambda ch, a: core_stage(ch), lambda ch, a: core_item(ch), 'core', tot)
fo, nf = agg(fd, 'ffmpeg_g', ff_stage, ff_item, 'ff', tot)
for st in want:
    print(f'== {st}: CoreAV {sum(v for (s,_),v in co.items() if s==st)/nc:.3f}, FFmpeg {sum(v for (s,_),v in fo.items() if s==st)/nf:.3f} (of FFmpeg total, mean per stream)')
    a = sorted(((v / nc, i) for (s, i), v in co.items() if s == st), reverse=True)[:16]
    b = sorted(((v / nf, i) for (s, i), v in fo.items() if s == st), reverse=True)[:16]
    for x, y in zip(a + [(0, '')] * 16, b + [(0, '')] * 16):
        if x[0] or y[0]: print(f'  {x[0]:6.4f} {x[1][:58]:58} | {y[0]:6.4f} {y[1][:58]}')
