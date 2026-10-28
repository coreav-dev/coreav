"""Local, ungated: N binaries (first = base), default kernels, interleaved, pinned, discard."""
import subprocess, sys, time, statistics, json, random, os
rounds, out = int(sys.argv[1]), sys.argv[2]
bins = [a for a in sys.argv[3:] if not a.endswith('.h264')]
streams = [a for a in sys.argv[3:] if a.endswith('.h264')]
F = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
def run(binary, args):
    t = time.perf_counter()
    subprocess.run(['taskset', '-c', '2', binary, *args], stdout=subprocess.DEVNULL, check=True)
    return time.perf_counter() - t
res = {}
for s in streams:
    cls = s.split('.')[-2]; fr = ['--frames', str(F[cls]), '--discard', s]
    t = {b: [] for b in bins}
    for b in bins: run(b, fr)
    for r in range(rounds):
        order = bins[:] ; random.Random(r * 7 + len(res)).shuffle(order)
        for b in order: t[b].append(run(b, fr))
    name = os.path.basename(s)
    res[name] = {b: statistics.median(v) for b, v in t.items()}
    print(name, ' '.join(str(round(res[name][b] / res[name][bins[0]], 4)) for b in bins[1:]), flush=True)
summary = {}
for b in bins[1:]:
    ratios = [v[b] / v[bins[0]] for v in res.values()]
    rng = random.Random(1); boots = sorted(statistics.median(rng.choices(ratios, k=len(ratios))) for _ in range(2000))
    d = {'median': round(statistics.median(ratios), 4), 'ci': [round(boots[50], 4), round(boots[1949], 4)]}
    for c in F:
        rr = [v[b] / v[bins[0]] for k, v in res.items() if k.split('.')[-2] == c]
        if rr: d[c] = round(statistics.median(rr), 4)
    summary[os.path.basename(b)] = d
json.dump({'bins': bins, 'summary': summary, 'streams': res}, open(out, 'w'), indent=1)
print(json.dumps(summary))
