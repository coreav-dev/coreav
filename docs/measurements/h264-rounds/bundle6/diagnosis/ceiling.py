"""ceiling.py CGDIR: the instruction ceiling of bundle 6 from run2/cg.sh's callgrind outputs (CGDIR/<arm>.<stream>.t<N>.out.gz).

One thread: total instructions of p1z (the round's k) over base. Eight threads: the cut of the motion compensation
path's own instructions per instruction of deblock_rows, as a share of base's instructions, for p14 (bundle 5's P1 and
P4) and p1z, per stream and as medians per class.
"""
import subprocess, re, sys, glob, statistics as st, gzip, os, tempfile
D = sys.argv[1]
PAT = re.compile(r'predict_fast|reference_band|KeptViews|predict_list_any|edge_window')
NORM = re.compile(r'deblock_rows')
CAV = {"5185721dda11","6f7799768413","9db41aa16b5a","b71b6b31e54f","ec473c99c538","f9e973f44570"}
cache = {}
def costs(f):
    if f in cache: return cache[f]
    with tempfile.NamedTemporaryFile(delete=False) as t:
        t.write(gzip.open(f).read()); p = t.name
    out = subprocess.run(['callgrind_annotate', '--auto=no', '--inclusive=no', '--threshold=100', p], capture_output=True, text=True).stdout
    os.unlink(p)
    mc = norm = tot = 0
    for l in out.splitlines():
        m = re.match(r'\s*([\d,]+) \([^)]*\)\s+(.*)', l)
        if not m: continue
        v = int(m.group(1).replace(',', '')); name = m.group(2)
        if 'PROGRAM TOTALS' in name: tot = v; continue
        if PAT.search(name): mc += v
        if NORM.search(name): norm += v
    cache[f] = (mc, norm, tot); return cache[f]
streams = sorted({os.path.basename(f).split('.', 1)[1].rsplit('.t8', 1)[0] for f in glob.glob(f'{D}/base.*.t8.out.gz')})
rows = {'t1': [], 'p14': [], 'p1z': []}
cls = {}
for s in streams:
    b1 = costs(f'{D}/base.{s}.t1.out.gz'); z1 = costs(f'{D}/p1z.{s}.t1.out.gz')
    r1 = z1[2] / b1[2]
    b8 = costs(f'{D}/base.{s}.t8.out.gz')
    out = [s.ljust(22), f't1 p1z/base {r1:.4f}']
    rows['t1'].append(r1); cls.setdefault(s.split('.')[2], []).append(s)
    for a in ['p14', 'p1z']:
        c = costs(f'{D}/{a}.{s}.t8.out.gz')
        cut = (b8[0] / b8[1] - c[0] / c[1]) / (b8[2] / b8[1])
        rows[a].append((s, cut)); out.append(f'{a} t8 cut {cut*100:.2f}%')
    print(*out)
for a in ['p14', 'p1z']:
    v = [c for _, c in rows[a]]
    print(a, 't8 MC-path cut as share of base instructions: median %.2f%%' % (st.median(v) * 100),
          ' '.join(f"{k} {st.median([c for s, c in rows[a] if s.split('.')[2] == k])*100:.2f}%" for k in ['sd', '720p', '1080p', '4k']))
print('t1 p1z/base median %.4f cabac %.4f cavlc %.4f' % (st.median(rows['t1']),
      st.median([r for s, r in zip(streams, rows['t1']) if s[:12] not in CAV]), st.median([r for s, r in zip(streams, rows['t1']) if s[:12] in CAV])))
