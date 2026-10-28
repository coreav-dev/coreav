"""rec_pmu.py ES_DIR OUT CORE_BIN FFMPEG_BTBN FFMPEG_ATTR ROUNDS [ipc]: hardware-event counts and samples of CoreAV
`--discard` and `ffmpeg -threads 1 -f null` on the 32 training streams at the class frame counts, one thread pinned to
CPU 3 (the profile method of ../profile-6c9e240, with hardware events in place of cpu-clock).

1. Probes which events the host counts (OUT/events.json): each candidate alone on a short CoreAV run; an event the
   PMU does not expose is reported `<not supported>` or `<not counted>` by perf and is left out.
2. perf stat: every supported event, in passes of at most three programmable events beside cycles and instructions
   (so nothing is multiplexed on a core with four programmable counters a thread), for CoreAV, the BtbN build (the
   timing reference) and the attribution build -> OUT/stat/<stream>.<class>.<dec>.<round>.json {event: count}.
3. perf record: the sampled groups of SAMPLED (those supported), 2,000 samples a second an event, for CoreAV and the
   attribution build -> OUT/rec/<stream>.<class>.<dec>.<group>.<round>.json {event: [[dso, file address, weight]]},
   weight the summed sample periods, so a stage's weight estimates its event count.
With `ipc` (a second run): only cycles and instructions sampled together (group `ipc`), for each stage's instructions
per cycle, perf stat of cycles and instructions alone (step 2 with no other event) for the three builds, and a
probe of the top-down metrics (OUT/topdown-probe.txt); where perf reports them for a process, a
perf stat of TopdownL1 and TopdownL2 per stream for CoreAV and the BtbN build (OUT/topdown/).
"""
import collections, json, os, re, subprocess, sys, tempfile

ES, out, core, ffb, ffa = os.path.join(sys.argv[1], ''), sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
rounds = int(sys.argv[6])
IPC = len(sys.argv) > 7 and sys.argv[7] == 'ipc'
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
CPU = '3'
CANDIDATES = [
    'branch-misses', 'L1-dcache-loads', 'L1-dcache-load-misses',
    'l1d.replacement', 'l1d_pend_miss.fb_full', 'l1d_pend_miss.l2_stall',
    'l2_rqsts.miss', 'l2_rqsts.rfo_miss', 'l2_rqsts.all_rfo', 'l2_rqsts.demand_data_rd_miss',
    'mem_load_retired.l1_miss', 'mem_load_retired.l2_miss', 'mem_load_retired.l3_miss',
    'mem_inst_retired.all_loads', 'mem_inst_retired.all_stores',
    'ld_blocks.store_forward', 'resource_stalls.sb', 'resource_stalls.scoreboard',
    'exe_activity.bound_on_stores', 'exe_activity.bound_on_loads',
    'cycle_activity.stalls_total', 'cycle_activity.stalls_l1d_miss', 'cycle_activity.stalls_l2_miss',
    'cycle_activity.stalls_l3_miss', 'br_misp_retired.all_branches',
]
# Sampled groups: where the store traffic and the misses land, by stage.
SAMPLED = {
    'stores': ['cycles', 'exe_activity.bound_on_stores', 'resource_stalls.sb', 'ld_blocks.store_forward'],
    'misses': ['l1d.replacement', 'l2_rqsts.rfo_miss', 'mem_load_retired.l1_miss', 'branch-misses'],
}
os.makedirs(f'{out}/stat', exist_ok=True)
os.makedirs(f'{out}/rec', exist_ok=True)
streams = sorted(f for f in os.listdir(ES) if f.endswith('.h264'))


def argv(dec, f):
    n = str(FR[f.split('.')[2]])
    if dec == 'core':
        return [core, '--frames', n, '--discard', ES + f]
    return [ffb if dec == 'ffb' else ffa, '-nostdin', '-v', 'error', '-threads', '1', '-i', ES + f, '-frames:v', n,
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


def stat_all(found):
    """2. perf stat."""
    passes = [['cycles', 'instructions'] + found[i:i + 3] for i in range(0, len(found), 3)] or [['cycles', 'instructions']]
    for r in range(rounds):
        for f in streams:
            s, c = f.split('.')[0], f.split('.')[2]
            for dec in ('core', 'ffb', 'ffa'):
                res = {}
                for evs in passes:
                    for k, v in stat(evs, argv(dec, f)).items():
                        if k in ('cycles', 'instructions') and k in res:
                            res.setdefault(k + '#passes', []).append(v)
                            continue
                        res[k] = v
                json.dump(res, open(f'{out}/stat/{s}.{c}.{dec}.{r}.json', 'w'))
        print('stat round', r, 'done', flush=True)


def topdown():
    """The `ipc` run's probe of the top-down metrics, and their perf stat where a process gets them."""
    os.makedirs(f'{out}/topdown', exist_ok=True)
    cmd = [core, '--frames', '30', '--discard', ES + streams[0]]
    ok = []
    with open(f'{out}/topdown-probe.txt', 'w') as fh:
        for opt in (['--topdown'], ['-M', 'TopdownL1'], ['-M', 'TopdownL2'], ['-e', 'slots,topdown-retiring']):
            r = subprocess.run(['perf', 'stat', '-x', ','] + opt + ['--', 'taskset', '-c', CPU] + cmd,
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            fh.write(f'### perf stat {" ".join(opt)}: exit {r.returncode}\n{r.stderr}\n')
            if opt[0] == '-M' and r.returncode == 0 and '<not supported>' not in r.stderr:
                ok.append(opt[1])
    for f in streams if ok else []:
        s, c = f.split('.')[0], f.split('.')[2]
        for dec in ('core', 'ffb'):
            r = subprocess.run(['perf', 'stat', '-x', ',', '-M', ','.join(ok), '--', 'taskset', '-c', CPU] + argv(dec, f),
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
            open(f'{out}/topdown/{s}.{c}.{dec}.csv', 'w').write(r.stderr)
    print('topdown:', ok, flush=True)


if IPC:
    SAMPLED = {'ipc': ['cycles', 'instructions']}
    topdown()
    stat_all([])
    supported = ['instructions']
else:
    supported = probe_events()
    stat_all(supported)

# 3. perf record.
line_re = re.compile(r'^\s*(\d+)\s+(.+?):\s+([0-9a-f]+)\s+\((.*)\)\s*$')
mm = re.compile(r'PERF_RECORD_MMAP2? \S+: \[0x([0-9a-f]+)\(0x([0-9a-f]+)\) @ (0x)?([0-9a-f]+)[^\]]*\]: \S+ (\S+)')
names = (os.path.basename(core), os.path.basename(ffa))
for r in range(rounds):
    for f in streams:
        s, c = f.split('.')[0], f.split('.')[2]
        for dec in ('core', 'ffa'):
            for g, evs in SAMPLED.items():
                evs = [e for e in evs if e == 'cycles' or e in supported]
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
