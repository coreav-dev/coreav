"""pf_stalls.py RES CORE_BIN DIMS: per class, the reference hints' events per macroblock, CoreAV's prefetch::for_rows
(split by caller: rows_for_read, the reference; rows_for_write, the output) against FFmpeg's ff_prefetch_* (function
level), from rec_pf.py's sampled groups. Each stream counts equally; rounds are averaged."""
import bisect, collections, glob, json, os, re, statistics, subprocess, sys

res, core_bin, dims_path = sys.argv[1], sys.argv[2], sys.argv[3]
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
dims = {}
for l in open(dims_path):
    f, d = l.split()
    w, h, n = (int(x) for x in d.split(','))
    dims[f.split('.')[0]] = ((w + 15) // 16) * ((h + 15) // 16) * min(n, FR[f.split('.')[2]])


def load_delta(lines):
    for l in lines:
        p = l.split()
        if p[:1] == ['LOAD'] and 'E' in p[6:]:
            return int(p[2], 16) - int(p[1], 16)
    return 0


syms = []
for l in open(f'{res}/ffattr.nm.txt'):
    p = l.split()
    if len(p) == 3 and p[1] in 'tTwW' and '.' not in p[2]:
        syms.append((int(p[0], 16), p[2]))
syms.sort()
sa = [a for a, _ in syms]
ff_delta = load_delta(open(f'{res}/ffattr.readelf-l.txt'))


def ff_func(a):
    i = bisect.bisect_right(sa, a + ff_delta) - 1
    return syms[i][1] if i >= 0 else '??'


recs = sorted(glob.glob(f'{res}/rec/*.json'))
core_dso = os.path.basename(core_bin)
addrs = sorted({a for p in recs if '.core.' in p for rows in json.load(open(p)).values() for d, a, n in rows
                if d == core_dso})
c_delta = load_delta(subprocess.run(['readelf', '-lW', core_bin], capture_output=True, text=True).stdout.splitlines())
txt = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', core_bin],
                     input='\n'.join(hex(a + c_delta) for a in addrs) + '\n', capture_output=True, text=True).stdout
item = {}
for a, b in zip(addrs, txt.split('\n\n')):
    ls = b.strip().split('\n')
    fr = list(zip(ls[0::2], ls[1::2]))
    if any(n.startswith('for_rows') and 'prefetch.rs' in f for n, f in fr):
        item[a] = 'read' if any(n.startswith('rows_for_read') for n, f in fr) else 'write'

# An event sampled in more than one group (cycles) is averaged over its groups and rounds, not summed.
nr = collections.Counter()
for p in recs:
    s, c, dec, g, r = os.path.basename(p)[:-5].split('.')
    for ev in json.load(open(p)):
        nr[(s, c, dec, ev.split(':')[0].split('/')[0])] += 1
acc = collections.defaultdict(float)      # (stream, class, part, event) -> count per MB
evs = set()
for p in recs:
    s, c, dec, g, r = os.path.basename(p)[:-5].split('.')
    for ev, rows in json.load(open(p)).items():
        ev = ev.split(':')[0].split('/')[0]
        k = 1 / nr[(s, c, dec, ev)] / dims[s]
        evs.add(ev)
        for d, a, n in rows:
            if dec == 'core':
                part = item.get(a) if d == core_dso else None
                acc[(s, c, 'core:all', ev)] += n * k
                if part:
                    acc[(s, c, 'core:' + part, ev)] += n * k
            else:
                acc[(s, c, 'ffa:all', ev)] += n * k
                if d == 'ffattr' and ff_func(a).startswith('ff_prefetch'):
                    acc[(s, c, 'ffa:pf', ev)] += n * k
streams = sorted({(k[0], k[1]) for k in acc})
parts = ('core:read', 'core:write', 'ffa:pf', 'core:all', 'ffa:all')
out = {}
for c in ('sd', '720p', '1080p', '4k'):
    ss = [x for x in streams if x[1] == c]
    if not ss:
        continue
    for part in parts:
        for ev in sorted(evs):
            v = statistics.mean(acc.get((s, c, part, ev), 0.0) for s, _ in ss)
            out[f'{c} {part} {ev}'] = v
json.dump(out, open(os.path.join(res, 'pf_stalls.json'), 'w'), indent=1)
ev_order = [e for e in ('cycles', 'instructions', 'cycle_activity.stalls_total', 'cycle_activity.stalls_l1d_miss',
                        'l1d_pend_miss.fb_full', 'sw_prefetch_access.t0', 'load_hit_prefetch.swpf',
                        'resource_stalls.scoreboard') if e in evs] + sorted(evs - {'cycles', 'instructions',
    'cycle_activity.stalls_total', 'cycle_activity.stalls_l1d_miss', 'l1d_pend_miss.fb_full', 'sw_prefetch_access.t0',
    'load_hit_prefetch.swpf', 'resource_stalls.scoreboard'})
print('per macroblock, mean over the class streams;', ', '.join(ev_order))
for c in ('sd', '720p', '1080p', '4k'):
    for part in parts:
        if f'{c} {part} cycles' in out or f'{c} {part} {ev_order[0]}' in out:
            print(f'{c:5} {part:10} ' + ' '.join(f'{out.get(f"{c} {part} {e}", 0):10.2f}' for e in ev_order))

# The rule fixed in README.md before the run: X, the issue part's excess, against T, 0.75% of CoreAV's cycles.
if 'cycle_activity.stalls_total' in evs:
    print()
    print('class  N_read N_write N_ff     X  S_read S_write S_ff     Y  B_read B_write B_ff     T  verdict')
    for c in ('720p', '1080p', '4k'):
        g = lambda part, ev: out.get(f'{c} {part} {ev}', 0.0)
        n = {p: g(p, 'cycles') - g(p, 'cycle_activity.stalls_total') for p in ('core:read', 'core:write', 'ffa:pf')}
        st = {p: g(p, 'cycle_activity.stalls_total') for p in n}
        b = {p: g(p, 'l1d_pend_miss.fb_full') for p in n}
        x = n['core:read'] + n['core:write'] - n['ffa:pf']
        y = st['core:read'] + st['core:write'] - st['ffa:pf']
        t = 0.0075 * g('core:all', 'cycles')
        print(f"{c:5} {n['core:read']:7.2f} {n['core:write']:7.2f} {n['ffa:pf']:5.2f} {x:5.2f} {st['core:read']:7.2f} "
              f"{st['core:write']:7.2f} {st['ffa:pf']:5.2f} {y:5.2f} {b['core:read']:7.2f} {b['core:write']:7.2f} "
              f"{b['ffa:pf']:5.2f} {t:5.2f}  {'probe' if x >= t else 'below'}")
