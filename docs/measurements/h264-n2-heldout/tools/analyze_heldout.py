"""Summarise the held-out reports: python3 -I analyze_heldout.py REPORTS_DIR"""
import json, random, statistics, sys, collections
d = sys.argv[1]; classes = ('sd', '720p', '1080p', '4k'); out = {}
def ci(v, seed=20261021, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025*n)], m[int(.975*n)-1]
for arm in ('ff7', 'ff9'):
    rows = []
    for c in classes:
        r = json.load(open(f'{d}/{c}.{arm}.json'))
        rows += r['streams']
    out[arm] = {'streams': len(rows), 'failed': [x['stream'] for x in rows if 'ratio' not in x or not x.get('ratio')]}
    ok = [x for x in rows if x.get('ratio')]
    for p in ('discard', 'write'):
        res = {}
        for label, sel in [('overall', ok)] + [(c, [x for x in ok if x['class'] == c]) for c in classes]:
            v = [x['ratio'][f'main:{p}'] for x in sel]
            lo, hi = ci(v)
            res[label] = {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
                          'share_faster': sum(1 for t in v if t < 1) / len(v), 'faster': sum(1 for t in v if t < 1),
                          'min': min(v), 'max': max(v), 'bar_1.15_met': statistics.median(v) <= 1.15}
        out[arm][p] = res
    out[arm]['per_stream'] = {x['stream']: {p: x['ratio'][f'main:{p}'] for p in ('discard', 'write')} for x in ok}
    out[arm]['ours_seconds'] = {x['stream']: {p: x['seconds'][f'main_{p}'] for p in ('discard', 'write')} for x in ok}
    out[arm]['ff_seconds'] = {x['stream']: {'null': x['seconds']['ff_null'], 'raw': x['seconds']['ff_raw']} for x in ok}
# ffmpeg 9 over 7 and ours arm-to-arm
common = [s for s in out['ff7']['per_stream'] if s in out['ff9']['per_stream']]
cross = {}
for key, f in (('ff9_over_ff7_null', lambda s: out['ff9']['ff_seconds'][s]['null'] / out['ff7']['ff_seconds'][s]['null']),
               ('ff9_over_ff7_raw', lambda s: out['ff9']['ff_seconds'][s]['raw'] / out['ff7']['ff_seconds'][s]['raw']),
               ('ours_discard_arm2_over_arm1', lambda s: out['ff9']['ours_seconds'][s]['discard'] / out['ff7']['ours_seconds'][s]['discard']),
               ('ours_write_arm2_over_arm1', lambda s: out['ff9']['ours_seconds'][s]['write'] / out['ff7']['ours_seconds'][s]['write'])):
    v = [f(s) for s in common]; lo, hi = ci(v); cross[key] = {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi]}
out['cross'] = cross
json.dump(out, open(f'{d}/summary.json', 'w'), indent=1)
for arm in ('ff7', 'ff9'):
    print(arm, 'streams', out[arm]['streams'], 'failed', out[arm]['failed'])
    for p in ('discard', 'write'):
        for k, v in out[arm][p].items():
            print(f"  {p:7} {k:8} n={v['n']:2} median {v['median']:.3f} [{v['ci95'][0]:.3f},{v['ci95'][1]:.3f}] faster {v['faster']}/{v['n']} range {v['min']:.2f}-{v['max']:.2f}")
print(json.dumps(cross, indent=0))
