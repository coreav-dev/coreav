"""python3 -I analyze.py DIR PREFIX -> the luma kernel round's ratios between two builds timed in the same rounds.

The reports are DIR/PREFIX.{sd,720p,1080p,4k}.json from scripts/h264_parity.py with the decoders
`base` (`main`, the commit the branch is based on) and `k` (the branch's last code commit: the
luma 6-tap by byte pairs, the centre in 16 bits, full-sample weighting in words). For k over base,
each pairing (discard, write), overall, per class, over the 9 weighted streams and over the 23
others: the median per-stream ratio with a 95% bootstrap interval (2,000 resamples,
random.Random(20261024)) and how many streams are faster. Then bars LK-1 to LK-4 of
docs/design/h264.md ("Kernel round: luma interpolation by byte pairs"), each build's median
ratio to FFmpeg and the streams faster than it, overall and per class, as information, and each
stream's ratio. LK-5 (output) is read from identity.txt and the conformance report, not here.
"""
import json, random, statistics, sys

d, pre = sys.argv[1], sys.argv[2]
classes = ('sd', '720p', '1080p', '4k')
# Fixed with the bars, before the run: round 14's weighted streams (weighted predictions at least 10%
# of their luma blocks in docs/measurements/h264-rounds/luma-mc/calls.txt).
WEIGHTED = {'9db41aa16b5a', '41e324ebb708', 'b41f70a7fa60', '65ec61f95a00', '1d4b1775d78a',
            'dcb262cf0138', '8ae90cadb5bc', '7650ae466748', '278dbf5a63bb'}
groups = ('overall',) + classes + ('weighted', 'unweighted')


def member(x, lab):
    w = x['stream'].split('.')[0] in WEIGHTED
    return (lab == 'overall' or x['class'] == lab or (lab == 'weighted' and w)
            or (lab == 'unweighted' and not w))


rows = []
for c in classes:
    rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
builds = ('base', 'k')
ok = [x for x in rows if x.get('seconds')
      and all(f'{b}_{p}' in x['seconds'] for b in builds for p in ('discard', 'write'))]


def ci(v, seed=20261024, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(new, old, p, sel):
    v = [x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in sel]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
            'faster': sum(1 for t in v if t < 1)}


out = {'failed': [x['stream'] for x in rows if x not in ok], 'pairs': {}}
for new, old in (('k', 'base'),):
    key = f'{new}/{old}'
    out['pairs'][key] = {}
    for p in ('discard', 'write'):
        res = {lab: summary(new, old, p, [x for x in ok if member(x, lab)]) for lab in groups}
        res['per_stream'] = {x['stream']: x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in ok}
        out['pairs'][key][p] = res
dis, wri = out['pairs']['k/base']['discard'], out['pairs']['k/base']['write']
out['bars'] = {
    'LK-1': dis['overall']['median'] <= 0.997,
    'LK-2': wri['overall']['median'] <= 1.0,
    'LK-3': all(r[c]['median'] <= 1.005 for r in (dis, wri) for c in classes),
    'LK-4': dis['weighted']['median'] <= 0.997
    and all(r['unweighted']['median'] <= 1.005 for r in (dis, wri)),
}
out['vs_ffmpeg'] = {}
for b in builds:
    out['vs_ffmpeg'][b] = {}
    for p in ('discard', 'write'):
        res = {}
        for lab in groups:
            v = [x['ratio'][f'{b}:{p}'] for x in ok if member(x, lab)]
            res[lab] = {'n': len(v), 'median': statistics.median(v), 'faster': sum(1 for t in v if t < 1)}
        out['vs_ffmpeg'][b][p] = res
json.dump(out, open(f'{d}/{pre}.summary.json', 'w'), indent=1)
print('failed', out['failed'])
for key, pairs in out['pairs'].items():
    for p, res in pairs.items():
        for lab, v in res.items():
            if lab == 'per_stream':
                continue
            print(f"{key:9} {p:7} {lab:8} n={v['n']:2} median {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
print('bars (k over base; LK-5 is read from identity.txt and the conformance report):',
      ', '.join(f"{k} {'pass' if v else 'FAIL'}" for k, v in out['bars'].items()))
for b, pairs in out['vs_ffmpeg'].items():
    for p, res in pairs.items():
        print(f"{b:4} vs FFmpeg {p:7} " + ' '.join(
            f"{lab} {v['median']:.3f} ({v['faster']}/{v['n']} faster)" for lab, v in res.items()))
print('per stream, k over base (discard, write):')
for x in ok:
    s = x['stream']
    print(f"  {s} {x['class']:5} {dis['per_stream'][s]:.4f} {wri['per_stream'][s]:.4f}")
