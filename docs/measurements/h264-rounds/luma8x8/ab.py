# ab.py OUT ROUNDS NAME=BIN ...: interleaved CPU-time A/B over the 32 training streams, one thread, --discard,
# class frame counts, pinned to CPU 2. Per round: streams shuffled, arm order alternating. Writes JSON lines.
import os, sys, json, random, glob, subprocess, resource, time
out, rounds = sys.argv[1], int(sys.argv[2]); arms = [a.split('=', 1) for a in sys.argv[3:]]
ES = os.environ.get('ES', '$HOME/h264work/training'); FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
streams = sorted(glob.glob(ES + '/*.h264')); rng = random.Random(1)
def run(bin_, f):
    n = FR[os.path.basename(f).split('.')[2]]
    p = subprocess.Popen(['taskset', '-c', '2', bin_, '--discard', '--frames', str(n), f], stdout=subprocess.DEVNULL)
    _, st, ru = os.wait4(p.pid, 0)
    if st: raise SystemExit(f'{bin_} {f} status {st}')
    return ru.ru_utime + ru.ru_stime
with open(out, 'a') as o:
    for r in range(rounds):
        order = streams[:]; rng.shuffle(order)
        for i, f in enumerate(order):
            seq = arms if (i + r) % 2 == 0 else arms[::-1]
            for name, b in seq:
                o.write(json.dumps({'round': r, 'stream': os.path.basename(f), 'arm': name, 's': run(b, f)}) + '\n'); o.flush()
