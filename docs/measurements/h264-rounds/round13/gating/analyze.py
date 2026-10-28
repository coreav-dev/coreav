"""python3 -I analyze.py DIR PREFIX [PREFIX8] -> round 13's ratios between four builds timed in the same rounds.

The reports are DIR/PREFIX.{sd,720p,1080p,4k}.json from scripts/h264_parity.py with the decoders
`base` (main), `g` (part G alone: the common prediction block and the unweighted slice's shortcut),
`r` (part R alone: the reference's lines requested ahead) and `gr` (both). For each pair (gr, g and r
over base, and each part on top of the other: gr over r for G, gr over g for R), each pairing
(discard, write), overall,
per class and over the 720p, 1080p and 4k classes together (`gated`: the streams whose pictures are
large enough for the hints, by the size rule) and over 1080p and 4k (`large`): the median per-stream
ratio with a 95% bootstrap interval (2,000 resamples, random.Random(20261024)) and how many streams
are faster; then the bars of docs/design/h264.md ("Round 13"), each build's median ratio to FFmpeg
as information, and every stream's ratio. DIR/PREFIX8.{sd,720p,1080p,4k}.json, the run with eight
threads of `base` and `g`, gives R13-4 where the build kept holds part G. R13-6 (output) is read
from identity.txt and the conformance report, not here.
"""
import json, random, statistics, sys

d, pre = sys.argv[1], sys.argv[2]
classes = ('sd', '720p', '1080p', '4k')
groups = {'gated': ('720p', '1080p', '4k'), 'large': ('1080p', '4k')}
rows = []
for c in classes:
    rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
builds = ('base', 'g', 'r', 'gr')
ok = [x for x in rows if x.get('seconds') and all(f'{b}_{p}' in x['seconds'] for b in builds for p in ('discard', 'write'))]


def ci(v, seed=20261024, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def select(lab):
    if lab == 'overall':
        return ok
    if lab in groups:
        return [x for x in ok if x['class'] in groups[lab]]
    return [x for x in ok if x['class'] == lab]


out = {'failed': [x['stream'] for x in rows if x not in ok], 'pairs': {}}
for new, old in (('gr', 'base'), ('g', 'base'), ('r', 'base'), ('gr', 'r'), ('gr', 'g')):
    key = f'{new}/{old}'
    out['pairs'][key] = {}
    for p in ('discard', 'write'):
        res = {}
        for lab in ('overall',) + classes + tuple(groups):
            v = [x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in select(lab)]
            lo, hi = ci(v)
            res[lab] = {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
                        'faster': sum(1 for t in v if t < 1)}
        res['per_stream'] = {x['stream']: x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in ok}
        out['pairs'][key][p] = res
out['vs_ffmpeg'] = {b: {p: statistics.median([x['ratio'][f'{b}:{p}'] for x in ok]) for p in ('discard', 'write')}
                    for b in builds}


def bars(new):
    """R13-1 to R13-4 for build `new` over base."""
    pr = out['pairs'][f'{new}/base']
    dis, wri = pr['discard'], pr['write']
    res = {
        'R13-1': dis['overall']['median'] <= 0.995 and dis['overall']['ci95'][1] < 1.0,
        'R13-2': wri['overall']['median'] <= 1.0 and wri['overall']['ci95'][1] < 1.005,
        'R13-3': all(dis[c]['median'] <= 1.005 for c in classes),
    }
    if 'g' in new:
        res['R13-4'] = threaded['median'] <= 1.01 if threaded else None
    return res


# R13-4: eight threads, `g` over `base`, discard, all streams.
threaded = None
if len(sys.argv) > 3:
    rows8 = []
    for c in classes:
        rows8 += json.load(open(f'{d}/{sys.argv[3]}.{c}.json'))['streams']
    v = [x['seconds']['g_discard'] / x['seconds']['base_discard'] for x in rows8
         if x.get('seconds') and 'g_discard' in x['seconds'] and 'base_discard' in x['seconds']]
    if v:
        lo, hi = ci(v)
        threaded = {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi]}
out['threads8_g_over_base_discard'] = threaded

# R13-5: a part stays where, on top of the other, the upper end of the interval is below 1.00 on
# both pairings: part G over all streams (it acts on every block), part R over the 720p, 1080p and
# 4k classes (the size rule turns it on there).
earns = {'G': all(out['pairs']['gr/r'][p]['overall']['ci95'][1] < 1.0 for p in ('discard', 'write')),
         'R': all(out['pairs']['gr/g'][p]['gated']['ci95'][1] < 1.0 for p in ('discard', 'write'))}
if earns['G'] and earns['R']:
    kept = 'gr'
elif earns['G'] or earns['R']:
    kept = 'g' if earns['G'] else 'r'
else:
    # Neither adds to the other: the round keeps the part that is faster alone over all streams,
    # on discard.
    kept = min(('g', 'r'), key=lambda b: out['pairs'][f'{b}/base']['discard']['overall']['median'])
out['R13-5'] = {'earns_its_place': earns, 'build_read': kept}
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
print('eight threads, g over base, discard:', threaded)
print('R13-5 earns its place:', earns, '-> bars read for', kept)
print('bars:', out['bars'])
print('per stream, kept over base (discard, write):')
pr = out['pairs'][f'{kept}/base']
for x in ok:
    s = x['stream']
    print(f"  {s} {x['class']:5} {pr['discard']['per_stream'][s]:.4f} {pr['write']['per_stream'][s]:.4f}")
