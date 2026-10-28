# bound.py: per stream, the inter 8x8 transform, the luma residual glue, the 4x4 pairs and Intra_8x8 as shares of the
# stream's instructions (main, callgrind; edges.py output per stream in $CG), and an upper bound for a per-macroblock
# call plus an 8x8 DC-only path: all the glue removed, and 80% of the transform of the DC-only share of 8x8 blocks
# ("all dc": as if every inter 8x8 block were DC-only). Also the inter 8x8 transforms' count, and their instructions
# and the glue's per transform (the glue's per block is meaningful only where 8x8 blocks are most of the luma residual).
import json, glob, os, re, statistics as st
dc = {}
for l in open(os.path.join(os.path.dirname(os.path.abspath(__file__)), '../luma-dc/counts.txt')):
    m = re.match(r'(\S+\.(?:sd|720p|1080p|4k))\s+(\S+)\s+(\d+)', l)
    if m: dc[m.group(1)] = float(m.group(2)) if m.group(2) != '-' else 0.0
rows = []
for f in sorted(glob.glob(os.environ.get('CG', 'cg') + '/*.json')):
    s = os.path.basename(f)[:-5]; d = json.load(open(f)); t = d['total']
    def self_(k): return sum(v for n, v in d['self'].items() if n.endswith(k)) / t * 100
    def edge(a, b): return sum(e[2] for e in d['edge'] if a in e[0] and e[1].endswith(b)) / t * 100
    n8 = sum(e[3] for e in d['edge'] if 'reconstruct_inter' in e[0] and e[1].endswith('x86::idct8x8_add'))
    i8 = edge('reconstruct_inter', 'x86::idct8x8_add')
    glue = self_('>::reconstruct_inter') + self_('>::residual_4x4_luma')
    q4 = edge('residual_4x4_luma', 'idct4x4_add_quad')
    intra8 = edge('reconstruct_intra', 'intra8x8_mb')
    ub = glue + dc[s] * i8 * 0.8
    rows.append((s, i8, glue, q4, intra8, dc[s], ub, glue + i8 * 0.8, n8, i8 * t / 100 / max(n8, 1), glue * t / 100 / max(n8, 1)))
print(f'{"stream":22} {"idct8 inter":>11} {"glue":>6} {"quad4":>6} {"intra8":>6} {"p8dc":>5} {"bound":>6} {"all dc":>6} {"8x8 n":>8} {"Ir/8x8":>6} {"glue/8x8":>8}')
for r in rows:
    per = f'{r[9]:6.0f} {r[10]:8.0f}' if r[8] else f'{"-":>6} {"-":>8}'
    print(f'{r[0]:22} {r[1]:10.2f}% {r[2]:5.2f}% {r[3]:5.2f}% {r[4]:5.2f}% {r[5]:5.2f} {r[6]:5.2f}% {r[7]:5.2f}% {r[8]:8} {per}')
for c in ('sd', '720p', '1080p', '4k'):
    b = [r[6] for r in rows if r[0].endswith('.' + c)]
    a = [r[7] for r in rows if r[0].endswith('.' + c)]
    print(f'{c:6} bound median {st.median(b):.2f}%  max {max(b):.2f}%  streams >=1.5%: {sum(x >= 1.5 for x in b)} of {len(b)}'
          f'  | every 8x8 block DC-only: median {st.median(a):.2f}%, streams >=1.5%: {sum(x >= 1.5 for x in a)}')
print(f'all bound median {st.median(r[6] for r in rows):.2f}%')
