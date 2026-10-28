"""table.py bound.jsonl: per stream and per class (medians), the bound of plan.txt and the parts reported as
information."""
import json, statistics as st, sys
rows = [json.loads(l) for l in open(sys.argv[1])]
G = ['R:new', 'R:apply', 'R:prepare_level_scales', 'R:chroma_qp']
def p(r, g, k): return r['parts'].get(g, {}).get(k, 0.0)
print(f'{"stream":22} {"bound":>6} ' + ' '.join(f'{g[2:12]:>10}' for g in G) + f' {"glue max":>8} {"i4x4 t":>7} {"i4x4 i":>7}')
for r in rows:
    print(f'{r["stream"]:22} {r["bound"]:5.2f}% ' + ' '.join(f'{p(r, g, "max%"):9.3f}%' for g in G)
          + f' {p(r, "glue", "max%"):7.2f}% {p(r, "K:intra4x4", "time%"):6.2f}% {p(r, "K:intra4x4", "instr%"):6.2f}%')
print()
build = False
for c in ('sd', '720p', '1080p', '4k'):
    rs = [r for r in rows if r['stream'].endswith('.' + c)]
    m = st.median(r['bound'] for r in rs); build |= m >= 1.5
    print(f'{c:6} n={len(rs)} bound median {m:.2f}% (max {max(r["bound"] for r in rs):.2f}%); R time {st.median(sum(p(r, g, "time%") for g in G) for r in rs):.2f}%'
          f' instr {st.median(sum(p(r, g, "instr%") for g in G) for r in rs):.2f}%; glue max(t,i) {st.median(p(r, "glue", "max%") for r in rs):.2f}%'
          f'; intra4x4 time {st.median(p(r, "K:intra4x4", "time%") for r in rs):.2f}% instr {st.median(p(r, "K:intra4x4", "instr%") for r in rs):.2f}%')
print('threshold: build if a class median bound >= 1.5%:', 'BUILD' if build else 'do not build')
