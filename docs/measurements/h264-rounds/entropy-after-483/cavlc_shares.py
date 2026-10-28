"""cavlc_shares.py: per CAVLC stream, the excess over FFmpeg per MB of the MB-level syntax read together (CAVLC
syntax, MB header, MB layer, mvd/ref_idx), the residual, NAL/RBSP and the rest, as shares of its instructions."""
import sys, re, glob, os
sys.path.insert(0, '/tmp/ent2')
exec(open('/tmp/ent2/final.py').read().split("for mode in ('CABAC', 'CAVLC'):")[0])
tot = {os.path.basename(p).split('.')[0]: int(re.search(r'Collected : (\d+)', open(p).read()).group(1)) for p in glob.glob('/tmp/ent2/cg/*.core.log')}
keys = list(G)
grp = {'MB-level syntax': [keys[1], keys[2], keys[3], keys[5]], 'residual': [keys[0]], 'NAL/RBSP': [keys[4]]}
for mode in ('CAVLC', 'CABAC'):
    print(mode)
    for s in sorted(core):
        if (bins[s]['sig'] > 0) != (mode == 'CABAC'): continue
        t = tot[s] / dims[s]; line = f'  {s} {t:7.0f}/MB'
        used = set()
        for name, gs in grp.items():
            cc = sum((G[g][0] for g in gs), []); fc = sum((G[g][1] for g in gs), []); used |= set(cc)
            c = sum(sum(core[s][k]) for k in cc); f = sum(sum(ffr[s][k]) for k in fc)
            line += f'  {name} {c - f:+6.0f} ({(c - f) / t:+.1%})'
        rest = sum(sum(v) for k, v in core[s].items() if k not in used)
        print(line + f'  other {rest:5.0f} ({rest / t:.1%})')
