# ab.py OUT ROUNDS NAME=BIN ...: interleaved CPU-time A/B over the 32 training streams, one thread, --discard,
# class frame counts, unpinned, eight threads. Per round: streams shuffled, arm order alternating. Writes JSON lines.
import os, sys, json, random, glob, subprocess, resource, time
out, rounds = sys.argv[1], int(sys.argv[2]); arms = [a.split('=', 1) for a in sys.argv[3:]]
ES = os.environ.get('ES', '$HOME/h264work/training'); FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
streams = sorted(glob.glob(ES + '/*.h264')); rng = random.Random(1)
def run(bin_, f):
    n = FR[os.path.basename(f).split('.')[2]]
    t0 = time.perf_counter()
    p = subprocess.Popen([bin_, '--threads', '8', '--discard', '--frames', str(n), f], stdout=subprocess.DEVNULL)
    _, st, ru = os.wait4(p.pid, 0)
    if st: raise SystemExit(f'{bin_} {f} status {st}')
    return ru.ru_utime + ru.ru_stime, time.perf_counter() - t0
with open(out, 'a') as o:
    for r in range(rounds):
        order = streams[:]; rng.shuffle(order)
        for i, f in enumerate(order):
            seq = arms if (i + r) % 2 == 0 else arms[::-1]
            for name, b in seq:
                c, w = run(b, f)
                o.write(json.dumps({'round': r, 'stream': os.path.basename(f), 'arm': name, 's': c, 'w': w}) + '\n'); o.flush()
