"""placement.py BASE K [AB_BEFORE AB_NOW]: functions of more than 256 bytes in both binaries (nm), and how many start at
the same offset within a 64-byte line; with two ab3.py outputs of the same base binary, the base's own time now over
before, per stream."""
import statistics, subprocess, sys
def syms(b):
    d = {}
    for l in subprocess.run(['nm', '-C', '-S', b], capture_output=True, text=True).stdout.splitlines():
        p = l.split(' ', 3)
        if len(p) == 4 and p[2] in ('t', 'T'):
            try: d[p[3]] = (int(p[0], 16), int(p[1], 16))
            except ValueError: pass
    return d
a, k = syms(sys.argv[1]), syms(sys.argv[2])
both = [n for n in a if n in k and a[n][1] > 256]
print(f'functions over 256 bytes in both: {len(both)}; same offset mod 64: {sum(a[n][0] % 64 == k[n][0] % 64 for n in both)}')
if len(sys.argv) > 4:
    rd = lambda p: {l.split()[0][:12]: float(l.split()[1]) for l in open(p) if not l.startswith('median')}
    x, y = rd(sys.argv[3]), rd(sys.argv[4]); r = [y[s] / x[s] for s in x]
    print(f'base time, this run over the earlier run, per stream: median {statistics.median(r):.4f}, {min(r):.4f} to {max(r):.4f}')
