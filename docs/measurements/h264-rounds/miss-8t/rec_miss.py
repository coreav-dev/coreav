"""rec_miss.py ES_DIR OUT ROUNDS CORE_MAIN CORE_FT FFMPEG_ATTR: the eight-thread miss probe's records (README.md), a
variant of ../profile-ae609547-pmu/rec_pmu.py.

Decoders: `main` (CoreAV `main`), `ft` (`main` with #508's code commit) and `ffa` (FFmpeg's attribution build), on the
32 training streams at the class frame counts, CoreAV `--discard` against `ffmpeg -f null`. Placements:
- `p1`: one thread (`--threads 1`, `-threads 1`) pinned to CPU 3;
- `p81`: eight threads (`--threads 8`, `-threads 8`), every thread pinned to CPU 3: the banded decode's work and
  footprint with every write and read on one core;
- `p8`: eight threads unpinned on the VM's eight cores, as the eight-thread speed bars run.
`main` and `ffa` run in every placement, `ft` in `p8` only.

1. Probe (OUT/events.json): each event of GROUPS alone on a short CoreAV run; one perf reports `<not supported>` or
   `<not counted>` for is left out of its group. Then each group counted together, which shows (`#running%`) whether
   any of its events was multiplexed.
2. perf stat of cycles, instructions and task-clock with the call's wall time, every decoder and placement, every
   round -> OUT/stat/<stream>.<class>.<dec>.<place>.<round>.json.
3. perf record of each group (2,000 samples a second an event, the events of a group in one call, at most four
   programmable events so that none is multiplexed) -> OUT/rec/<stream>.<class>.<dec>.<place>.<group>.<round>.json
   {event: [[dso, file address, weight]]}, weight the summed sample periods.
"""
import collections, json, os, re, subprocess, sys, tempfile, time

ES, out, rounds = os.path.join(sys.argv[1], ''), sys.argv[2], int(sys.argv[3])
BIN = {'main': sys.argv[4], 'ft': sys.argv[5], 'ffa': sys.argv[6]}
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
CPU = '3'
PLACES = {'p1': (1, True), 'p81': (8, True), 'p8': (8, False)}
RUNS = [('p1', 'main'), ('p1', 'ffa'), ('p81', 'main'), ('p81', 'ffa'), ('p8', 'main'), ('p8', 'ft'), ('p8', 'ffa')]
GROUPS = {
    'loads': ['cycles', 'instructions', 'mem_load_retired.l1_miss', 'mem_load_retired.l2_miss',
              'mem_load_retired.fb_hit', 'l1d_pend_miss.fb_full'],
    'stores': ['cycles', 'l2_rqsts.all_rfo', 'l2_rqsts.rfo_miss', 'resource_stalls.sb', 'mem_inst_retired.all_stores'],
    'stalls': ['cycles', 'cycle_activity.stalls_total', 'cycle_activity.stalls_l1d_miss',
               'cycle_activity.stalls_l2_miss', 'machine_clears.memory_ordering'],
}
os.makedirs(f'{out}/stat', exist_ok=True)
os.makedirs(f'{out}/rec', exist_ok=True)
streams = sorted(f for f in os.listdir(ES) if f.endswith('.h264'))


def argv(dec, place, f):
    n = str(FR[f.split('.')[2]])
    threads, pinned = PLACES[place]
    pin = ['taskset', '-c', CPU] if pinned else []
    if dec in ('main', 'ft'):
        return pin + [BIN[dec], '--frames', n] + (['--threads', str(threads)] if threads > 1 else []) + \
            ['--discard', ES + f]
    return pin + [BIN[dec], '-nostdin', '-v', 'error', '-threads', str(threads), '-i', ES + f, '-frames:v', n,
                  '-f', 'null', '-']


def stat(events, cmd):
    o = tempfile.mktemp()
    t0 = time.perf_counter()
    subprocess.run(['perf', 'stat', '-x', ',', '-o', o, '-e', ','.join(events + ['task-clock']), '--'] + cmd,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    res = {'wall_s': time.perf_counter() - t0}
    for line in open(o):
        p = line.strip().split(',')
        if len(p) < 3 or line.startswith('#'):
            continue
        val, ev = p[0], p[2]
        res[ev] = None if val.startswith('<') else float(val)
        if len(p) > 4 and p[4] and p[4] != '100.00':
            res[ev + '#running%'] = float(p[4])
    os.remove(o)
    return res


# 1. Probe.
probe = [BIN['main'], '--frames', '30', '--discard', ES + streams[0]]
ok = {}
for ev in sorted({e for g in GROUPS.values() for e in g}):
    try:
        ok[ev] = stat([ev], probe).get(ev)
    except subprocess.CalledProcessError:
        ok[ev] = None
groups = {g: [e for e in evs if ok.get(e)] for g, evs in GROUPS.items()}
# Each group counted together: an event perf runs for less than all of the time (`#running%`) was multiplexed.
together = {g: stat(evs, probe) for g, evs in groups.items() if evs}
json.dump({'counts_on_probe': ok, 'groups': groups, 'groups_together': together}, open(f'{out}/events.json', 'w'),
          indent=1)
print('groups:', groups, flush=True)

# 2. perf stat.
for r in range(rounds):
    for f in streams:
        s, c = f.split('.')[0], f.split('.')[2]
        for place, dec in RUNS:
            json.dump(stat(['cycles', 'instructions'], argv(dec, place, f)),
                      open(f'{out}/stat/{s}.{c}.{dec}.{place}.{r}.json', 'w'))
    print('stat round', r, 'done', flush=True)

# 3. perf record.
line_re = re.compile(r'^\s*(\d+)\s+(.+?):\s+([0-9a-f]+)\s+\((.*)\)\s*$')
mm = re.compile(r'PERF_RECORD_MMAP2? \S+: \[0x([0-9a-f]+)\(0x([0-9a-f]+)\) @ (0x)?([0-9a-f]+)[^\]]*\]: \S+ (\S+)')
names = {os.path.basename(b) for b in BIN.values()}
for r in range(rounds):
    for f in streams:
        s, c = f.split('.')[0], f.split('.')[2]
        for place, dec in RUNS:
            for g, evs in groups.items():
                if not evs:
                    continue
                data = tempfile.mktemp()
                subprocess.run(['perf', 'record', '-q', '-F', '2000', '-e', ','.join(evs), '-o', data, '--']
                               + argv(dec, place, f), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
                txt = subprocess.run(['perf', 'script', '-i', data, '--show-mmap-events', '-F', 'event,period,ip,dso'],
                                     capture_output=True, text=True).stdout
                os.remove(data)
                maps, cnt = [], collections.defaultdict(collections.Counter)
                for line in txt.splitlines():
                    if 'PERF_RECORD_MMAP' in line:
                        m = mm.search(line)
                        if m:
                            maps.append((int(m.group(1), 16), int(m.group(2), 16), int(m.group(4), 16), m.group(5)))
                        continue
                    m = line_re.match(line)
                    if not m:
                        continue
                    period, ev, ip, dso = int(m.group(1)), m.group(2).strip(), int(m.group(3), 16), m.group(4)
                    key = None
                    for st, ln, off, path in maps:
                        if st <= ip < st + ln and os.path.basename(path) in names:
                            key = (os.path.basename(path), ip - st + off)
                            break
                    if key is None:
                        key = (os.path.basename(dso), 0)
                    cnt[ev][key] += period
                json.dump({ev: [[d, a, n] for (d, a), n in cn.items()] for ev, cn in cnt.items()},
                          open(f'{out}/rec/{s}.{c}.{dec}.{place}.{g}.{r}.json', 'w'))
        print('record round', r, f, 'done', flush=True)
open(f'{out}/done', 'w').write('done\n')
