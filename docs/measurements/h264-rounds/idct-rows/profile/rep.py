import sys, os, re, json, bisect, collections
sys.path.insert(0, "/tmp/prof/src/docs/measurements/h264-gap-2/tools")
import stages, cg
G="/tmp/prof/gap"; FF="$HOME/ffbuild/ffmpeg-9.0.2/ffmpeg_g"
PERF_LINE = re.compile(r"^\s+([\d.]+)%\s+(\d+)\s+\[.\]\s+(.*?)(?:\s+-\s+-)?\s*$")
streams=[l.split() for l in open(G+"/streams.txt") if l.strip()]
addrs,names=cg.symbols(FF)
def by_address(a):
    k=bisect.bisect_right(addrs,a)-1
    return names[k] if 0<=k and a<addrs[-1]+0x100000 else f"{a:#x} (outside ffmpeg)"
tot={s:collections.Counter() for s in ("ours","ffsimd")}
fn={s:collections.Counter() for s in ("ours","ffsimd")}
per={}
for sid,cls,F in streams:
    per[sid]={}
    for side in ("ours","ffsimd"):
        c=collections.Counter()
        if side=="ffsimd":
            for line in open(f"{G}/perf/{sid}.{side}.ips"):
                n=by_address(int(line.split()[0],16)); c[stages.stage(n,"ffmpeg")]+=1; fn[side][n]+=1
        else:
            for line in open(f"{G}/perf/{sid}.{side}.txt"):
                m=PERF_LINE.match(line)
                if m:
                    n=m.group(3).strip(); k=int(m.group(2)); c[stages.stage(n,"ours")]+=k; fn[side][n]+=k
        per[sid][side]=dict(c); tot[side].update(c)
old=json.load(open("/tmp/prof/src/docs/measurements/h264-gap-2/evidence/stages.json"))["stages"]["ours"]
oldf=json.load(open("/tmp/prof/src/docs/measurements/h264-gap-2/evidence/stages.json"))["stages"]["ffsimd"]
print("| Stage | ms ours | ms FFmpeg SIMD | ours/SIMD | gap ms | old ms ours | old gap ms |")
T=[0,0,0,0]
for st in stages.STAGES:
    o,f=tot["ours"][st],tot["ffsimd"][st]; oo=old.get(st,{}).get("ms",0); of=oldf.get(st,{}).get("ms",0)
    T=[T[0]+o,T[1]+f,T[2]+oo,T[3]+of]
    print(f"| {st} | {o} | {f} | {o/f:.2f} | {o-f:+d} | {oo} | {oo-of:+d} |")
print(f"| total | {T[0]} | {T[1]} | {T[0]/T[1]:.2f} | {T[0]-T[1]:+d} | {T[2]} | {T[2]-T[3]:+d} |")
print("\nTop 10 ours:")
for n,k in fn["ours"].most_common(10): print(f"{k:6d} {stages.stage(n,'ours'):25s} {n[:150]}")
for st in ("deblock strength/driver","idct/dequant/add","intra pred"):
    print(f"\n{st}:")
    for n,k in [x for x in fn["ours"].most_common() if stages.stage(x[0],"ours")==st][:5]: print(f"{k:6d} {n[:160]}")
print("\nother ours top:"); [print(f"{k:6d} {n[:150]}") for n,k in [x for x in fn["ours"].most_common() if stages.stage(x[0],"ours")=="other"][:10]]
print("\nother ffsimd top:"); [print(f"{k:6d} {n[:150]}") for n,k in [x for x in fn["ffsimd"].most_common() if stages.stage(x[0],"ffmpeg")=="other"][:10]]
json.dump({"per_stream":per,"total":{s:dict(v) for s,v in tot.items()},"functions":{s:v.most_common(200) for s,v in fn.items()}},open(G+"/stages.json","w"),indent=1)
