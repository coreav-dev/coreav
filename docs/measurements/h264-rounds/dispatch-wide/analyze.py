"""python3 -I analyze.py DIR PREFIX1 PREFIXM PREFIX8 -> the bars WD-1 to WD-8 of docs/design/h264.md ("Kernel round:
the hot paths compiled twice, wider") from the A/B run's reports: `main` (main, baseline build), `v3` (main built for
x86-64-v3), `after` (this round, default features) and `safe` (this round without the feature `fast`), per-stream
ratios of one arm over another for the one-thread calls (PREFIX1, pinned; every arm), the mapped-allocator calls (PREFIXM;
main and after) and the eight-thread calls (PREFIX8, unpinned; main, v3 and after), discard and write, each median with
a bootstrap 95% interval, overall and per class; and every arm's ratio to FFmpeg with the share of streams faster than
it. WD-9 (output) is read from identity.txt and the conformance reports, not here."""
import json, random, statistics, sys

d, pre1, prem, pre8 = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
classes = ('sd', '720p', '1080p', '4k')


def load(prefix):
    rows = []
    for c in classes:
        rows += json.load(open(f'{d}/{prefix}.{c}.json'))['streams']
    return rows


def ci(v, seed=20261024, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def pair(rows, a, b, p):
    """Per stream a over b, pairing p, overall and per class."""
    ok = [x for x in rows if x.get('seconds') and f'{a}_{p}' in x['seconds'] and f'{b}_{p}' in x['seconds']]
    out = {}
    for lab in ('overall',) + classes:
        sel = [x for x in ok if lab == 'overall' or x['class'] == lab]
        v = [x['seconds'][f'{a}_{p}'] / x['seconds'][f'{b}_{p}'] for x in sel]
        lo, hi = ci(v)
        out[lab] = {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi], 'faster': sum(t < 1 for t in v)}
    out['per_stream'] = {x['stream']: x['seconds'][f'{a}_{p}'] / x['seconds'][f'{b}_{p}'] for x in ok}
    return out


def ffmpeg(rows, a, p):
    out = {}
    for lab in ('overall',) + classes:
        v = [x['ratio'][f'{a}:{p}'] for x in rows
             if f'{a}:{p}' in x.get('ratio', {}) and (lab == 'overall' or x['class'] == lab)]
        out[lab] = {'n': len(v), 'median': statistics.median(v) if v else float('nan'),
                    'faster_than_ffmpeg': sum(t < 1 for t in v)}
    return out


r1, rm, r8 = load(pre1), load(prem), load(pre8)
P = ('discard', 'write')
res = {
    'g': {f'{a}/{b}': {p: pair(r1, a, b, p) for p in P}
          for a, b in (('after', 'main'), ('after', 'v3'), ('v3', 'main'), ('after', 'safe'), ('safe', 'main'))},
    'gm': {'after/main': {p: pair(rm, 'after', 'main', p) for p in P}},
    't8': {f'{a}/{b}': {p: pair(r8, a, b, p) for p in P}
           for a, b in (('after', 'main'), ('after', 'v3'), ('v3', 'main'))},
    'ffmpeg': {'g': {a: {p: ffmpeg(r1, a, p) for p in P} for a in ('main', 'v3', 'after', 'safe')},
               't8': {a: {p: ffmpeg(r8, a, p) for p in P} for a in ('main', 'v3', 'after')}},
}
g, t8 = res['g'], res['t8']
med = lambda t, k='overall': t[k]['median']
res['bars'] = {
    'WD-1': med(g['after/main']['discard']) <= 0.980 and g['after/main']['discard']['overall']['ci95'][1] < 1.000,
    'WD-2': med(g['after/main']['write']) <= 0.985,
    'WD-3': all(med(g['after/main'][p], k) <= 1.000 for p in P for k in classes),
    'WD-4': med(t8['after/main']['discard']) <= 0.990
    and all(med(t8['after/main'][p], k) <= 1.005 for p in P for k in classes),
    'WD-5': med(g['after/v3']['discard']) <= 1.010 and all(med(g['after/v3']['discard'], k) <= 1.020 for k in classes)
    and med(t8['after/v3']['discard']) <= 1.010 and all(med(t8['after/v3']['discard'], k) <= 1.020 for k in classes),
    'WD-6': med(g['after/safe']['discard']) <= 0.980 and g['after/safe']['discard']['overall']['ci95'][1] < 1.000,
    'WD-7': med(g['safe/main']['discard']) <= 1.005,
    'WD-8': med(res['gm']['after/main']['discard']) <= 0.985
    and res['gm']['after/main']['discard']['overall']['ci95'][1] < 1.000,
}
json.dump(res, open(f'{d}/{pre1}.summary.json', 'w'), indent=1)
for call, t in (('1T', g), ('1T mapped', res['gm']), ('8T', t8)):
    for name, by in t.items():
        for p in P:
            for k in ('overall',) + classes:
                v = by[p][k]
                print(f"{call:9} {name:10} {p:7} {k:7} n={v['n']:2} {v['median']:.4f} "
                      f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
for call, t in res['ffmpeg'].items():
    for a, by in t.items():
        for p in P:
            print(f"vs FFmpeg 9.0.2 {call:2} {a:5} {p:7} " + ' '.join(
                f"{k} {v['median']:.3f} ({v['faster_than_ffmpeg']}/{v['n']} faster)" for k, v in by[p].items()))
print('bars:', res['bars'])
print('per stream (1T discard: after/main, after/v3, v3/main, after/safe; 8T discard after/main):')
for s_ in sorted(g['after/main']['discard']['per_stream']):
    f = lambda t, n: t[n]['discard']['per_stream'].get(s_, float('nan'))
    print(f"  {s_} {f(g, 'after/main'):.4f} {f(g, 'after/v3'):.4f} {f(g, 'v3/main'):.4f} "
          f"{f(g, 'after/safe'):.4f} {f(t8, 'after/main'):.4f}")
