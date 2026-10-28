import os, re, sys, statistics as st
INTER = ('predict_list_any', 'predict_whole', 'x86::predict', 'luma_mc', 'chroma_mc', 'window', 'luma_columns', 'motion_compensate',
         'Row16', 'Row8', 'Row4', 'try_map', 'luma_block', 'chroma_block', 'predict::', 'Kernels>::predict', 'avg', 'chroma_pair_mc', 'mc_into', 'luma_rows', 'luma_centre', 'hv')
def load(d, s):
    tot, inter, items = 0, 0, {}
    for line in open(f'{d}/{s}.txt'):
        m = re.match(r'\s*([\d.]+)%\s+(\d+)\s+\[.\]\s+(.*)', line)
        if not m: continue
        n, sym = int(m.group(2)), m.group(3)
        tot += n
        if any(k in sym for k in INTER) and 'deblock' not in sym and 'intra' not in sym and 'idct' not in sym:
            inter += n; items[sym] = items.get(sym, 0) + n
    return tot, inter, items
a, b = sys.argv[1], sys.argv[2]
rows = []
for f in sorted(os.listdir(a)):
    s = f[:-4]
    t1, i1, _ = load(a, s); t8, i8, it8 = load(b, s)
    rows.append((s, t8 / t1, i1 / t1, i8 / t1, (i8 - i1) / t8))
    print(f'{s:24} cpu8/cpu1 {t8/t1:.3f}  inter1 {i1/t1:.3f} inter8 {i8/t1:.3f}  (inter8-inter1)/cpu8 {(i8-i1)/t8:+.3f}')
for c in ('sd', '720p', '1080p', '4k', ''):
    r = [x for x in rows if x[0].endswith(c)]
    print(c or 'all', 'median cpu8/cpu1', round(st.median(x[1] for x in r), 3), 'median excess share', round(st.median(x[4] for x in r), 3))
print('--- inflation inter vs other; excess inter beyond other inflation, share of cpu8')
ex = []
for f in sorted(os.listdir(a)):
    s = f[:-4]
    t1, i1, _ = load(a, s); t8, i8, _ = load(b, s)
    o1, o8 = t1 - i1, t8 - i8
    e = (i8 - i1 * o8 / o1) / t8
    ex.append((s, e)); print(f'{s:24} inter x{i8/i1:.2f} other x{o8/o1:.2f} excess {e:+.3f}')
for c in ('sd', '720p', '1080p', '4k', ''):
    print(c or 'all', round(st.median(x[1] for x in ex if x[0].endswith(c)), 3))
