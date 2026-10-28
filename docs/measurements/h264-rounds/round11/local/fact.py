"""fact.py DIR PREFIX a/b [a/b ...]: median per-stream ratio of build a over build b, with a 95% bootstrap interval."""
import json, random, statistics, sys
d, pre, pairs = sys.argv[1], sys.argv[2], sys.argv[3:]
classes = ('sd', '720p', '1080p', '4k'); rows = []
for c in classes:
    try: rows += json.load(open(f'{d}/{pre}.{c}.json'))['streams']
    except FileNotFoundError: pass
def ci(v, seed=20261022, n=2000):
    rng = random.Random(seed)
    m = sorted(statistics.median(rng.choices(v, k=len(v))) for _ in range(n)); return m[int(.025*n)], m[int(.975*n)-1]
for pair in pairs:
    a, b = pair.split('/')
    for p in ('discard', 'write'):
        ok = [x for x in rows if x.get('seconds') and f'{a}_{p}' in x['seconds'] and f'{b}_{p}' in x['seconds']]
        for lab in ('overall',) + classes:
            sel = [x for x in ok if lab == 'overall' or x['class'] == lab]
            if not sel: continue
            v = [x['seconds'][f'{a}_{p}'] / x['seconds'][f'{b}_{p}'] for x in sel]
            lo, hi = ci(v)
            print(f"{pair:8} {p:7} {lab:8} n={len(v):2} {statistics.median(v):.4f} [{lo:.4f},{hi:.4f}] faster {sum(t < 1 for t in v)}/{len(v)}")
        if p == 'discard':
            print('   worst:', ' '.join(f"{x['stream'][:6]}:{x['seconds'][f'{a}_{p}']/x['seconds'][f'{b}_{p}']:.3f}" for x in sorted(ok, key=lambda x: -x['seconds'][f'{a}_{p}']/x['seconds'][f'{b}_{p}'])[:4]))
    if 'ffmpeg' not in pair:
        for p in ('discard','write'):
            for n in (a, b):
                r = [x['ratio'][f'{n}:{p}'] for x in rows if x.get('ratio') and f'{n}:{p}' in x['ratio']]
                if r: print(f"   {n} vs ffmpeg {p}: {statistics.median(r):.3f}")
