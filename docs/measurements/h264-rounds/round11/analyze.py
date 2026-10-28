"""python3 -I analyze.py DIR PREFIX -> round 11's ratios between four builds timed in the same rounds.

The reports are DIR/PREFIX.{sd,720p,1080p,4k}.json from scripts/h264_parity.py with the decoders
`base` (main at 555689b), `m` (part M alone: the macroblock records), `p` (part P alone: the
prediction's common block) and `mp` (both). For each pair (mp over base, m over base, p over base,
and each part on top of the other: mp over p for M, mp over m for P), each pairing (discard,
write), overall and per class: the median per-stream ratio with a 95% bootstrap interval (2,000
resamples, random.Random(20261022)) and how many streams are faster; then bars R11-1 to R11-3 and
R11-5 as `docs/design/h264.md` ("Round 11") fixes them, and each build's median ratio to FFmpeg as
information. R11-4 (output) is read from identity.txt and conformance.txt, not here.
"""
import json, random, statistics, sys

d, pre = sys.argv[1], sys.argv[2]
classes = ('sd', '720p', '1080p', '4k')
rows = []
for c in classes:
    rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
builds = ('base', 'm', 'p', 'mp')
ok = [x for x in rows if x.get('seconds') and all(f'{b}_{p}' in x['seconds'] for b in builds for p in ('discard', 'write'))]


def ci(v, seed=20261022, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


out = {'failed': [x['stream'] for x in rows if x not in ok], 'pairs': {}}
for new, old in (('mp', 'base'), ('m', 'base'), ('p', 'base'), ('mp', 'p'), ('mp', 'm')):
    key = f'{new}/{old}'
    out['pairs'][key] = {}
    for p in ('discard', 'write'):
        res = {}
        for lab in ('overall',) + classes:
            sel = [x for x in ok if lab == 'overall' or x['class'] == lab]
            v = [x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in sel]
            lo, hi = ci(v)
            res[lab] = {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
                        'faster': sum(1 for t in v if t < 1)}
        res['per_stream'] = {x['stream']: x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in ok}
        out['pairs'][key][p] = res
out['vs_ffmpeg'] = {b: {p: statistics.median([x['ratio'][f'{b}:{p}'] for x in ok]) for p in ('discard', 'write')}
                    for b in builds}


def bars(new):
    """R11-1 to R11-3 for build `new` over base."""
    pr = out['pairs'][f'{new}/base']
    dis, wri = pr['discard'], pr['write']
    return {
        'R11-1': dis['overall']['median'] <= 0.985 and dis['overall']['ci95'][1] < 1.0,
        'R11-2': wri['overall']['median'] <= 0.985,
        'R11-3': all(dis[c]['median'] <= 1.005 for c in classes),
    }


# R11-5: a part stays where, on top of the other, the upper end of the interval is below 1.00 on
# both pairings.
earns = {part: all(out['pairs'][key][p]['overall']['ci95'][1] < 1.0 for p in ('discard', 'write'))
         for part, key in (('M', 'mp/p'), ('P', 'mp/m'))}
if earns['M'] and earns['P']:
    kept = 'mp'
elif earns['M'] or earns['P']:
    kept = 'm' if earns['M'] else 'p'
else:
    # Neither adds to the other: the round keeps the part that is faster alone, on discard.
    kept = min(('m', 'p'), key=lambda b: out['pairs'][f'{b}/base']['discard']['overall']['median'])
out['R11-5'] = {'earns_its_place': earns, 'build_read': kept}
out['bars'] = {kept: bars(kept)}
json.dump(out, open(f'{d}/{pre}.summary.json', 'w'), indent=1)
print('failed', out['failed'])
for key, pairs in out['pairs'].items():
    for p, res in pairs.items():
        for lab, v in res.items():
            if lab == 'per_stream':
                continue
            print(f"{key:8} {p:7} {lab:8} n={v['n']:2} median {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
for b, v in out['vs_ffmpeg'].items():
    print(f"{b:4} vs FFmpeg: discard {v['discard']:.3f} write {v['write']:.3f}")
print('R11-5 earns its place:', earns, '-> bars read for', kept)
print('bars:', out['bars'])
