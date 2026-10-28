"""place.py BASE OTHER...: functions over 256 bytes in both binaries (nm, mangled names), how many keep their offset
mod 64 and mod 4096, and the text start of the h264 crate's functions."""
import subprocess, sys
def syms(b):
    d = {}
    for l in subprocess.run(['nm', '-S', b], capture_output=True, text=True).stdout.splitlines():
        p = l.split(' ', 3)
        if len(p) == 4 and p[2] in ('t', 'T'):
            try: d[p[3]] = (int(p[0], 16), int(p[1], 16))
            except ValueError: pass
    return d
a = syms(sys.argv[1])
for o in sys.argv[2:]:
    k = syms(o); both = [n for n in a if n in k and a[n][1] > 256]
    same = [n for n in both if a[n][1] == k[n][1]]
    print(f'{o}: {len(both)} functions >256 B in both ({len(same)} same size); same offset mod 64: '
          f'{sum(a[n][0] % 64 == k[n][0] % 64 for n in both)}, mod 4096: {sum(a[n][0] % 4096 == k[n][0] % 4096 for n in both)}, '
          f'aligned to 64 in OTHER: {sum(k[n][0] % 64 == 0 for n in both)}')
