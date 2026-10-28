"""rec_pf.py ES_DIR OUT CORE_BIN FFMPEG_ATTR ROUNDS: where the reference hints' cycles go, stall against issue, for
CoreAV `--discard` and the attribution build of `ffmpeg -threads 1 -f null` on the streams of ES_DIR (the 720p, 1080p
and 4k training streams; the sd class issues no hints) at the class frame counts, one thread pinned to CPU 3. A copy of
../pmu-profile/rec_pmu.py with its probe (step 1) and its perf record (step 3), and two sampled groups of its own:
`stall` (cycles, instructions, all stall cycles, stall cycles with a level-1 miss pending, cycles a request waited for a
fill buffer) and `swpf` (cycles, software prefetches issued, demand loads that found a software prefetch still in
flight, cycles with loads outstanding and nothing executing). No perf stat pass: the run is read at function level
only. Writes OUT/events.json and OUT/rec/<stream>.<class>.<dec>.<group>.<round>.json {event: [[dso, address, weight]]}.
"""
import collections, json, os, re, subprocess, sys, tempfile

ES, out, core, ffa = os.path.join(sys.argv[1], ''), sys.argv[2], sys.argv[3], sys.argv[4]
rounds = int(sys.argv[5])
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
CPU = '3'
CANDIDATES = [
    'cycle_activity.stalls_total', 'cycle_activity.stalls_l1d_miss', 'l1d_pend_miss.fb_full',
    'sw_prefetch_access.t0', 'load_hit_prefetch.swpf', 'exe_activity.bound_on_loads',
]
# Sampled groups: at most three programmable events beside the fixed cycles and instructions, so none is multiplexed.
SAMPLED = {
    'stall': ['cycles', 'instructions', 'cycle_activity.stalls_total', 'cycle_activity.stalls_l1d_miss',
              'l1d_pend_miss.fb_full'],
    'swpf': ['cycles', 'sw_prefetch_access.t0', 'load_hit_prefetch.swpf', 'exe_activity.bound_on_loads'],
}
os.makedirs(f'{out}/rec', exist_ok=True)
streams = sorted(f for f in os.listdir(ES) if f.endswith('.h264'))


def argv(dec, f):
    n = str(FR[f.split('.')[2]])
    if dec == 'core':
        return [core, '--frames', n, '--discard', ES + f]
    return [ffa, '-nostdin', '-v', 'error', '-threads', '1', '-i', ES + f, '-frames:v', n,
            '-f', 'null', '-']


def stat(events, cmd):
    o = tempfile.mktemp()
    subprocess.run(['perf', 'stat', '-x', ',', '-o', o, '-e', ','.join(events), '--', 'taskset', '-c', CPU] + cmd,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
    res = {}
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


def probe_events():
    """1. Probe."""
    cmd = [core, '--frames', '30', '--discard', ES + streams[0]]
    ok = {}
    for ev in ['cycles', 'instructions'] + CANDIDATES:
        try:
            ok[ev] = stat([ev], cmd).get(ev)
        except subprocess.CalledProcessError:
            ok[ev] = None
    found = [e for e in CANDIDATES if ok.get(e)]
    json.dump({'counts_on_probe': ok, 'supported': found}, open(f'{out}/events.json', 'w'), indent=1)
    print('supported:', found, flush=True)
    return found


supported = probe_events()

# 3. perf record.
line_re = re.compile(r'^\s*(\d+)\s+(.+?):\s+([0-9a-f]+)\s+\((.*)\)\s*$')
mm = re.compile(r'PERF_RECORD_MMAP2? \S+: \[0x([0-9a-f]+)\(0x([0-9a-f]+)\) @ (0x)?([0-9a-f]+)[^\]]*\]: \S+ (\S+)')
names = (os.path.basename(core), os.path.basename(ffa))
for r in range(rounds):
    for f in streams:
        s, c = f.split('.')[0], f.split('.')[2]
        for dec in ('core', 'ffa'):
            for g, evs in SAMPLED.items():
                evs = [e for e in evs if e in ('cycles', 'instructions') or e in supported]
                if not evs:
                    continue
                data = tempfile.mktemp()
                subprocess.run(['perf', 'record', '-q', '-F', '2000', '-e', ','.join(evs), '-o', data, '--',
                                'taskset', '-c', CPU] + argv(dec, f),
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
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
                          open(f'{out}/rec/{s}.{c}.{dec}.{g}.{r}.json', 'w'))
    print('record round', r, 'done', flush=True)
open(f'{out}/done', 'w').write('done\n')
