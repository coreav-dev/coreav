"""d1w.py BIN STREAM LINE...: callgrind events of the named source lines (inline-folded, as addr.py): Ir, Dw, D1 read/write misses per 1000 Ir."""
import sys, collections
src = open('$HOME/h264work/r26/addr.py').read().split("if __name__")[0]
B, s, want = sys.argv[1], sys.argv[2], sys.argv[3:]
sys.argv = ['x', B, s]; g = {'__name__': 'x'}; exec(src, g)
C = g['cg_instr'](); F = g['frames'](set(C))
agg = collections.defaultdict(lambda: [0] * 13)
for a, r in C.items():
    k = g['key'](F.get(a, []))[0] if a >= 0 else 'outside'
    if k in want:
        for i, v in enumerate(r): agg[k][i] += v
for k, r in agg.items():
    kI = r[0] / 1000
    print(f'{s} {k}: Ir {r[0]} Dw {r[2]} D1mr/kI {r[4] / kI:.1f} D1mw/kI {r[5] / kI:.1f} DLmw/kI {r[8] / kI:.2f}')
