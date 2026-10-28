"""fn_rank.py RES [N]: from stages_fn.py's RES/functions.json, per class and over the 32, each decoder's functions by
cycles as a share of that stream's FFmpeg (attribution build) cycles, mean over the class's streams, with their stage;
and the stages with entropy and the MB layer as one (the two decoders split them differently)."""
import collections, json, sys

res, n = sys.argv[1], int(sys.argv[2]) if len(sys.argv) > 2 else 25
d = json.load(open(f'{res}/functions.json'))
w = collections.defaultdict(lambda: collections.defaultdict(float))  # (stream, class, dec) -> (stage, fn) -> weight
for k, v in d['items'].items():
    s, c, dec, ev, stage = k.split('|')
    if ev == 'cycles':
        for fn, x in v.items():
            w[(s, c, dec)][(stage, fn)] += x
streams = sorted({(s, c) for s, c, _ in w})
GROUP = {'entropy': 'entropy+MB', 'mb': 'entropy+MB'}
for lab in ('sd', '720p', '1080p', '4k', 'all'):
    ss = [(s, c) for s, c in streams if lab in ('all', c)]
    acc = {dec: collections.defaultdict(float) for dec in ('core', 'ffa')}
    for s, c in ss:
        base = sum(w[(s, c, 'ffa')].values())
        for dec in acc:
            for key, x in w[(s, c, dec)].items():
                acc[dec][key] += x / base / len(ss)
    print(f'\n== {lab} (n={len(ss)}): stages, cycles as shares of FFmpeg\'s')
    for st in ('entropy+MB', 'inter', 'recon', 'deblock', 'other'):
        cv = {dec: sum(x for (g, _), x in acc[dec].items() if GROUP.get(g, g) == st) for dec in acc}
        print(f"  {st:11} CoreAV {cv['core']:.3f} FFmpeg {cv['ffa']:.3f} excess {cv['core'] - cv['ffa']:+.3f}")
    print(f"  {'total':11} CoreAV {sum(acc['core'].values()):.3f} FFmpeg {sum(acc['ffa'].values()):.3f}")
    rows = [sorted(acc[dec].items(), key=lambda y: -y[1])[:n] for dec in ('core', 'ffa')]
    for i in range(n):
        f = lambda r: f'{r[1]:.4f} {r[0][0][:7]:7} {r[0][1][:40]:40}' if r else ' ' * 53
        print('  ' + f(rows[0][i] if i < len(rows[0]) else None) + ' | ' + f(rows[1][i] if i < len(rows[1]) else None))
