"""python3 -I analyze.py DIR PREFIX -> the CABAC registers round's ratios between four builds timed in the same rounds.

The reports are DIR/PREFIX.{sd,720p,1080p,4k}.json from scripts/h264_parity.py with the decoders
`base` (the commit the branch is based on), `c1` (the engine in three registers), `c1b` (+
renormalization from the rangeLPS table) and `c4` (+ coded_block_flag inline: the last code commit).
For each pair (c4, c1 and c1b over base, and each part over the build before it), each pairing
(discard, write), overall and per class: the median per-stream ratio with a 95% bootstrap interval
(2,000 resamples, random.Random(20261023)) and how many streams are faster. Then bars E-1 to E-3 of
docs/design/h264.md ("Entropy round: the CABAC engine in three registers") for c4 over base, each
build's median ratio to FFmpeg and the streams faster than it, overall and per class, as information,
and each stream's ratio. E-4 (output) is read from identity.txt and the conformance report, not here.
"""
import json, random, statistics, sys

d, pre = sys.argv[1], sys.argv[2]
classes = ('sd', '720p', '1080p', '4k')
rows = []
for c in classes:
    rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
builds = ('base', 'c1', 'c1b', 'c4')
ok = [x for x in rows if x.get('seconds')
      and all(f'{b}_{p}' in x['seconds'] for b in builds for p in ('discard', 'write'))]


def ci(v, seed=20261023, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(new, old, p, sel):
    v = [x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in sel]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
            'faster': sum(1 for t in v if t < 1)}


out = {'failed': [x['stream'] for x in rows if x not in ok], 'pairs': {}}
for new, old in (('c4', 'base'), ('c1', 'base'), ('c1b', 'base'), ('c1b', 'c1'), ('c4', 'c1b')):
    key = f'{new}/{old}'
    out['pairs'][key] = {}
    for p in ('discard', 'write'):
        res = {lab: summary(new, old, p, [x for x in ok if lab == 'overall' or x['class'] == lab])
               for lab in ('overall',) + classes}
        res['per_stream'] = {x['stream']: x['seconds'][f'{new}_{p}'] / x['seconds'][f'{old}_{p}'] for x in ok}
        out['pairs'][key][p] = res
dis, wri = out['pairs']['c4/base']['discard'], out['pairs']['c4/base']['write']
out['bars'] = {
    'E-1': dis['overall']['median'] <= 0.99 and dis['overall']['ci95'][1] < 1.0,
    'E-2': wri['overall']['median'] <= 1.0,
    'E-3': all(r[c]['median'] <= 1.005 for r in (dis, wri) for c in classes),
}
out['vs_ffmpeg'] = {}
for b in builds:
    out['vs_ffmpeg'][b] = {}
    for p in ('discard', 'write'):
        res = {}
        for lab in ('overall',) + classes:
            v = [x['ratio'][f'{b}:{p}'] for x in ok if lab == 'overall' or x['class'] == lab]
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
print('bars (c4 over base; E-4 is read from identity.txt and the conformance report):',
      ', '.join(f"{k} {'pass' if v else 'FAIL'}" for k, v in out['bars'].items()))
for b, pairs in out['vs_ffmpeg'].items():
    for p, res in pairs.items():
        print(f"{b:4} vs FFmpeg {p:7} " + ' '.join(
            f"{lab} {v['median']:.3f} ({v['faster']}/{v['n']} faster)" for lab, v in res.items()))
print('per stream, c4 over base (discard, write):')
for x in ok:
    s = x['stream']
    print(f"  {s} {x['class']:5} {dis['per_stream'][s]:.4f} {wri['per_stream'][s]:.4f}")
