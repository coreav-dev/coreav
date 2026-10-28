"""hot.py ORDER BASE OTHER...: of the ordered (sampled) functions, how many keep their offset mod 64 and mod 4096."""
import subprocess, sys
def syms(b):
    d = {}
    for l in subprocess.run(['nm', b], capture_output=True, text=True).stdout.splitlines():
        p = l.split(' ', 2)
        if len(p) == 3 and p[1] in ('t', 'T'): d[p[2]] = int(p[0], 16)
    return d
hot = [l.strip() for l in open(sys.argv[1])]
a = syms(sys.argv[2])
for o in sys.argv[3:]:
    k = syms(o); h = [n for n in hot if n in a and n in k]
    print(f'{o}: {len(h)} sampled functions; same mod 64 {sum(a[n] % 64 == k[n] % 64 for n in h)}, mod 4096 {sum(a[n] % 4096 == k[n] % 4096 for n in h)}')
