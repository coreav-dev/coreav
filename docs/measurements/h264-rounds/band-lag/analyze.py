"""python3 -I analyze.py DIR -> the threading round's ratios between two builds timed in the same rounds.

DIR holds the reports of scripts/h264_parity.py with the decoders `base` (main, the commit the
branch is based on) and `a` (the branch tip): t.{sd,720p,1080p,4k}.json, timed with `--threads 1,8`
unpinned, and p.{sd,720p,1080p,4k}.json, timed with one thread pinned (`--pin 2`). For a over base,
each pairing (discard, write): the median per-stream ratio with a 95% bootstrap interval (2,000
resamples, random.Random(20261023)) and how many streams are faster, at 8 threads (the t run) and at
1 thread (the p run), overall and per class; then bars L1 to L4 (docs/design/h264.md, "Threading
round: a band published when its rows are filtered"). Information: each build's scaling against
FFmpeg's, (ours_8 / ffmpeg_8) / (ours_1 / ffmpeg_1) per stream, whose median per class is bar P3's
quantity (target 1.10); each build's ratio to FFmpeg at 8 threads and pinned at 1, with the streams
faster than FFmpeg; and the host's logical CPU count from the reports.
"""
import json, os, random, statistics, sys

d = sys.argv[1]
classes = ('sd', '720p', '1080p', '4k')
builds = ('base', 'a')
pairs = ('discard', 'write')


def load(pre):
    rows, hosts = [], set()
    for c in classes:
        r = json.load(open(f'{d}/{pre}.{c}.json'))
        hosts.add((r['machine']['cpu_model'], r['machine']['logical_cpus']))
        rows += r['streams']
    return rows, hosts


def stream_id(x):
    return os.path.basename(x['stream']).split('.')[0]


def ci(v, seed=20261023, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(v):
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
            'faster': sum(1 for t in v if t < 1)}


def groups(rows):
    out = {'overall': rows}
    out.update({c: [x for x in rows if x['class'] == c] for c in classes})
    return out


out = {}
for pre, tag in (('t', '@8'), ('p', '')):
    rows, hosts = load(pre)
    ok = [x for x in rows if all(f'{b}_{p}{tag}' in x.get('seconds', {}) for b in builds for p in pairs)]
    res = {'hosts': sorted(hosts), 'failed': [x['stream'] for x in rows if x not in ok], 'a/base': {},
           'vs_ffmpeg': {}}
    for p in pairs:
        key = f'{p}{tag}'
        res['a/base'][p] = {g: summary([x['seconds'][f'a_{key}'] / x['seconds'][f'base_{key}'] for x in sel])
                            for g, sel in groups(ok).items()}
        res['a/base'][p]['per_stream'] = {
            stream_id(x): x['seconds'][f'a_{key}'] / x['seconds'][f'base_{key}'] for x in ok}
    for b in builds:
        res['vs_ffmpeg'][b] = {}
        for p in pairs:
            res['vs_ffmpeg'][b][p] = {g: summary([x['ratio'][f'{b}:{p}{tag}'] for x in sel])
                                      for g, sel in groups(ok).items()}
    if pre == 't':
        res['scaling'] = {b: {p: {g: summary([x['ratio'][f'scaling:{b}:{p}@8'] for x in sel])
                                  for g, sel in groups(ok).items()} for p in pairs} for b in builds}
    out[pre] = res
t, p1 = out['t']['a/base'], out['p']['a/base']
out['bars'] = {
    'L1': t['discard']['sd']['median'] <= 0.85 and t['discard']['sd']['ci95'][1] < 1.0,
    'L2': t['write']['sd']['median'] <= 0.90,
    'L3': all(t[p][c]['median'] <= 1.005 for p in pairs for c in ('720p', '1080p', '4k')),
    'L4': all(p1[p]['overall']['median'] <= 1.005 for p in pairs)
          and all(p1[p][c]['median'] <= 1.01 for p in pairs for c in classes),
}
json.dump(out, open(f'{d}/summary.json', 'w'), indent=1)
for pre, name in (('t', '8 threads, unpinned'), ('p', '1 thread, pinned')):
    r = out[pre]
    print(f'== {name}: hosts {r["hosts"]}, failed {r["failed"]}')
    for p, res in r['a/base'].items():
        for g, v in res.items():
            if g != 'per_stream':
                print(f"a/base {p:7} {g:8} n={v['n']:2} median {v['median']:.4f} "
                      f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
    for b, res in r['vs_ffmpeg'].items():
        for p, gs in res.items():
            print(f"{b:4} vs FFmpeg {p:7} " + ' '.join(
                f"{g} {v['median']:.3f} ({v['faster']}/{v['n']} faster)" for g, v in gs.items()))
    for b, res in r.get('scaling', {}).items():
        for p, gs in res.items():
            print(f"{b:4} scaling against FFmpeg's (P3) {p:7} " + ' '.join(
                f"{g} {v['median']:.3f} [{v['ci95'][0]:.3f},{v['ci95'][1]:.3f}]" for g, v in gs.items()))
for k, v in out['bars'].items():
    print(k, 'passes' if v else 'FAILS')
