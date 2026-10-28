"""rss.py: maximum resident size, minor page faults, wall and CPU time of one decode each, at 1 and 8 threads, on
one sd and one 4k training stream; `m-13e4` is `base` and `pf2` the probe of `binaries.sha256`. Output: rss.txt."""
import os, subprocess, sys, time
B = os.path.expanduser('~/h264work/bins/'); ES = os.path.expanduser('~/h264work/train/es/')
for f, n in (('41e324ebb708.0.sd.h264', 1800), ('3b1b441e23ae.0.4k.h264', 150)):
    for t in (1, 8):
        for b in ('m-13e4', 'pf2'):
            t0 = time.time()
            p = subprocess.Popen([B + b, '--frames', str(n), '--threads', str(t), '--discard', ES + f], stdout=subprocess.DEVNULL)
            _, s, ru = os.wait4(p.pid, 0)
            print(f.split('.')[2], f't{t}', b, f'maxrss {ru.ru_maxrss/1024:.0f} MB  minflt {ru.ru_minflt}  wall {time.time()-t0:.2f}s  cpu {ru.ru_utime+ru.ru_stime:.2f}s')
