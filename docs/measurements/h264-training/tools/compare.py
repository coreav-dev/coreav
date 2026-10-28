"""The representativeness check and the comparison with the held-out draw.

    python3 -I docs/measurements/h264-training/tools/compare.py

Run from the repository root. Prints, for the training set's check run and
for the held-out draw's FFmpeg 7.0.2 arm: the median per-stream ratio with
its 95% bootstrap interval (as the held-out README), the ratio by bit rate
(bits per macroblock, terciles of both sets together) and by profile, and
each side's time over N1's FFmpeg estimate for the same frames
(`estimated_ffmpeg_seconds` of the lists), a yardstick per stream that
does not depend on the machine of the run.
"""
import json
import random
import statistics

H = 'docs/measurements/h264-n2-heldout'
T = 'docs/measurements/h264-training'
CLASSES = ['sd', '720p', '1080p', '4k']


def interval(r):
    rng = random.Random(20261021)
    m = sorted(statistics.median(rng.choices(r, k=len(r))) for _ in range(2000))
    return round(statistics.median(r), 3), [round(m[int(.025 * 2000)], 3), round(m[int(.975 * 2000) - 1], 3)]


def rows(listing, es_path, report, name):
    L = {f"{s['sha256'][:12]}.{s['track']}": s for s in json.load(open(listing))['streams']}
    es = json.load(open(es_path))
    out = []
    for k in CLASSES:
        for s in json.load(open(report.format(k)))['streams']:
            n = s['stream'].split('/')[-1]
            m = L['.'.join(n.split('.')[:2])]
            mbs = ((m['width'] + 15) // 16) * ((m['height'] + 15) // 16)
            ours = [v for kk, v in s['seconds'].items() if kk.endswith('_discard')][0]
            out.append({'set': name, 'class': k, 'profile': m['profile_idc'],
                        'discard': [v for kk, v in s['ratio'].items() if kk.endswith(':discard')][0],
                        'write': [v for kk, v in s['ratio'].items() if kk.endswith(':write')][0],
                        'bits_per_mb': es[n]['es_size'] * 8 / (m['frames_measured'] * mbs),
                        'ff_over_n1': s['seconds']['ff_null'] / m['estimated_ffmpeg_seconds'],
                        'ours_over_n1': ours / m['estimated_ffmpeg_seconds']})
    return out


def main():
    held = rows(f'{H}/list.json', f'{H}/es.json', H + '/reports/{}.ff7.json', 'held-out (ca5e4bc8)')
    train = rows(f'{T}/list.json', f'{T}/es.json', T + '/reports/check-dd8e81c.{}.json', 'training (dd8e81c)')
    cuts = sorted(x['bits_per_mb'] for x in held + train)
    c1, c2 = cuts[len(cuts) // 3], cuts[2 * len(cuts) // 3]
    for R in (held, train):
        name = R[0]['set']
        print(name, 'n', len(R))
        for pairing in ('discard', 'write'):
            print(f'  {pairing}: overall', interval([x[pairing] for x in R]),
                  {k: interval([x[pairing] for x in R if x['class'] == k])[0] for k in CLASSES})
        for lo, hi, lab in [(0, c1, 'low'), (c1, c2, 'mid'), (c2, 1e9, 'high')]:
            sub = [x['discard'] for x in R if lo <= x['bits_per_mb'] < hi]
            print(f'  bit rate {lab} ({len(sub)}): {statistics.median(sub):.3f}')
        for p in (66, 77, 100):
            sub = [x['discard'] for x in R if x['profile'] == p]
            print(f'  profile_idc {p} ({len(sub)}): {statistics.median(sub):.3f}')
        ff = sorted(x['ff_over_n1'] for x in R)
        print(f"  FFmpeg over N1 estimate, 10th to 90th percentile: "
              f"{ff[int(.1 * len(ff))]:.2f} to {ff[int(.9 * len(ff))]:.2f}")
        print(f"  median bits/MB {statistics.median(x['bits_per_mb'] for x in R):.1f}; "
              f"FFmpeg over N1 estimate {statistics.median(x['ff_over_n1'] for x in R):.3f}; "
              f"ours over N1 estimate {statistics.median(x['ours_over_n1'] for x in R):.3f}")
    print(f'bit-rate terciles at {c1:.1f} and {c2:.1f} bits/MB')
    calibration()


def calibration():
    """The machine calibration: the held-out binary on 16 held-out streams
    on this machine (`reports/calibration-ca5e4bc8.<class>.json`), per
    stream against #366's report for the same stream."""
    import os
    if not os.path.exists(f'{T}/reports/calibration-ca5e4bc8.sd.json'):
        return
    q = {'discard': [], 'write': [], 'ffmpeg': [], 'ours': []}
    for k in CLASSES:
        there = {s['stream'].split('/')[-1]: s
                 for s in json.load(open(f'{H}/reports/{k}.ff7.json'))['streams']}
        for s in json.load(open(f'{T}/reports/calibration-ca5e4bc8.{k}.json'))['streams']:
            o = there[s['stream'].split('/')[-1]]
            for pairing in ('discard', 'write'):
                here = [v for kk, v in s['ratio'].items() if kk.endswith(':' + pairing)][0]
                was = [v for kk, v in o['ratio'].items() if kk.endswith(':' + pairing)][0]
                q[pairing].append(here / was)
            q['ffmpeg'].append(s['seconds']['ff_null'] / o['seconds']['ff_null'])
            ours = [v for kk, v in s['seconds'].items() if kk.endswith('_discard')][0]
            was = [v for kk, v in o['seconds'].items() if kk.endswith('_discard')][0]
            q['ours'].append(ours / was)
    print('calibration, n', len(q['discard']))
    for name, r in q.items():
        print(f'  {name} here over #366:', interval(r))


if __name__ == '__main__':
    main()
