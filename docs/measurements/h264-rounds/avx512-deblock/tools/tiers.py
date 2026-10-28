"""Local, ungated: one binary, --kernels avx2 vs auto, interleaved, pinned."""
import subprocess, sys, time, statistics, json, random, os
binary, rounds, out = sys.argv[1], int(sys.argv[2]), sys.argv[3]
streams = sys.argv[4:]
F = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
res = {}
def run(args):
    t = time.perf_counter()
    subprocess.run(['taskset', '-c', '2', binary, *args], stdout=subprocess.DEVNULL, check=True)
    return time.perf_counter() - t
for s in streams:
    cls = s.split('.')[-2]; fr = ['--frames', str(F[cls]), '--discard', s]
    arms = {'avx2': ['--kernels', 'avx2'], 'auto': []}
    t = {k: [] for k in arms}
    for k in arms: run(arms[k] + fr)
    for r in range(rounds):
        order = list(arms) if r % 2 == 0 else list(arms)[::-1]
        for k in order: t[k].append(run(arms[k] + fr))
    res[os.path.basename(s)] = {k: statistics.median(v) for k, v in t.items()}
    print(os.path.basename(s), round(res[os.path.basename(s)]['auto'] / res[os.path.basename(s)]['avx2'], 4), flush=True)
ratios = [v['auto'] / v['avx2'] for v in res.values()]
rng = random.Random(1); boots = sorted(statistics.median(rng.choices(ratios, k=len(ratios))) for _ in range(2000))
summary = {'median': statistics.median(ratios), 'ci': [boots[50], boots[1949]], 'n': len(ratios)}
for c in F:
    rr = [v['auto'] / v['avx2'] for k, v in res.items() if k.split('.')[-2] == c]
    if rr: summary[c] = statistics.median(rr)
json.dump({'summary': summary, 'streams': res}, open(out, 'w'), indent=1)
print(json.dumps(summary))
