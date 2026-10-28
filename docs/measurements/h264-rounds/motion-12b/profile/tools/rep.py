import json, collections, sys
o = json.load(open('/tmp/mbc/ours.json')); f = json.load(open('/tmp/mbc/ffc.json'))
M = 1e6
print("stage | Ir ours M | Ir ffc M | ratio | Bc ours | Bc ffc | Bcm ours K | Bcm ffc K")
for s in sorted(set(o['stage']) | set(f['stage']), key=lambda s: -o['stage'].get(s, [0])[0]):
    a = o['stage'].get(s, [0]*5); b = f['stage'].get(s, [0]*5)
    print(f"{s:24s} {a[0]/M:9.0f} {b[0]/M:9.0f} {a[0]/max(b[0],1):5.2f} {a[1]/M:7.0f} {b[1]/M:7.0f} {a[2]/1e3:8.0f} {b[2]/1e3:8.0f}")
sys.path.insert(0,'/tmp/mbc/tools'); from stages import stage
for side, d, nm in (("ours", o, 'ours'), ("ffc", f, 'ffmpeg')):
    print(f"\nmb control functions, {side}: Ir M, Bc M, Bcm K, Bi M, Bim K")
    rows = [(k, v) for k, v in d['fn'].items() if stage(k, nm) == 'mb control']
    for k, v in sorted(rows, key=lambda r: -r[1][0])[:25]:
        print(f"{v[0]/M:8.0f} {v[1]/M:6.0f} {v[2]/1e3:7.0f} {v[3]/M:5.1f} {v[4]/1e3:6.0f}  {k[:110]}")
