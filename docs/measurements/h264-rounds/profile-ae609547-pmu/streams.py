"""streams.py RES [N]: each stream's time against FFmpeg from rec_pmu.py's perf stat (RES/stat/), and the N slowest
(10 by default) with their causes from stages_pmu.py's RES/stages.json.

- Time: `wall_s` of CoreAV `--discard` over the BtbN build's `-f null`, the mean of the rounds on each side; also the
  cycles (perf stat) and, for CoreAV and FFmpeg, task-clock over wall time (the cores kept busy). Per class and over
  the 32: the median per stream and the share of streams below 1.00. These are profiling runs on a PMU host, one call
  a round under perf, not the speed bars' timing (scripts/h264_parity*.py): diagnosis only.
- Causes: a stream's stages by excess over parity (CoreAV's cycles less FFmpeg's, as shares of that stream's FFmpeg
  cycles, the attribution build), with entropy and the MB layer as one stage as in top5.py; for its first two stages,
  the leading functions on each side."""
import collections, glob, json, os, statistics, sys

res = sys.argv[1]
N = int(sys.argv[2]) if len(sys.argv) > 2 else 10
CLASSES = ('sd', '720p', '1080p', '4k')
NAMES = {'em': 'entropy + MB', 'inter': 'inter', 'deblock': 'deblocking', 'recon': 'reconstruction',
         'other': 'other'}

tot = collections.defaultdict(lambda: collections.defaultdict(list))
for p in glob.glob(f'{res}/stat/*.json'):
    s, c, dec, r = os.path.basename(p)[:-5].split('.')
    for ev, v in json.load(open(p)).items():
        if '#' not in ev and v is not None:
            tot[(s, c, dec)][ev].append(v)
m = lambda k, ev: statistics.mean(tot[k][ev]) if tot[k][ev] else None
rows = {}
for s, c in sorted({(k[0], k[1]) for k in tot}):
    a, b = (s, c, 'core'), (s, c, 'ffb')
    if m(a, 'wall_s') and m(b, 'wall_s'):
        rows[(s, c)] = {'wall': m(a, 'wall_s') / m(b, 'wall_s'), 'cycles': m(a, 'cycles') / m(b, 'cycles'),
                        # task-clock is in milliseconds
                        'busy_core': m(a, 'task-clock') / 1000 / m(a, 'wall_s'),
                        'busy_ff': m(b, 'task-clock') / 1000 / m(b, 'wall_s')}

print('Time against FFmpeg (BtbN build), per stream, CoreAV over FFmpeg; busy = task-clock over wall time')
print(f"  {'class':6} {'n':>3} {'wall':>7} {'faster':>7} {'cycles':>7} {'busy C':>7} {'busy F':>7}")
summary = {}
for lab in CLASSES + ('all',):
    v = [r for (s, c), r in rows.items() if lab in ('all', c)]
    if not v:
        continue
    med = {k: statistics.median(x[k] for x in v) for k in ('wall', 'cycles', 'busy_core', 'busy_ff')}
    faster = sum(x['wall'] < 1 for x in v)
    summary[lab] = dict(med, n=len(v), faster=faster)
    print(f"  {lab:6} {len(v):3} {med['wall']:7.3f} {faster:3}/{len(v):<3} {med['cycles']:7.3f} "
          f"{med['busy_core']:7.2f} {med['busy_ff']:7.2f}")

st = json.load(open(f'{res}/stages.json')).get('streams', {})


def stages(key):
    """The stages of one stream with entropy and the MB layer as one, by excess; each with its leading functions."""
    d = st.get(key)
    if not d:
        return []
    out = []
    for x in ('em', 'inter', 'deblock', 'recon', 'other'):
        parts = ('entropy', 'mb') if x == 'em' else (x,)
        core = sum(d['core'][p] for p in parts)
        ff = sum(d['ffa'][p] for p in parts)
        items = {dec: sorted((i for p in parts for i in d['items'][p][dec]), key=lambda y: -y[1])[:3]
                 for dec in ('core', 'ffa')}
        out.append((x, core, ff, items))
    return sorted(out, key=lambda y: y[2] - y[1])


slow = sorted(rows.items(), key=lambda kv: -kv[1]['wall'])[:N]
print(f'\nThe {N} slowest streams against FFmpeg (wall time), each with its stages by excess over parity '
      '(shares of its FFmpeg cycles)')
out = {'summary': summary, 'streams': {f'{s}.{c}': r for (s, c), r in rows.items()}, 'slowest': []}
for (s, c), r in slow:
    sg = stages(f'{s}.{c}')
    print(f"\n{s}.{c}: wall {r['wall']:.3f}, cycles {r['cycles']:.3f}, busy {r['busy_core']:.2f} against "
          f"{r['busy_ff']:.2f}")
    for x, core, ff, _ in sg:
        print(f'  {NAMES[x]:15} CoreAV {core:6.3f} FFmpeg {ff:6.3f} excess {core - ff:+.3f}')
    for x, core, ff, items in sg[:2]:
        print(f'  in {NAMES[x]}: CoreAV ' + ', '.join(f'{n} {w:.3f}' for n, w in items['core'])
              + ' | FFmpeg ' + ', '.join(f'{n} {w:.3f}' for n, w in items['ffa']))
    out['slowest'].append({'stream': f'{s}.{c}', **r,
                           'stages': [{'stage': x, 'core': core, 'ffmpeg': ff, 'excess': core - ff, 'items': items}
                                      for x, core, ff, items in sg]})
json.dump(out, open(f'{res}/streams.json', 'w'), indent=1)
