"""core_split.py CORE_DIR FFMPEG: CoreAV inter split only (inter_split.py's parts), time over each stream's FFmpeg total from the same run."""
import collections, glob, json, os, re, sys
cd, ffg = sys.argv[1:3]
P = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'profile', '')
sys.argv = [sys.argv[0], cd, cd, ffg, 'inter']
exec(open(P + "items.py").read().split("def agg(")[0].replace("os.path.dirname(os.path.abspath(__file__))", repr(P[:-1])))
src = open(P + 'inter_split.py').read()
exec(src[src.index('def part('):src.index('tot = collections.Counter()')])
tot = collections.Counter(); nr = collections.Counter()
for p in glob.glob(f'{cd}/*.ff.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    tot[(s, c)] += sum(n for _, _, n in json.load(open(p))['samples']) / 4000; nr[(s, c)] += 1
tot = {k: v / nr[k] for k, v in tot.items()}
C = per_stream(cd, 'prof-main', lambda ch, a: core_stage(ch), lambda ch, a: core_item(ch), 'core', True)
items = collections.Counter()
R = json.load(open(f'{cd}/addr.prof-main.json'))
for p in glob.glob(f'{cd}/*.core.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    for name, a, n in json.load(open(p))['samples']:
        if name != 'prof-main': continue
        ch = R.get(str(a), [])
        if core_stage(ch) == 'inter': items[(part(core_item(ch), True), core_item(ch))] += n / 4000 / tot[(s, c)] / 32
for lab in ('sd', '720p', '1080p', '4k', 'all'):
    ks = [k for k in tot if lab == 'all' or k[1] == lab]
    print(lab, '  '.join(f"{q} {sum(C.get(k, {}).get(q, 0) / tot[k] for k in ks) / len(ks):.3f}" for q in PARTS))
for (q, i), v in items.most_common(30): print(f'  {v:.4f} {q:8} {i}')
