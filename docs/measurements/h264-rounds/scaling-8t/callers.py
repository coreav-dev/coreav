"""callers.py BASE... (plan S2a): the growth from one thread to eight of each (libc or kernel symbol, first CoreAV
caller) pair, as % of our one-thread samples, mean over the BASEs given; reads BASE.1.txt and BASE.8.txt (perf script
with call chains)."""
import collections, re, sys
def parse(p):
    smp=[]; cur=None
    for line in open(p):
        if not line.strip():
            if cur: smp.append(cur); cur=None
            continue
        if not line.startswith('\t') and not line.startswith(' '*8+' '*0) or re.match(r'^\s*\d+\s*$', line):
            if re.match(r'^\s*\d+\s*$', line):
                cur=[int(line.split()[0]),[]]; continue
        m=re.match(r'^\s+([0-9a-f]+)\s+(.*?)\s+\((.*)\)\s*$', line)
        if m and cur is not None: cur[1].append((m.group(2), m.group(3)))
    if cur: smp.append(cur)
    return smp
SKIP=re.compile(r'^(std::|alloc|core::|<alloc|<core|__rust|with_capacity|allocate|try_allocate|from_elem|to_vec|clone|copy_from_slice|copy_nonoverlapping|spec_|extend|<\[|new|resize|fill|vec|from_|<std|<T as|\{closure|_mm|memcpy|memmove|memset|write|zeroed|into|map|call|run|__)', re.I)
def key(fr):
    d0=fr[0][1]
    if 'libc' not in d0 and 'kernel' not in d0: return None
    lf=('kernel ' if 'kernel' in d0 else 'libc ')+fr[0][0]
    for s,d in fr[1:]:
        if 'libc' in d or 'kernel' in d or 'vdso' in d: continue
        if SKIP.match(s): continue
        return (re.sub(r'\+0x.*','',lf)[:40], s[:90])
    return (lf[:40],'?')
agg=collections.Counter(); tot1=0; lib=collections.Counter()
for base in sys.argv[1:]:
    s1=parse(base+'.1.txt'); s8=parse(base+'.8.txt')
    t1=sum(w for w,_ in s1)
    for sign,ss in ((-1,s1),(1,s8)):
        for w,fr in ss:
            if not fr: continue
            k=key(fr)
            if k: agg[k]+=sign*w/t1/len(sys.argv[1:]); lib[sign]+=w/t1/len(sys.argv[1:])
print('libc and kernel share 1T %.2f%% 8T %.2f%%'%(100*lib[-1],100*lib[1]))
for k,v in sorted(agg.items(),key=lambda x:-abs(x[1]))[:40]: print(f'{100*v:7.2f} {k[0]:40s} {k[1]}')
