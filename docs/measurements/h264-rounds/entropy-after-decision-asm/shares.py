"""shares.py: each group's CoreAV instructions and its excess over FFmpeg, per MB, as a share of the stream's
CoreAV instructions (callgrind totals of the same runs), per stream and per class median."""
import sys, re, glob, os, statistics, pickle, collections
sys.path.insert(0, '/tmp/ent2')
exec(open('/tmp/ent2/final.py').read().split("for mode in ('CABAC', 'CAVLC'):")[0])
tot = {}
for p in glob.glob('/tmp/ent2/cg/*.core.log'):
    s = os.path.basename(p).split('.')[0]
    m = re.search(r'Collected : (\d+)', open(p).read()); tot[s] = int(m.group(1)) / dims[s]
cls = {os.path.basename(p).split('.')[0]: os.path.basename(p).split('.')[1] for p in glob.glob('/tmp/ent2/cg/*.core.out')}
for g, (cc, fc) in G.items():
    rows = []
    for s in core:
        c = sum(sum(core[s][k]) for k in cc); f = sum(sum(ffr[s][k]) for k in fc)
        rows.append((s, cls[s], 'CABAC' if bins[s]['sig'] > 0 else 'CAVLC', c / tot[s], (c - f) / tot[s]))
    print(f'\n{g[:70]}')
    for mode in ('CABAC', 'CAVLC'):
        r = [x for x in rows if x[2] == mode]
        if not r: continue
        line = f'  {mode} n={len(r):2} share med {statistics.median(x[3] for x in r):6.2%} excess med {statistics.median(x[4] for x in r):+6.2%} max {max(x[4] for x in r):+6.2%}'
        for c in ('sd', '720p', '1080p', '4k'):
            rc = [x[4] for x in r if x[1] == c]
            if rc: line += f'  {c} {statistics.median(rc):+.2%}'
        print(line)
    print('  all classes (32):', '  '.join(f"{c} {statistics.median(x[4] for x in rows if x[1] == c):+.2%}" for c in ('sd', '720p', '1080p', '4k')))
print('\ninstructions per MB, median:', statistics.median(tot.values()))
