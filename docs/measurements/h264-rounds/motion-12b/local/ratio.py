import json,glob,sys,statistics as st
env=sys.argv[1]
allr={'discard':[],'write':[]}
for f in sorted(glob.glob(f'./{env}.*.json')):
    d=json.load(open(f))
    for s in d['streams']:
        sec=s['seconds']
        r={m: sec[f'm_{m}']/sec[f'base_{m}'] for m in ('discard','write')}
        for m in r: allr[m].append(r[m])
        print(f"{s['stream'][:12]} {s['class']:6} discard {r['discard']:.3f} write {r['write']:.3f}  m/ff {sec['m_discard']/sec['ff_null']:.3f}")
for m,v in allr.items(): print(env, m,'median',round(st.median(v),4),'n',len(v))
