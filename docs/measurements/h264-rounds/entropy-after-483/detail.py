"""detail.py CAT [MODE]: a category's instructions per MB by innermost CoreAV frame and line, mean over the streams."""
import sys, collections, pickle
sys.path.insert(0, '/tmp/ent2')
exec(open('/tmp/ent2/final.py').read().split("core = collections.defaultdict")[0])
want = sys.argv[1]; mode = sys.argv[2] if len(sys.argv) > 2 else 'CABAC'
ss = [s for (s, c) in per if (bins[s]['sig'] > 0) == (mode == 'CABAC')]
byfn = collections.Counter(); byline = collections.Counter()
for (s, c), d in per.items():
    if s not in ss: continue
    for a, ir in d.items():
        ch = chains.get(a, [])
        st = core_stage([[x[0], x[1].rsplit(':', 2)[0]] for x in ch])
        if st not in ('entropy', 'mb') or cat(ch) != want: continue
        sh = short(ch)
        byfn[' > '.join(sh[-3:])] += ir / dims[s] / len(ss)
        inner = [f for f in ch if 'coreav-' in f[1]]
        if inner: byline[inner[0][1].rsplit('/', 1)[-1]] += ir / dims[s] / len(ss)
for k, v in byfn.most_common(14): print(f'{v:7.1f}  {k[:150]}')
print('--- lines')
for k, v in byline.most_common(25): print(f'{v:7.1f}  {k}')
