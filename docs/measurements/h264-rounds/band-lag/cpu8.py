"""python3 -I cpu8.py OUT.json FRAMES BASE A STREAM... -> CPU time (user + system) of two decoders at 8 threads.

Information beside bars L1 to L4: per stream, three interleaved runs of each decoder with
`--frames FRAMES --threads 8 --discard`, the median CPU seconds and wall seconds of each, and a's
CPU over base's.
"""
import json, os, statistics, subprocess, sys, time

out, frames, base, a, streams = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5:]


def run(binary, stream):
    start = time.perf_counter()
    p = subprocess.Popen([binary, '--frames', frames, '--threads', '8', '--discard', stream],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    _, status, usage = os.wait4(p.pid, 0)
    if status != 0:
        return None
    return usage.ru_utime + usage.ru_stime, time.perf_counter() - start


rows = []
for stream in streams:
    t = {'base': [], 'a': []}
    for _ in range(3):
        for name, binary in (('base', base), ('a', a)):
            r = run(binary, stream)
            if r is not None:
                t[name].append(r)
    if not t['base'] or not t['a']:
        continue
    cpu = {n: statistics.median(c for c, _ in v) for n, v in t.items()}
    wall = {n: statistics.median(w for _, w in v) for n, v in t.items()}
    rows.append({'stream': os.path.basename(stream), 'cpu': cpu, 'wall': wall,
                 'cpu_a/base': cpu['a'] / cpu['base']})
    print(f"{os.path.basename(stream)[:20]:20} cpu base {cpu['base']:.3f} a {cpu['a']:.3f} "
          f"ratio {cpu['a'] / cpu['base']:.3f}", flush=True)
print('median a/base CPU', round(statistics.median(r['cpu_a/base'] for r in rows), 4))
json.dump(rows, open(out, 'w'), indent=1)
