import json, sys, collections
o = json.load(open('/tmp/mbc/ours.json'))
want = sys.argv[1]; n = int(sys.argv[2]) if len(sys.argv) > 2 else 25
rows = [(fl, v) for fn, fl, v in o['line'] if fn and want in fn]
tot = [sum(v[i] for _, v in rows) for i in range(5)]
print(f"{want}: Ir {tot[0]/1e6:.0f}M Bc {tot[1]/1e6:.0f}M Bcm {tot[2]/1e3:.0f}K")
# aggregate by file too
byf = collections.Counter()
for fl, v in rows: byf[fl.split(':')[0]] += v[0]
print("  by file:", ", ".join(f"{k} {v/1e6:.0f}" for k, v in byf.most_common(8)))
for fl, v in sorted(rows, key=lambda r: -r[1][0])[:n]:
    print(f"  {v[0]/1e6:7.1f} {v[1]/1e6:6.1f} {v[2]/1e3:7.0f}  {fl}")
