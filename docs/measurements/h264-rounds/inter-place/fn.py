"""fn.py PKL STAGE: per class, the stage's functions on each side: cycles as a share of FFmpeg's (mean per stream),
instructions a macroblock (mean per stream), IPC (summed over the class's streams)."""
import pickle, sys, collections
d = pickle.load(open(sys.argv[1], 'rb')); stage = sys.argv[2]; N = int(sys.argv[3]) if len(sys.argv) > 3 else 14
pi, st, dims = d['per_item'], d['st'], d['dims']
streams = sorted({(k[0], k[1]) for k in st})
for lab in ('720p', '1080p', '4k', 'all'):
    ss = [(s, c) for s, c in streams if lab in ('all', c)]
    print(f'\n== {lab} {stage}')
    for dec in ('core', 'ffa'):
        cyc, ins, cs, isum = (collections.defaultdict(float) for _ in range(4))
        for s, c in ss:
            base = sum(st.get((s, c, 'ffa', 'cycles'), {}).values())
            for n, w in pi.get((s, c, dec, 'cycles', stage), {}).items():
                cyc[n] += w / base / len(ss); cs[n] += w
            for n, w in pi.get((s, c, dec, 'instructions', stage), {}).items():
                ins[n] += w / dims[s] / len(ss); isum[n] += w
        tot = sum(cyc.values())
        print(f'  {dec}: total {tot:.4f}, ins/MB {sum(ins.values()):.0f}, IPC {sum(isum.values()) / sum(cs.values()):.2f}')
        for n, v in sorted(cyc.items(), key=lambda x: -x[1])[:N]:
            print(f'    {v:.4f} {ins[n]:7.0f}/MB IPC {isum[n] / cs[n] if cs[n] else 0:5.2f}  {n[:70]}')
