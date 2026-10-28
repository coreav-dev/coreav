"""chainsplit.py CORE_DIR FF_DIR: time (over FFmpeg total, mean per stream, per class) of samples whose inline chain holds a frame
matching a group's regex, CoreAV vs FFmpeg; first matching group wins (innermost-first search over groups in order)."""
import collections, glob, json, os, re, sys
cd, fd = sys.argv[1:3]
GC = [('cabac_residual', r'significance_map|cabac::levels|coded_block_flag|parse_residual|residual_block_cabac|CabacSyntax.*block|syntax::block'),
      ('cavlc_residual', r'cavlc|CavlcSyntax.*block'),
      ('cabac_other', r'cabac'),]
GF = [('cabac_residual', r'decode_cabac_residual|decode_significance|decode_cabac_luma_residual|get_cabac_cbf_ctx'),
      ('cavlc_residual', r'decode_residual|decode_luma_residual|cavlc'),
      ('cabac_other', r'cabac'),]
tot = collections.Counter(); nr = collections.Counter()
for p in glob.glob(f'{cd}/*.ff.*.json'):
    s, c, _, r = os.path.basename(p)[:-5].split('.')
    tot[(s, c)] += sum(n for _, _, n in json.load(open(p))['samples']) / 4000; nr[(s, c)] += 1
tot = {k: v / nr[k] for k, v in tot.items()}
def run(d, dso, kind, G):
    R = json.load(open(f'{d}/addr.{dso}.json')); out = collections.defaultdict(collections.Counter); nf = collections.Counter()
    for p in glob.glob(f'{d}/*.{kind}.*.json'):
        s, c, _, r = os.path.basename(p)[:-5].split('.'); nf[c] += 1; nf['all'] += 1
        for name, a, n in json.load(open(p))['samples']:
            if name != dso: continue
            ch = R.get(str(a), []); txt = ' | '.join(f[0] for f in ch)
            for g, rx in G:
                if re.search(rx, txt): out[c][g] += n / 4000 / tot[(s, c)]; out['all'][g] += n / 4000 / tot[(s, c)]; break
    return {c: {g: v / nf[c] for g, v in o.items()} for c, o in out.items()}
C = run(cd, 'prof-main', 'core', GC); F = run(fd, 'ffmpeg_g', 'ff', GF)
for c in ('sd', '720p', '1080p', '4k', 'all'):
    print(c, '  '.join(f'{g} {C[c].get(g,0):.4f}/{F[c].get(g,0):.4f}' for g, _ in GC))
