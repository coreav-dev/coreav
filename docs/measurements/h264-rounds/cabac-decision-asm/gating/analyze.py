"""python3 -I analyze.py DIR ROUND -> a round's ratios between two builds (`base` and `k`) timed in the same rounds.

DIR holds, per class (sd, 720p, 1080p, 4k), reports of scripts/h264_parity.py with the decoders `base` and `k` (and, where
a call has them, `basenofast` and `knofast`, the two builds without the feature `fast`): g.<class>.json (32 training streams,
one thread pinned), t8.<class>.json (32 training, eight threads: Compute Engine), h.<class>.json (64 held-out, one thread
pinned), h8.<class>.json (64 held-out, eight threads: Compute Engine). Each call present: k over base, discard and write,
overall and per class: the median per-stream ratio with a 95% bootstrap interval (2,000 resamples, random.Random(20261024))
and how many streams are faster; each build's median ratio to FFmpeg and the streams faster than it; knofast over basenofast
(information); the harness's host-fault records (null-arm deviation, retried and unresolved streams, the unresolved listed,
in-call gates). ROUND names the bars evaluated: ip (docs/design/h264.md, "Inter round: the placement checked once a
band"), ad ("Entropy round: DecodeDecision in assembly"). Output (identity, conformance) bars are read from identity.txt and
the conformance report, not here."""
import json, os, random, statistics, sys

d, rnd = sys.argv[1], sys.argv[2]
classes = ('sd', '720p', '1080p', '4k')
groups = ('overall',) + classes
CALLS = (('g', '1 thread, 32 training'), ('t8', '8 threads, 32 training'), ('h', '1 thread, 64 held-out'),
         ('h8', '8 threads, 64 held-out'))
method = {}
unresolved_list = {}


def member(x, lab):
    return lab == 'overall' or x['class'] == lab


def load(pre):
    rows = []
    for c in classes:
        p = f'{d}/{pre}.{c}.json'
        if not os.path.exists(p):
            return None
        rep = json.load(open(p))
        rows += rep['streams']
        m = rep.get('method', {})
        method[f'{pre}.{c}'] = {k: m.get(k) for k in ('unresolved', 'retried', 'gate_drift_streams')}
        method[f'{pre}.{c}']['null_max'] = max((abs(v) for x in rep['streams'] for v in x.get('null', {}).values()),
                                               default=None)
        unresolved_list[f'{pre}.{c}'] = [x['stream'] for x in rep['streams'] if x.get('unresolved')]
    return rows


def ci(v, seed=20261024, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n))
    return m[int(.025 * n)], m[int(.975 * n) - 1]


def summary(a, b, p, sel):
    v = [x['seconds'][f'{a}_{p}'] / x['seconds'][f'{b}_{p}'] for x in sel]
    lo, hi = ci(v)
    return {'n': len(v), 'median': statistics.median(v), 'ci95': [lo, hi], 'faster': sum(1 for t in v if t < 1)}


out = {}
for pre, label in CALLS:
    rows = load(pre)
    if rows is None:
        continue
    builds = [b for b in ('base', 'k', 'basenofast', 'knofast')
              if rows and all(f'{b}_discard' in x.get('seconds', {}) for x in rows if x.get('seconds'))]
    ok = [x for x in rows if x.get('seconds') and all(f'{b}_{p}' in x['seconds'] for b in ('base', 'k') for p in ('discard', 'write'))]
    res = {'label': label, 'failed': [x['stream'] for x in rows if x not in ok]}
    for p in ('discard', 'write'):
        res[p] = {lab: summary('k', 'base', p, [x for x in ok if member(x, lab)]) for lab in groups}
    res['vs_ffmpeg'] = {}
    for b in builds:
        res['vs_ffmpeg'][b] = {}
        for p in ('discard', 'write'):
            r = {}
            for lab in groups:
                v = [x['ratio'][f'{b}:{p}'] for x in ok if member(x, lab)]
                r[lab] = {'n': len(v), 'median': statistics.median(v), 'faster': sum(1 for t in v if t < 1)}
            res['vs_ffmpeg'][b][p] = r
    if 'knofast' in builds and 'basenofast' in builds:
        res['knofast_over_basenofast'] = {p: {lab: summary('knofast', 'basenofast', p, [x for x in ok if member(x, lab)])
                                              for lab in groups} for p in ('discard', 'write')}
    out[pre] = res


def med(pre, p, lab='overall'):
    return out[pre][p][lab]['median']


def per_class_le(pre, ps, bar):
    return all(med(pre, p, c) <= bar for p in ps for c in classes)


bars = {}
have = lambda *c: all(x in out for x in c)
if rnd == 'ip':
    if have('g'):
        bars['IP-1 (g discard all-32 <= 0.995, upper < 1.000)'] = med('g', 'discard') <= 0.995 and out['g']['discard']['overall']['ci95'][1] < 1.0
        bars['IP-3 1T (g write all-32 <= 1.000)'] = med('g', 'write') <= 1.0
        bars['IP-4 1T (g each class discard+write <= 1.005)'] = per_class_le('g', ('discard', 'write'), 1.005)
    if have('t8'):
        bars['IP-2 (t8 discard all-32 <= 0.995, upper < 1.000)'] = med('t8', 'discard') <= 0.995 and out['t8']['discard']['overall']['ci95'][1] < 1.0
        bars['IP-3 8T (t8 write all-32 <= 1.000)'] = med('t8', 'write') <= 1.0
        bars['IP-4 8T (t8 each class discard+write <= 1.005)'] = per_class_le('t8', ('discard', 'write'), 1.005)
    if have('h8'):
        bars['IP-6 (h8 discard all-64 <= 1.000)'] = out['h8']['discard']['overall']['n'] == 64 and med('h8', 'discard') <= 1.0
elif rnd == 'ad':
    if have('g'):
        bars['AD-1 (g discard all-32 <= 0.980, upper < 1.000)'] = med('g', 'discard') <= 0.980 and out['g']['discard']['overall']['ci95'][1] < 1.0
        bars['AD-2 (g write all-32 <= 0.995)'] = med('g', 'write') <= 0.995
        bars['AD-3 (g each class discard+write <= 1.005)'] = per_class_le('g', ('discard', 'write'), 1.005)
    if have('t8'):
        bars['AD-4 (t8 discard all-32 <= 1.000; each class <= 1.010)'] = med('t8', 'discard') <= 1.0 and all(med('t8', 'discard', c) <= 1.010 for c in classes)
    if have('h'):
        bars['AD-5 (h discard all-64 <= 0.990)'] = out['h']['discard']['overall']['n'] == 64 and med('h', 'discard') <= 0.990
summ = {'calls': out, 'bars': bars, 'method': method, 'unresolved': unresolved_list}
json.dump(summ, open(f'{d}/summary.json', 'w'), indent=1)
for pre, res in out.items():
    print(res['label'], 'failed', res['failed'])
    for p in ('discard', 'write'):
        for lab in groups:
            v = res[p][lab]
            print(f"  k/base {p:7} {lab:8} n={v['n']:2} median {v['median']:.4f} "
                  f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
    for b, pairs in res['vs_ffmpeg'].items():
        for p, r in pairs.items():
            print(f"  {b:10} vs FFmpeg {p:7} " + ' '.join(
                f"{lab} {v['median']:.3f} ({v['faster']}/{v['n']} faster)" for lab, v in r.items()))
    if 'knofast_over_basenofast' in res:
        for p, r in res['knofast_over_basenofast'].items():
            print(f"  knofast/basenofast {p:7} " + ' '.join(f"{lab} {v['median']:.4f} [{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}]"
                                                           for lab, v in r.items()))
for k, v in method.items():
    print(f"  {k}: unresolved {v['unresolved']} {unresolved_list[k]}, retried {v['retried']}, gate drift {v['gate_drift_streams']}, "
          f"largest null deviation {v['null_max']}")
print('bars:', ', '.join(f"{k}: {'pass' if v else 'FAIL'}" for k, v in bars.items()))
