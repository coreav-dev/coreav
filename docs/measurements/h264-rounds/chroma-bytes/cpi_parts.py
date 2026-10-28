"""#486's cycle and instruction samples in CoreAV's chroma deblocking kernel, by part of the kernel.

Usage: python3 -I cpi_parts.py RES CORE_BIN > cpi-parts.txt

RES and CORE_BIN are those of ../kernel-gaps/kernels_pmu.py (#486's raw records and the rebuild of acf3ee2f with line
tables that reproduces its stages); that script's classification is run as it is, up to its family of each address.
A sample of the `deblock-chroma` family goes to the part named by the outermost two of its inline frames below
`filter_chroma_mb` among the kernel's functions (`vertical_edges`, `horizontal_edges`, `lanes`, `filter_pair`,
`filter_chroma16`, `store_rows`, `rows_at_mut`), or `top` where there is none. Weights are the samples' periods, all
streams and rounds together.
"""
import collections, json, os, subprocess, sys

res, core_bin = sys.argv[1], sys.argv[2]
here = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'kernel-gaps')
src = open(os.path.join(here, 'kernels_pmu.py')).read()
ns = {'__file__': os.path.join(here, 'kernels_pmu.py')}
sys.argv = ['kernels_pmu.py', res, core_bin]
exec(compile(src[:src.index('dims = {}')], 'kernels_pmu.py', 'exec'), ns)
fam_of, recs, dso, cd = ns['fam_of'], ns['recs'], ns['dso'], ns['cd']
w = {ev: collections.Counter() for ev in ('cycles', 'instructions')}
for p in recs:
    if os.path.basename(p).split('.')[2] != 'core':
        continue
    for ev, rows in json.load(open(p)).items():
        for d, a, n in rows:
            if d == dso and fam_of.get(a) == 'deblock-chroma':
                w[ev.split(':')[0]][a + cd] += n
addrs = sorted(set(w['cycles']) | set(w['instructions']))
txt = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', core_bin],
                     input='\n'.join(hex(a) for a in addrs) + '\n', capture_output=True, text=True).stdout
KNOWN = ('filter_chroma_mb', 'vertical_edges', 'horizontal_edges', 'lanes', 'filter_pair', 'filter_chroma16',
         'store_rows', 'rows_at_mut')
part = {}
for a, b in zip(addrs, txt.split('\n\n')):
    ls = [l for l in b.strip().split('\n') if l]
    names = [ls[i].split('::')[-1] for i in range(0, len(ls), 2)]
    path = [n for n in reversed(names) if n in KNOWN]
    part[a] = '/'.join(path[1:3]) or 'top'
cy, ins = collections.Counter(), collections.Counter()
for a in addrs:
    cy[part[a]] += w['cycles'][a]
    ins[part[a]] += w['instructions'][a]
tc, ti = sum(cy.values()), sum(ins.values())
print(f'deblock-chroma: cycles per instruction {tc / ti:.3f} over all of it')
print(f"{'cycles':>8} {'instr.':>8} {'CPI':>6}  part")
for k, v in cy.most_common():
    print(f'{100 * v / tc:7.1f}% {100 * ins[k] / ti:7.1f}% {v / max(ins[k], 1):6.2f}  {k}')
