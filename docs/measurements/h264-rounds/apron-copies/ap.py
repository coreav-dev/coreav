"""ap.py RUN_DIR: probe AP's ratios (plan-probe.md), twice and none over base and each over FFmpeg, eight threads,
median of the 32 per-stream ratios with a bootstrap 95% interval (10,000 resamples, seed 1), per class, and the
null arm's median (1 + the harness's null deviation). Output: RUN_DIR/ap-summary.txt (printed)."""
import json, random, statistics as st, sys
d0 = sys.argv[1]
CL = ('sd', '720p', '1080p', '4k')
rows = []
for c in CL:
    d = json.load(open(f'{d0}/t8.{c}.json'))
    for s in d['streams']:
        sec, r = s['seconds'], s['ratio']
        rows.append(dict(c=c, s=s['stream'],
                         tw_d=sec['twice_discard'] / sec['base_discard'], tw_w=sec['twice_write'] / sec['base_write'],
                         no_d=sec['none_discard'] / sec['base_discard'], no_w=sec['none_write'] / sec['base_write'],
                         b_ff=r['base:discard'], tw_ff=r['twice:discard'], no_ff=r['none:discard'],
                         null_d=1 + s['null']['base_discard'], null_w=1 + s['null']['base_write']))
def ci(x):
    g = random.Random(1); m = sorted(st.median(g.choices(x, k=len(x))) for _ in range(10000))
    return st.median(x), m[249], m[9749]
out = []
for k, n in (('no_d', 'discard none/base'), ('no_w', 'write none/base'), ('tw_d', 'discard twice/base'),
             ('tw_w', 'write twice/base'), ('null_d', 'discard null/base'), ('null_w', 'write null/base'),
             ('b_ff', 'base/FFmpeg discard'), ('no_ff', 'none/FFmpeg discard'), ('tw_ff', 'twice/FFmpeg discard')):
    x = [r[k] for r in rows]
    m, lo, hi = ci(x)
    out.append(f'{n:22s} all {m:.4f} [{lo:.4f}, {hi:.4f}] ' + ' '.join(f'{c} {st.median(r[k] for r in rows if r["c"] == c):.4f}' for c in CL) + f'  below1 {sum(v < 1 for v in x)}/32')
out.append('per stream, discard: none/base twice/base')
out += [f'  {r["c"]:5s} {r["s"]:28s} {r["no_d"]:.4f} {r["tw_d"]:.4f}' for r in rows]
open(f'{d0}/ap-summary.txt', 'w').write('\n'.join(out) + '\n')
print('\n'.join(out))
