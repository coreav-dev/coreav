"""callers_items.py CG_DIR: run 2's libc and kernel growth (callers.py's pairs) grouped by caller, per class (mean over
the class's streams in run 2), as % of our one-thread cycles."""
import collections, glob, os, re, statistics as st, sys
d = sys.argv[1]
here = os.path.dirname(os.path.abspath(__file__))
ns = {'__name__': 'callers'}
src = open(os.path.join(here, 'callers.py')).read()
exec(src[:src.index('agg=collections.Counter()')], ns)
GROUPS = [('aprons: memmove in copy_top', r'copy_top'),
          ('band publish: wake-ups', r'publish|SyncWaker|notify|unpark'),
          ('band waits', r'wait_for|wait|park'),
          ('first touch: BandData::fit', r'BandData3fit|into_bands|finish_grow'),
          ('first touch: predicted blocks', r'predict10luma_block|predict12chroma_block'),
          ('other', r'.')]
res = collections.defaultdict(lambda: collections.defaultdict(list))
for p in sorted(glob.glob(f'{d}/*.1.txt')):
    base = p[:-6]
    c = os.path.basename(base).split('.')[2]
    s1, s8 = ns['parse'](base + '.1.txt'), ns['parse'](base + '.8.txt')
    t1 = sum(w for w, _ in s1)
    g = collections.Counter()
    for sign, ss in ((-1, s1), (1, s8)):
        for w, fr in ss:
            k = fr and ns['key'](fr)
            if k:
                name = next(n for n, rx in GROUPS if re.search(rx, k[1]))
                g[name] += sign * w / t1
    for n, _ in GROUPS:
        res[n][c].append(g[n])
CL = ['sd', '720p', '1080p', '4k']
print(f'{"caller group (libc and kernel samples)":40s} {"all":>6s} ' + ' '.join(f'{c:>6s}' for c in CL))
for n, _ in GROUPS:
    cm = [st.mean(res[n][c]) for c in CL]
    print(f'{n:40s} {100 * st.mean(cm):6.2f} ' + ' '.join(f'{100 * x:6.2f}' for x in cm))
print('streams:', {c: len(res['other'][c]) for c in CL})
