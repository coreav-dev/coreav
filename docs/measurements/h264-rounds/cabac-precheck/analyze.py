"""analyze.py DIR: the CABAC pre-check's medians (information only, no bars).

Reads DIR/g.{sd,720p,1080p,4k}.json (one thread, pinned, five rounds; per
stream the median seconds of each build and of FFmpeg 9.0.2) and prints, per
build, the median over the eight streams of its ratio to `base`, per class,
`b1` against `e1`, and `base` against FFmpeg."""
import json, os, statistics, sys

d = sys.argv[1]
rows = []
for c in ('sd', '720p', '1080p', '4k'):
    for s in json.load(open(os.path.join(d, f'g.{c}.json')))['streams']:
        rows.append((c, s['stream'].split('.')[0], s['seconds']))
med = statistics.median
for b in ('bmi2all', 'e1', 'b1'):
    for m in ('discard', 'write'):
        r = [x[f'{b}_{m}'] / x[f'base_{m}'] for _, _, x in rows]
        cls = {c: med([x[f'{b}_{m}'] / x[f'base_{m}'] for k, _, x in rows if k == c]) for c in ('sd', '720p', '1080p', '4k')}
        print(f"{b:8s} {m:7s} over base: median {med(r):.4f}, faster {sum(v < 1 for v in r)}/{len(r)}; "
              + ', '.join(f'{c} {v:.4f}' for c, v in cls.items()))
for m in ('discard', 'write'):
    r = [x[f'b1_{m}'] / x[f'e1_{m}'] for _, _, x in rows]
    print(f"b1 over e1 {m}: median {med(r):.4f}; " + ', '.join(f'{s} {x[f"b1_{m}"] / x[f"e1_{m}"]:.4f}' for _, s, x in rows))
r = [x['base_discard'] / x['ff_null'] for _, _, x in rows]
print('base over FFmpeg 9.0.2, discard: median %.3f; ' % med(r)
      + ', '.join(f"{s} {x['base_discard']:.2f} s / {x['ff_null']:.2f} s" for _, s, x in rows))
