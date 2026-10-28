# recon.py: per stream, shares of the instructions in recon.rs by function (lines at main 2b31addd), inlined included,
# and the kernel calls made from the reconstruction functions (inclusive)
import json, glob, os, statistics as st
R = [('scale', 25, 35), ('new', 73, 95), ('apply', 98, 102), ('prepare', 129, 178), ('chroma_qp', 181, 185),
     ('offs', 189, 198), ('nbs', 203, 218), ('r_intra', 222, 275), ('luma_dc', 278, 322), ('r_inter', 325, 345),
     ('res4', 348, 371), ('r_chroma', 374, 411), ('chroma_dc', 414, 434)]
K = ['intra4x4_mb', 'intra8x8_mb', 'intra16x16', 'intra_chroma_mb', 'chroma_residual_mb', 'idct4x4_add_quad', 'idct8x8_add']
rows = []
for f in sorted(glob.glob(os.path.expanduser('~/h264work/r33/cg/*.lines.json'))):
    s = os.path.basename(f).split('.out')[0]; d = json.load(open(f)); t = d['total']
    part = {n: 0 for n, _, _ in R}; other = 0
    for k, v in d['lines'].items():
        ln = int(k.split(':')[1])
        for n, a, b in R:
            if a <= ln <= b: part[n] += v; break
        else: other += v
    ker = {k: sum(e[2] for e in d['edge'] if 'recon' not in e[1] and e[1].endswith('::' + k) and ('reconstruct' in e[0] or 'residual_4x4' in e[0] or 'decode_mb' in e[0] or 'decode_c' in e[0] or 'slice' in e[0])) for k in K}
    rows.append((s, t, {n: v / t * 100 for n, v in part.items()}, other / t * 100, {k: v / t * 100 for k, v in ker.items()}))
hdr = [n for n, _, _ in R]
print('stream'.ljust(22), ' '.join(f'{h:>9}' for h in hdr), ' other', ' '.join(f'{k[:9]:>9}' for k in K))
for s, t, p, o, k in rows:
    print(s.ljust(22), ' '.join(f'{p[h]:9.3f}' for h in hdr), f'{o:6.3f}', ' '.join(f'{k[x]:9.3f}' for x in K))
for c in ('sd', '720p', '1080p', '4k'):
    rs = [r for r in rows if r[0].endswith('.' + c)]
    print(c.ljust(22), ' '.join(f'{st.median(r[2][h] for r in rs):9.3f}' for h in hdr), '      ', ' '.join(f'{st.median(r[4][x] for r in rs):9.3f}' for x in K))
