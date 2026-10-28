"""python3 -I analyze.py DIR -> the second MB layer layout round's after/before per-stream ratios (`before` is
main, `after` the round's code): the calls g (one thread, pinned), t8 (eight threads), gm (as g with every buffer
above 128 KB mapped) and h (as g, the 64 held-out streams), discard and write, each with a bootstrap 95% interval
of the median, over all streams and per class; the bars ML2-1 to ML2-6 of docs/design/h264.md ("MB layer round 2:
the layout without the added instructions"); and the ratios to FFmpeg with the share of streams faster than it.
ML2-7 (output) is read from identity.txt and the conformance reports, not here."""
import json, random, statistics, sys

d = sys.argv[1]
classes = ('sd', '720p', '1080p', '4k')
calls = {'g': 32, 't8': 32, 'gm': 32, 'h': 64}


def load(prefix):
    rows = []
    for c in classes:
        rows += json.load(open(f'{d}/{prefix}.{c}.json'))['streams']
    ok = [x for x in rows if x.get('seconds') and all(f'{a}_{p}' in x['seconds']
          for a in ('before', 'after') for p in ('discard', 'write'))]
    return rows, ok


def ci(v, seed=20261025, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(sel, p):
    v = [x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}'] for x in sel]
    rb = [x['ratio'][f'before:{p}'] for x in sel]
    ra = [x['ratio'][f'after:{p}'] for x in sel]
    if not v:
        return {'n': 0, 'median': float('nan'), 'ci95': [float('nan')] * 2, 'faster_after': 0,
                'before_vs_ffmpeg': float('nan'), 'after_vs_ffmpeg': float('nan'),
                'faster_than_ffmpeg_before': 0, 'faster_than_ffmpeg_after': 0}
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
            'faster_after': sum(1 for t in v if t < 1),
            'before_vs_ffmpeg': statistics.median(rb), 'after_vs_ffmpeg': statistics.median(ra),
            'faster_than_ffmpeg_before': sum(1 for t in rb if t < 1),
            'faster_than_ffmpeg_after': sum(1 for t in ra if t < 1)}


def table(ok):
    res = {}
    for p in ('discard', 'write'):
        r = {lab: summary([x for x in ok if lab == 'overall' or x['class'] == lab], p)
             for lab in ('overall',) + classes}
        r['per_stream'] = {x['stream']: x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}']
                           for x in ok}
        r['vs_ffmpeg'] = {x['stream']: (x['ratio'][f'before:{p}'], x['ratio'][f'after:{p}']) for x in ok}
        res[p] = r
    return res


loaded = {c: load(c) for c in calls}
t = {c: table(loaded[c][1]) for c in calls}
out = {'failed': {c: [x['stream'] for x in loaded[c][0] if x not in loaded[c][1]] for c in calls}, 'calls': t}


def at(c, p, k, most, n=None, upper=None):
    """Whether call c, pairing p, set k (all or a class) has every stream it names and a median at most
    `most` (and an upper interval end below `upper`)."""
    v = t[c][p][k]
    full = n if n is not None else (calls[c] if k == 'overall' else calls[c] // 4)
    return v['n'] == full and v['median'] <= most and (upper is None or v['ci95'][1] < upper)


# A bar is read only over every stream it names: a stream that failed in either arm fails it.
out['bars'] = {
    'ML2-1': at('g', 'discard', 'overall', 0.995, upper=1.000),
    'ML2-2': at('g', 'write', 'overall', 1.000),
    'ML2-3': all(at('g', p, k, 1.005) for p in ('discard', 'write') for k in classes),
    'ML2-4': at('t8', 'discard', 'overall', 1.000) and all(at('t8', 'discard', k, 1.010) for k in classes),
    'ML2-5': at('gm', 'discard', 'overall', 0.995) and at('h', 'discard', 'overall', 1.000),
    'ML2-6': at('g', 'discard', '4k', 0.980) and at('h', 'discard', '4k', 0.980),
}
json.dump(out, open(f'{d}/summary.json', 'w'), indent=1)
print('failed', out['failed'])
for c in calls:
    for p in ('discard', 'write'):
        for k, v in t[c][p].items():
            if k in ('per_stream', 'vs_ffmpeg'):
                continue
            print(f"{c:2} {p:7} {k:8} n={v['n']:2} after/before {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster_after']}/{v['n']} | vs ffmpeg "
                  f"before {v['before_vs_ffmpeg']:.3f} after {v['after_vs_ffmpeg']:.3f}, faster than ffmpeg "
                  f"{v['faster_than_ffmpeg_before']}/{v['n']} then {v['faster_than_ffmpeg_after']}/{v['n']}")
print('bars:', out['bars'])
for c in calls:
    print(f'per stream, {c} (discard, write; discard vs ffmpeg before -> after):')
    for s_ in sorted(t[c]['discard']['per_stream']):
        fb, fa = t[c]['discard']['vs_ffmpeg'][s_]
        print(f"  {s_} {t[c]['discard']['per_stream'][s_]:.4f} "
              f"{t[c]['write']['per_stream'].get(s_, float('nan')):.4f}  {fb:.3f} -> {fa:.3f}")
