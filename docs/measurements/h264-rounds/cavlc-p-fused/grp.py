"""grp.py CG.OUT BIN NMB: self Ir of every instruction, grouped by the innermost frame of its inline chain
that names one of the groups below (addr2line -i). Prints Ir per MB."""
import sys, re, collections, subprocess
out, binp, MB = sys.argv[1], sys.argv[2], float(sys.argv[3])
names={}
def nm(s):
    m=re.match(r'\((\d+)\)(?: (.*))?', s)
    if not m: return s
    if m.group(2): names[m.group(1)]=m.group(2)
    return names.get(m.group(1),'?')
cur=None; pending=False; last=[0,0]; costs=collections.Counter()
def pos(tok, i):
    if tok=='*': return last[i]
    if tok[0] in '+-': return last[i]+int(tok,0)
    return int(tok,0)
for line in open(out, errors='replace'):
    c=line[:1]
    if line.startswith('fn='): cur=nm(line[3:].strip()); continue
    if line.startswith('cfn='): nm(line[4:].strip()); continue
    if line.startswith('calls='): pending=True; continue
    if c in '0123456789+-*':
        p=line.split()
        if len(p)<3: continue
        a=pos(p[0],0); l=pos(p[1],1); last[0]=a; last[1]=l
        if pending: pending=False; continue
        costs[a]+=int(p[2])
tot=sum(costs.values())
addrs=sorted(costs)
frames={}
for i in range(0,len(addrs),5000):
    chunk=addrs[i:i+5000]
    res=subprocess.run(['addr2line','-i','-f','-C','-a','-e',binp]+[hex(a) for a in chunk],capture_output=True,text=True).stdout.splitlines()
    curA=None; fn=None
    for ln in res:
        if ln.startswith('0x') and fn is None: curA=int(ln,16); frames[curA]=[]; continue
        if fn is None: fn=ln; continue
        frames[curA].append((fn, ln.split(' ')[0])); fn=None
G=[('mc','motion_compensate|predict_fast|predict_list|predict_whole|luma_block|chroma_block|Windows|predict_kernel'),
   ('deblock','deblock'),('residual','parse_residual|residual_cavlc|block_in|parse_4x4|cavlc::'),
   ('recon','reconstruct'),
   ('parse_inter','parse_inter_pred|parse_mb_pred|parse_sub_mb_pred|parse_mvd|partition_shape|store_mvd|set_ref_gt0|uses_list|sub_uses_list|fill_mvd_cache'),
   ('fill_ctx','fill_ctx'),('mv_predictor','mv_predictor'),('store','store_motion|store_mb|resolve|put_rows'),
   ('predict_parts','predict_partitions|predict_sub_partitions'),('p_skip','predict_p_skip|decode_skip|still'),
   ('inter_pred','inter_pred'),
   ('fused','p_cavlc|predict_p|parse_p_|::l0|c_or_d|p_motion|put_head|put_blocks|store_two'),
   ('mb_reset','MbData>::reset'),('start_mb','start_mb|derive_neighbours|enter_row|issue_hints'),
   ('finish_mb','finish_mb'),('decode_mb','decode_mb|decode_cavlc|decode_slice')]
grp=collections.Counter()
for a in addrs:
    fr=frames.get(a,[])
    key='other'
    for f,loc in fr:   # innermost first
        hit=None
        for g,pat in G:
            if re.search(pat,f): hit=g; break
        if hit: key=hit; break
    grp[key]+=costs[a]
print(f'total {tot:,} = {tot/MB:.1f}/MB')
for k,v in grp.most_common(): print(f'{v/MB:8.1f}/MB {100*v/tot:6.2f}%  {k}')
