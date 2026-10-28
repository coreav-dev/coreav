import glob, collections, json, sys, re
sys.path.insert(0,'/tmp/mbc/tools'); from stages import stage
o = json.load(open('/tmp/mbc/ours.json')); f = json.load(open('/tmp/mbc/ffc.json'))
def load(side):
    fn = collections.Counter()
    for p in glob.glob(f'/tmp/mbc/perf/*.{side}.*.txt'):
        for line in open(p):
            m = re.match(r'\s*[\d.]+%\s+(\d+)\s+\[\S\]\s+(.*?)\s+-\s+-\s*$', line)
            if m: fn[m.group(2).strip()] += int(m.group(1))
    return {k: v / 3 / 4999 * 1000 for k, v in fn.items()}  # ms per run
to, tf = load('ours'), load('ffc')
st = collections.defaultdict(lambda: [0, 0])
for k, v in to.items(): st[stage(k, 'ours')][0] += v
for k, v in tf.items(): st[stage(k, 'ffmpeg')][1] += v
print("stage: ms ours, ms ffc (2.50 GHz model 85, cpu-clock, mean of 3)")
for s, (a, b) in sorted(st.items(), key=lambda r: -r[1][0]): print(f"  {s:24s} {a:7.0f} {b:7.0f}")
print(f"  total {sum(v[0] for v in st.values()):.0f} {sum(v[1] for v in st.values()):.0f}")
print("\nmb control fns ours: ms, Ir M, ns per 1k Ir (=> cycles/insn at 2.5GHz: x2.5/1000)")
for side, t, d, nm in (('ours', to, o, 'ours'), ('ffc', tf, f, 'ffmpeg')):
    rows = [(k, v) for k, v in t.items() if stage(k, nm) == 'mb control']
    print(side)
    for k, v in sorted(rows, key=lambda r: -r[1])[:12]:
        ir = d['fn'].get(k, [0])[0]
        print(f"  {v:6.0f} ms {ir/1e6:6.0f}M  CPI {v*1e6*2.5/max(ir,1):4.2f}  {k[:90]}")
