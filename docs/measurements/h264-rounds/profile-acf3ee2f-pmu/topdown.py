"""topdown.py RES: the top-down metrics (TopdownL1 and TopdownL2, perf stat -M) of CoreAV and the BtbN build per
stream (RES/topdown/<stream>.<class>.<dec>.csv). Each metric is printed as a share of the decoder's own slots (mean
over streams) and as slots over FFmpeg's total slots of the same stream (mean over streams), so that the second column
pair shows where CoreAV's extra slots go."""
import collections, glob, os, statistics, sys

res = sys.argv[1]
data = collections.defaultdict(dict)
for p in glob.glob(f'{res}/topdown/*.csv'):
    s, c, dec = os.path.basename(p)[:-4].split('.')
    m, slots = {}, None
    for line in open(p):
        f = line.rstrip('\n').split(',')
        if len(f) >= 7 and f[6].strip().startswith('%  tma_'):
            m[f[6].strip()[3:]] = float(f[5]) / 100
        if len(f) >= 3 and f[2] == 'TOPDOWN.SLOTS' and slots is None:
            slots = float(f[0])
    data[(s, c)][dec] = (slots, m)
names = sorted({k for v in data.values() for _, (sl, m) in v.items() for k in m})
order = ['tma_retiring', 'tma_light_operations', 'tma_heavy_operations', 'tma_bad_speculation',
         'tma_branch_mispredicts', 'tma_machine_clears', 'tma_frontend_bound', 'tma_fetch_latency',
         'tma_fetch_bandwidth', 'tma_backend_bound', 'tma_core_bound', 'tma_memory_bound']
names = [n for n in order if n in names] + [n for n in names if n not in order]
for lab in ('sd', '720p', '1080p', '4k', 'all'):
    ks = [k for k in data if lab in ('all', k[1]) and 'core' in data[k] and 'ffb' in data[k]]
    rat = statistics.median(data[k]['core'][0] / data[k]['ffb'][0] for k in ks)
    print(f'\n{lab} (n={len(ks)}): CoreAV slots / FFmpeg slots, median {rat:.3f}')
    print(f"  {'metric':26} {'share CoreAV':>12} {'share FFmpeg':>12} {'CoreAV/FFslots':>14} {'FF/FFslots':>10} {'excess':>7}")
    for n in names:
        sc = statistics.mean(data[k]['core'][1].get(n, 0) for k in ks)
        sf = statistics.mean(data[k]['ffb'][1].get(n, 0) for k in ks)
        ac = statistics.mean(data[k]['core'][1].get(n, 0) * data[k]['core'][0] / data[k]['ffb'][0] for k in ks)
        af = statistics.mean(data[k]['ffb'][1].get(n, 0) for k in ks)
        print(f'  {n:26} {sc:12.3f} {sf:12.3f} {ac:14.3f} {af:10.3f} {ac - af:7.3f}')
