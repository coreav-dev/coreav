"""ipc.py STREAM A B [N]: per function, perf cpu-clock samples (summed over reps, pd/perf.BIN.STREAM.*.data) and cachegrind
instructions (ir.BIN.STREAM.out) for builds A and B. Prints relative IPC (instruction share over time share) for A, and
B-A deltas of time (ms) and instructions (M), largest |time delta| first, plus totals."""
import sys, glob, subprocess, collections, re
sys.path.insert(0, '$HOME/h264work/r25/tools')
from cgcmp_lib import load
D = '$HOME/h264work/r25/'
s, A, B = sys.argv[1:4]; N = int(sys.argv[4]) if len(sys.argv) > 4 else 25
def norm(f):
    f = re.sub(r'::h[0-9a-f]{16}$', '', f.strip())
    return f
def samples(b):
    c = collections.Counter(); n = 0
    for p in sorted(glob.glob(f'{D}pd/perf.{b}.{s}.*.data')):
        n += 1
        out = subprocess.run(['perf', 'report', '-i', p, '--no-children', '--sort', 'sym', '-F', 'sample,sym', '--stdio', '-q',
                              '--no-inline'], capture_output=True, text=True).stdout
        for line in out.splitlines():
            m = re.match(r'\s*(\d+)\s+\[.\]\s+(.*)', line)
            if m: c[norm(m.group(2))] += int(m.group(1))
    return {k: v / n / 10.0 for k, v in c.items()}  # ms per run (10 kHz)
def instr(b):
    _, t = load(f'{D}ir.{b}.{s}.out'); c = collections.Counter()
    for f, v in t.items(): c[norm(f)] += v[0]
    return c
ta, tb, ia, ib = samples(A), samples(B), instr(A), instr(B)
TA, TB, IA, IB = sum(ta.values()), sum(tb.values()), sum(ia.values()), sum(ib.values())
print(f'{s}: time {TA:.1f} -> {TB:.1f} ms ({TB/TA:.4f}); instructions {IA/1e6:.1f} -> {IB/1e6:.1f} M ({IB/IA:.4f}); '
      f'mean instructions per ms {IA/TA/1e6:.3f} G/s')
keys = set(ta) | set(tb) | set(ia) | set(ib)
rows = sorted(keys, key=lambda k: -abs(tb.get(k, 0) - ta.get(k, 0)))
print(f"{'function':64} {'time A ms':>9} {'rel IPC':>7} {'d time ms':>9} {'d instr M':>9}")
for k in rows[:N]:
    t0 = ta.get(k, 0); i0 = ia.get(k, 0)
    rel = (i0 / IA) / (t0 / TA) if t0 > 0 else float('nan')
    print(f'{k[-64:]:64} {t0:9.2f} {rel:7.2f} {tb.get(k, 0) - t0:+9.2f} {(ib.get(k, 0) - i0) / 1e6:+9.2f}')
ch = [k for k in keys if ib.get(k, 0) != ia.get(k, 0)]
dch = sum(tb.get(k, 0) - ta.get(k, 0) for k in ch); dun = (TB - TA) - dch
tch = sum(ta.get(k, 0) for k in ch); ich = sum(ia.get(k, 0) for k in ch); dich = sum(ib.get(k, 0) - ia.get(k, 0) for k in ch)
print(f'SUMMARY {s}: changed fns time {tch:.1f} ms ({100*tch/TA:.1f}% of A), their instr {dich/ich*100:+.1f}%, their time {dch/tch*100:+.1f}% '
      f'({dch:+.2f} ms); unchanged fns {dun:+.2f} ms; total {100*(TB/TA-1):+.2f}% time vs {100*(IB/IA-1):+.2f}% instr')
