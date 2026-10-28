"""sites.py OUT: calls to BitReader::ue from decode_cavlc by call line, and the total instructions."""
import re,collections,sys
names={};files={};cur=None;cfn=None;pend=None;last=0;fl=fi=None;by=collections.Counter();tot=0
def nm(t,s):
    m=re.match(r'\((\d+)\)(?: (.*))?',s)
    if not m: return s
    if m.group(2): t[m.group(1)]=m.group(2)
    return t.get(m.group(1),'?')
for line in open(sys.argv[1],errors='replace'):
    if line.startswith('fn='): cur=nm(names,line[3:].strip()); fi=None; continue
    if line.startswith('fl='): fl=nm(files,line[3:].strip()); fi=None; continue
    if line.startswith(('fi=','fe=')): fi=nm(files,line[3:].strip()); continue
    if line.startswith('cfn='): cfn=nm(names,line[4:].strip()); continue
    if line.startswith(('cfi=','cfl=')): nm(files,line[4:].strip()); continue
    if line.startswith(('cob=','ob=')): continue
    if line.startswith('calls='): pend=int(line[6:].split()[0]); continue
    if line[:1] in '0123456789+-*':
        p=line.split(); s=p[0]
        ln= last+int(s) if s[0] in '+-' else (last if s=='*' else int(s)); last=ln
        if pend is not None:
            if 'BitReader>::ue' in cfn and 'decode_cavlc' in cur:
                by[f"{(fi or fl).rsplit('/',1)[-1]}:{ln}"]+=pend
            pend=None
        elif len(p)>1: tot+=int(p[1])
skip=sum(v for k,v in by.items() if k.startswith('mb.rs'))
mb=sum(by.values())-skip
print(f'total {tot} mb-layer ue calls {mb} mb_skip_run {skip} | 36 a call {36*mb/tot*100:.2f}% | '+' '.join(f'{k}={v}' for k,v in by.most_common()))
