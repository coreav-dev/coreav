"""kitems.py CORE_DIR SYMRE: kernels-crate samples whose outer symbol matches SYMRE, by innermost coreav frame (fn, file),
in time over each stream's FFmpeg total, mean over 32 streams; also by class."""
import collections, glob, json, os, re, sys
cd, symre = sys.argv[1], re.compile(sys.argv[2])
R = json.load(open(f'{cd}/addr.prof-main.json'))
tot = collections.Counter(); nr = collections.Counter()
for p in glob.glob(f'{cd}/*.ff.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    tot[(s, c)] += sum(n for _, _, n in json.load(open(p))['samples']) / 4000; nr[(s, c)] += 1
tot = {k: v / nr[k] for k, v in tot.items()}
out = collections.Counter(); bycls = collections.defaultdict(collections.Counter); nf = collections.Counter()
for p in glob.glob(f'{cd}/*.core.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.'); nf[c] += 1
    for name, a, n in json.load(open(p))['samples']:
        if name != 'prof-main': continue
        ch = R.get(str(a), [])
        if not ch or not symre.search(ch[-1][0]): continue
        fr = next((f for f in ch if 'coreav-' in f[1]), ch[0])
        key = fr[1].rsplit('/', 1)[-1] + ' ' + re.sub(r'<[^<>]*>', '', fr[0])[-70:]
        v = n / 4000 / tot[(s, c)]
        out[key] += v; bycls[key][c] += v
N = len(glob.glob(f'{cd}/*.core.*.json'))
for k, v in out.most_common(30):
    print(f'{v/N:.4f} ' + ' '.join(f'{c}:{bycls[k][c]/nf[c]:.4f}' for c in ('sd','720p','1080p','4k')) + f'  {k}')
