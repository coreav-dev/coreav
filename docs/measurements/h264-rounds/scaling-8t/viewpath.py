"""viewpath.py RUN_DIR PROF_BIN: a reading made after run 1 (not an item of plan S2): the growth, one thread to eight,
of the path a banded reference takes for every block in `predict_fast_body` (acf3ee2f's inter.rs 1295 to 1310: the
`whole` lookup that misses, the call to `reference_band` and its return) with `reference_band` and the `band`
functions it calls; as % of our one-thread cycles, mean per class, all = mean of the class means."""
import collections, glob, json, os, re, statistics as st, subprocess, sys
run, prof = sys.argv[1], sys.argv[2]
pname = os.path.basename(prof)
recs = collections.defaultdict(list)
for p in sorted(glob.glob(f'{run}/rec/*.json')):
    s, a, _ = os.path.basename(p).split('.')[:3]
    if a.startswith('us'):
        recs[(s, a)].append(json.load(open(p)))
tim = json.load(open(f'{run}/timing.json'))
cls = {f.split('.')[0]: f.split('.')[2] for f in tim}
addrs = sorted({k for v in recs.values() for r in v for l in r.values() for o, k, _ in l if o == pname and k >= 0})
delta = 0
for l in subprocess.run(['readelf', '-lW', prof], capture_output=True, text=True).stdout.splitlines():
    p = l.split()
    if p and p[0] == 'LOAD' and 'E' in p[6:-1]:
        delta = int(p[2], 16) - int(p[1], 16)
        break
txt = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', prof],
                     input='\n'.join(hex(a + delta) for a in addrs) + '\n', capture_output=True, text=True).stdout
path = set()
for a, b in zip(addrs, txt.split('\n\n')):
    ls = [l for l in b.strip().split('\n') if l]
    ch = [(ls[i], ls[i + 1]) for i in range(0, len(ls) - 1, 2)]
    for fn, loc in ch:
        if 'reference_band' in fn or re.search(r'(Interior|Planes)>::band$', fn):
            path.add(a); break
        if fn.startswith('predict_fast_body') and '/inter.rs:' in loc:
            line = int(loc.split('/inter.rs:')[1].split(':')[0])
            if 1295 <= line <= 1310:
                path.add(a)
            break
res = collections.defaultdict(list)
for s in sorted(cls):
    w = {}
    for a in ('us1', 'us8'):
        tot = vp = 0
        for r in recs[(s, a)]:
            for ev, l in r.items():
                if ev.startswith('cycles'):
                    for o, k, n in l:
                        tot += n / len(recs[(s, a)])
                        if o == pname and k in path:
                            vp += n / len(recs[(s, a)])
        w[a] = (tot, vp)
    res[cls[s]].append((w['us8'][1] - w['us1'][1]) / w['us1'][0])
cm = {c: st.mean(v) for c, v in res.items()}
print(f'view path growth, % of our 1T cycles: all {100 * st.mean(cm.values()):.2f} '
      + ' '.join(f'{c} {100 * cm[c]:.2f}' for c in ('sd', '720p', '1080p', '4k')))
