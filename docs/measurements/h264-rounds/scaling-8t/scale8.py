"""scale8.py ES_DIR OUT US_TIMING US_PROF FFMPEG ROUNDS COUNT_ROUNDS REC_ROUNDS SEED (plan.md, steps 1 to 3).

1. OUT/timing.json {stream: {arm: [[wall, user, sys, minflt, nvcsw, nivcsw], ...]}}, warm-up round dropped,
   OUT/orders.json the order of every round.
2. OUT/stat/<stream>.<arm>.<round>.json {event: count} from perf stat.
3. OUT/rec/<stream>.<arm>.<round>.json {event: [[object, key, weight]]}: for our prof build key = file address;
   FFmpeg's own binary one key (0); other objects (libc, the kernel) by symbol.
"""
import collections, json, os, random, re, subprocess, sys, tempfile, time

ES, OUT, US, USP, FF = os.path.join(sys.argv[1], ''), sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5]
R, RC, RR, SEED = int(sys.argv[6]), int(sys.argv[7]), int(sys.argv[8]), int(sys.argv[9])
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
ARMS = ('us1', 'ff1', 'us8', 'ff8')
files = sorted(f for f in os.listdir(ES) if f.endswith('.h264'))
for d in ('stat', 'rec'):
    os.makedirs(f'{OUT}/{d}', exist_ok=True)


def cmd(f, arm, us=US):
    n, s, t = str(FR[f.split('.')[2]]), ES + f, int(arm[2])
    pin = ['taskset', '-c', '3'] if t == 1 else []
    if arm.startswith('us'):
        return pin + [us, '--frames', n] + (['--threads', '8'] if t == 8 else []) + ['--discard', s]
    return pin + [FF, '-nostdin', '-v', 'error', '-threads', str(t), '-i', s, '-frames:v', n,
                  '-fps_mode', 'passthrough', '-f', 'null', '-']


def run(argv):
    t0 = time.perf_counter()
    p = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    _, st, ru = os.wait4(p.pid, 0)
    w = time.perf_counter() - t0
    if st:
        raise SystemExit(f'decoder failed ({st}): {argv}')
    return [w, ru.ru_utime, ru.ru_stime, ru.ru_minflt, ru.ru_nvcsw, ru.ru_nivcsw]


# 1. Timing.
res = {f: {a: [] for a in ARMS} for f in files}
orders = []
for r in range(R + 1):
    g = random.Random(f'{SEED}:{r}')
    fs = files[:]
    g.shuffle(fs)
    order = []
    for f in fs:
        arms = list(ARMS)
        g.shuffle(arms)
        order.append([f, arms])
        for a in arms:
            m = run(cmd(f, a))
            if r:
                res[f][a].append(m)
    orders.append(order)
    json.dump(res, open(f'{OUT}/timing.json', 'w'))
    json.dump(orders, open(f'{OUT}/orders.json', 'w'))
    print('timing round', r, 'done', flush=True)

# 2. Counting.
EV = os.environ.get('STAT_EV', 'cycles:u,instructions:u,cycles:k,instructions:k,task-clock,context-switches,'
                    'cpu-migrations,page-faults').split(',')
for r in range(RC):
    for f in files:
        s = f.split('.')[0]
        for a in ARMS:
            o = tempfile.mktemp()
            subprocess.run(['perf', 'stat', '-x', ',', '-o', o, '-e', ','.join(EV), '--'] + cmd(f, a),
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
            out = {}
            for line in open(o):
                p = line.strip().split(',')
                if len(p) < 3 or line.startswith('#'):
                    continue
                out[p[2]] = None if p[0].startswith('<') else float(p[0])
                if len(p) > 4 and p[4] and p[4] != '100.00':
                    out[p[2] + '#running%'] = p[4]
            os.remove(o)
            json.dump(out, open(f'{OUT}/stat/{s}.{a}.{r}.json', 'w'))
    print('count round', r, 'done', flush=True)

# 3. Samples.
line_re = re.compile(r'^\s*(\d+)\s+(\S+?):\s+([0-9a-f]+)\s+(.*?)\s+\((.*)\)\s*$')
mm = re.compile(r'PERF_RECORD_MMAP2? \S+: \[0x([0-9a-f]+)\(0x([0-9a-f]+)\) @ (0x)?([0-9a-f]+)[^\]]*\]: \S+ (\S+)')
prof = os.path.basename(USP)
for r in range(RR):
    for f in files:
        s = f.split('.')[0]
        for a in ARMS:
            data = tempfile.mktemp()
            subprocess.run(['perf', 'record', '-q', '-F', '2000', '-e', os.environ.get('REC_EV', '{cycles,instructions}'), '-o', data, '--']
                           + cmd(f, a, USP), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
            txt = subprocess.run(['perf', 'script', '-i', data, '--show-mmap-events', '-F', 'event,period,ip,sym,dso'],
                                 capture_output=True, text=True).stdout
            os.remove(data)
            maps, cnt = [], collections.defaultdict(collections.Counter)
            for line in txt.splitlines():
                if 'PERF_RECORD_MMAP' in line:
                    m = mm.search(line)
                    if m and os.path.basename(m.group(5)) == prof:
                        maps.append((int(m.group(1), 16), int(m.group(2), 16), int(m.group(4), 16)))
                    continue
                m = line_re.match(line)
                if not m:
                    continue
                period, ev, ip, sym, dso = int(m.group(1)), m.group(2), int(m.group(3), 16), m.group(4), m.group(5)
                obj = os.path.basename(dso)
                if obj == prof:
                    key = next((ip - st + off for st, ln, off in maps if st <= ip < st + ln), -1)
                elif obj == os.path.basename(FF):
                    key = 0
                else:
                    key = re.sub(r'\+0x[0-9a-f]+$', '', sym)
                cnt[ev][(obj, key)] += period
            json.dump({ev: [[o, k, n] for (o, k), n in c.items()] for ev, c in cnt.items()},
                      open(f'{OUT}/rec/{s}.{a}.{r}.json', 'w'))
    print('record round', r, 'done', flush=True)
open(f'{OUT}/done', 'w').write('done\n')
