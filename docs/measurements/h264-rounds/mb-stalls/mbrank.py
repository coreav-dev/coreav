"""mbrank.py BIN STREAM...: MB-layer functions: time share (perf), relative IPC, mispredicts per 1000 instr (ir.*), per stream."""
import sys, re
sys.path.insert(0, '$HOME/h264work/r25/tools')
from cgcmp_lib import load
PAT = re.compile(r'start_mb|MbData>::reset|store_mvd|mv_predictor|colocated|spatial_direct|fill_ctx|fill_mvd_cache|fill_intra_cache|'
                 r'predict_p_skip|store_mb|finish_mb|predict_partitions|predict_direct|resolve|Planes>::request|decode_cabac<|decode_cavlc<|neighbour|ref_idx')
b = sys.argv[1]
for s in sys.argv[2:]:
    sys.argv = ['ipc', s, b, b, '0']
    import io, contextlib
    g = {}
    with contextlib.redirect_stdout(io.StringIO()):
        src = open('$HOME/h264work/r25/tools/ipc.py').read()
        try: exec(compile(src, "ipc", "exec"), g)
        except ZeroDivisionError: pass
    ta, ia, TA, IA = g['ta'], g['ia'], g['TA'], g['IA']
    _, raw = load(f'$HOME/h264work/r25/ir.{b}.{s}.out')
    bm = {}
    for f, v in raw.items():
        k = g['norm'](f); bm[k] = bm.get(k, 0) + v[2] + v[4]   # Bcm + Bim (cache-sim=no: Ir Bc Bcm Bi Bim)
    print(f'== {s}  total {TA:.0f} ms')
    for k in sorted((k for k in ta if PAT.search(k)), key=lambda k: -ta[k])[:12]:
        rel = (ia.get(k, 0) / IA) / (ta[k] / TA)
        print(f'  {k[-62:]:62} {100*ta[k]/TA:5.2f}%  relIPC {rel:4.2f}  Bm/kI {1000*bm.get(k,0)/max(ia.get(k,1),1):5.1f}')
