"""Local, ungated: two binaries, default kernels, interleaved, pinned, discard."""
import subprocess, sys, time, statistics, json, random, os
a_bin, b_bin, rounds, out = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
streams = sys.argv[5:]
F = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
def run(binary, args):
    t = time.perf_counter()
    subprocess.run(['taskset', '-c', '2', binary, *args], stdout=subprocess.DEVNULL, check=True)
    return time.perf_counter() - t
res = {}
for s in streams:
    cls = s.split('.')[-2]; fr = ['--frames', str(F[cls]), '--discard', s]
    arms = {'a': a_bin, 'b': b_bin}
    t = {k: [] for k in arms}
    for k in arms: run(arms[k], fr)
    for r in range(rounds):
        order = ['a', 'b'] if r % 2 == 0 else ['b', 'a']
        for k in order: t[k].append(run(arms[k], fr))
    name = os.path.basename(s)
    res[name] = {k: statistics.median(v) for k, v in t.items()}
    print(name, round(res[name]['b'] / res[name]['a'], 4), flush=True)
ratios = [v['b'] / v['a'] for v in res.values()]
rng = random.Random(1); boots = sorted(statistics.median(rng.choices(ratios, k=len(ratios))) for _ in range(2000))
summary = {'median': statistics.median(ratios), 'ci': [boots[50], boots[1949]], 'n': len(ratios)}
for c in F:
    rr = [v['b'] / v['a'] for k, v in res.items() if k.split('.')[-2] == c]
    if rr: summary[c] = statistics.median(rr)
json.dump({'a': a_bin, 'b': b_bin, 'summary': summary, 'streams': res}, open(out, 'w'), indent=1)
print(json.dumps(summary))
