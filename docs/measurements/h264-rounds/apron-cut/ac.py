"""ac.py RUN_DIR: the apron cut's eight-thread ratios (docs/design/h264.md, "Threading round: the aprons' rows asked
for ahead"): k, kh, kc and kw over base, discard and write, and each over FFmpeg, median of the 32 per-stream ratios
with a bootstrap 95% interval (10,000 resamples, seed 1), per class, and the null arm's median (1 + the harness's
null deviation). Output: RUN_DIR/ac-summary.txt (printed)."""
import json, random, statistics as st, sys
d0 = sys.argv[1]
CL = ('sd', '720p', '1080p', '4k'); ARMS = tuple(sys.argv[2:]) or ("k", "kh", "kc", "kw")
rows = []
for c in CL:
    for s in json.load(open(f'{d0}/t8.{c}.json'))['streams']:
        sec, r = s['seconds'], s['ratio']
        row = dict(c=c, s=s['stream'], b_ff=r['base:discard'],
                   null_d=1 + s['null']['base_discard'], null_w=1 + s['null']['base_write'])
        for a in ARMS:
            row[a + '_d'] = sec[a + '_discard'] / sec['base_discard']
            row[a + '_w'] = sec[a + '_write'] / sec['base_write']
            row[a + '_ff'] = r[a + ':discard']
        rows.append(row)
def ci(x):
    g = random.Random(1); m = sorted(st.median(g.choices(x, k=len(x))) for _ in range(10000))
    return st.median(x), m[249], m[9749]
keys = [(a + '_d', f'discard {a}/base') for a in ARMS] + [(a + '_w', f'write {a}/base') for a in ARMS]
keys += [('null_d', 'discard null/base'), ('null_w', 'write null/base'), ('b_ff', 'base/FFmpeg discard')]
keys += [(a + '_ff', f'{a}/FFmpeg discard') for a in ARMS]
out = []
for k, n in keys:
    x = [r[k] for r in rows]
    m, lo, hi = ci(x)
    out.append(f'{n:22s} all {m:.4f} [{lo:.4f}, {hi:.4f}] ' + ' '.join(f'{c} {st.median(r[k] for r in rows if r["c"] == c):.4f}' for c in CL) + f'  below1 {sum(v < 1 for v in x)}/32')
out.append('per stream, discard over base: ' + ' '.join(ARMS))
out += [f'  {r["c"]:5s} {r["s"]:28s} ' + ' '.join(f'{r[a + "_d"]:.4f}' for a in ARMS) for r in rows]
open(f'{d0}/ac-summary.txt', 'w').write('\n'.join(out) + '\n')
print('\n'.join(out))
