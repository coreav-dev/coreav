"""python3 -I analyze.py DIR -> the PGO round's after/before per-stream ratios (`before` the plain dist
build, `after` the build of scripts/pgo_build.py, both of one commit): the fast mode at one thread
(`g`, pinned) and eight (`t8`), the safe mode at one thread (`gs`), and the held-out set at one thread
(`h`); discard and write, a bootstrap 95% interval of the median, overall and per class; the bars PG-1
to PG-6 of docs/design/pgo.md; and the ratios to FFmpeg with the share of streams faster than it.
PG-7 (output) is read from identity.txt, not here. A call that was not run is reported as missing,
and its bar as not met."""
import json, os, random, statistics, sys

d = sys.argv[1]
classes = ('sd', '720p', '1080p', '4k')


def load(prefix):
    rows = []
    for c in classes:
        p = f'{d}/{prefix}.{c}.json'
        if os.path.exists(p):
            rows += json.load(open(p))['streams']
    ok = [x for x in rows if x.get('seconds') and 'before_discard' in x['seconds']
          and 'after_discard' in x['seconds']]
    return rows, ok


def ci(v, seed=20261025, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(sel, p):
    sel = [x for x in sel if f'after_{p}' in x['seconds'] and f'before_{p}' in x['seconds']]
    if not sel:
        return {'n': 0, 'median': float('nan'), 'ci95': [float('nan')] * 2}
    v = [x['seconds'][f'after_{p}'] / x['seconds'][f'before_{p}'] for x in sel]
    rb = [x['ratio'][f'before:{p}'] for x in sel]
    ra = [x['ratio'][f'after:{p}'] for x in sel]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi],
            'faster_after': sum(1 for t in v if t < 1),
            'before_vs_ffmpeg': statistics.median(rb), 'after_vs_ffmpeg': statistics.median(ra),
            'faster_than_ffmpeg_before': sum(1 for t in rb if t < 1),
            'faster_than_ffmpeg_after': sum(1 for t in ra if t < 1)}


def table(ok):
    return {p: {lab: summary([x for x in ok if lab == 'overall' or x['class'] == lab], p)
                for lab in ('overall',) + classes} for p in ('discard', 'write')}


calls = {k: load(k) for k in ('g', 't8', 'gs', 'h')}
t = {k: table(ok) for k, (_, ok) in calls.items()}
g, t8, gs, h = t['g'], t['t8'], t['gs'], t['h']
# A bar is read only over every stream it names: a stream that failed in either arm fails it.
bars = {
    'PG-1': g['discard']['overall']['n'] == 32 and g['discard']['overall']['median'] <= 0.990
    and g['discard']['overall']['ci95'][1] < 1.000,
    'PG-2': g['write']['overall']['n'] == 32 and g['write']['overall']['median'] <= 0.995,
    'PG-3': all(g[p][k]['n'] == 8 and g[p][k]['median'] <= 1.005 for p in ('discard', 'write') for k in classes),
    'PG-4': t8['discard']['overall']['n'] == 32 and t8['discard']['overall']['median'] <= 1.000
    and all(t8['discard'][k]['n'] == 8 and t8['discard'][k]['median'] <= 1.010 for k in classes),
    'PG-5': gs['discard']['overall']['n'] == 32 and gs['discard']['overall']['median'] <= 0.995,
    'PG-6': h['discard']['overall']['n'] == 64 and h['discard']['overall']['median'] <= 0.995
    and h['discard']['overall']['ci95'][1] < 1.005,
}
out = {'failed': {k: [x['stream'] for x in rows if x not in ok] for k, (rows, ok) in calls.items()},
       'missing': [k for k, (rows, _) in calls.items() if not rows], 'calls': t, 'bars': bars}
json.dump(out, open(f'{d}/summary.json', 'w'), indent=1)
print('failed', out['failed'], 'missing', out['missing'])
for name, tt in t.items():
    for p in ('discard', 'write'):
        for k, v in tt[p].items():
            if not v['n']:
                continue
            print(f"{name:3} {p:7} {k:8} n={v['n']:2} after/before {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster_after']}/{v['n']} | vs ffmpeg "
                  f"before {v['before_vs_ffmpeg']:.3f} after {v['after_vs_ffmpeg']:.3f}, faster than ffmpeg "
                  f"{v['faster_than_ffmpeg_before']}/{v['n']} then {v['faster_than_ffmpeg_after']}/{v['n']}")
print('bars:', bars)
