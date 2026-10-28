import sys, glob, os, pickle, collections
sys.path.insert(0, '/tmp/ent'); from sig import dims
rp = pickle.load(open('/tmp/ent/cav.pkl', 'rb')) if os.path.exists('/tmp/ent/cav.pkl') else None
CATS = [('mb_skip', [(11, 13), (24, 26)]), ('mb_type', [(0, 10), (14, 20), (27, 35)]), ('sub_mb_type', [(21, 23), (36, 39)]),
        ('t8x8', [(399, 401)]), ('mvd', [(40, 53)]), ('ref_idx', [(54, 59)]), ('qp_delta', [(60, 63)]), ('intra modes', [(64, 69)]),
        ('cbp', [(73, 84)]), ('cbf', [(85, 104), (460, 471), (1012, 1023)]), ('res via decision()', [(105, 459), (472, 1011)]),
        ('sig', [(1024, 1024)]), ('last', [(1025, 1025)]), ('abs first', [(1026, 1026)]), ('abs rest', [(1027, 1027)]),
        ('sign (levels)', [(1028, 1028)]), ('Cabac::bypass (mvd sign)', [(1029, 1029)]), ('terminate', [(1030, 1030)]),
        ('all bypass bins', [(1031, 1031)]), ('blocks N=4', [(1032, 1032)]), ('blocks N=16', [(1033, 1033)]), ('blocks N=64', [(1034, 1034)])]
out = {}
for p in glob.glob('/tmp/ent/bins/*.txt'):
    s = os.path.basename(p)[:-4]
    d = {int(a): int(b) for a, b in (x.split(':') for x in open(p).read().split()[1:])}
    out[s] = {k: sum(v for i, v in d.items() if any(a <= i <= b for a, b in r)) / dims[s] for k, r in CATS}
pickle.dump(out, open('/tmp/ent/bins.pkl', 'wb'))
cab = [s for s in out if out[s]['sig'] > 0]
print('CABAC streams', len(cab))
for k, _ in CATS:
    print(f'  {k:28} {sum(out[s][k] for s in cab)/len(cab):8.2f} bins/MB')
reg = lambda s: sum(out[s][k] for k in ('mb_skip','mb_type','sub_mb_type','t8x8','mvd','ref_idx','qp_delta','intra modes','cbp','cbf','res via decision()','sig','last','abs first','abs rest'))
print('  regular decisions', sum(reg(s) for s in cab)/len(cab), ' bypass', sum(out[s]['all bypass bins'] for s in cab)/len(cab))
