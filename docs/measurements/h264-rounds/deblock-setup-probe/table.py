"""table.py BOUND.JSONL: the probe's bound (plan.txt) per stream and per class, and per part."""
import json, sys, statistics as st
rows = [json.loads(l) for l in open(sys.argv[1])]
CLS = ['sd', '720p', '1080p', '4k']; cls = lambda r: r['stream'].rsplit('.', 1)[1]
PARTS = ['R:luma windows', 'R:luma params', 'R:chroma windows', 'R:chroma params', 'K']
print('stream                  bound,det  I      S      time%  ' + '  '.join(p[2:] if p != 'K' else 'K' for p in PARTS))
for r in sorted(rows, key=lambda r: (CLS.index(cls(r)), r['stream'])):
    t = sum(p['time%'] for p in r['parts'].values())
    print(f"{r['stream']:24s}{r['bound_detected']:6.2f}   {r['I']:5.2f}  {r['S']:5.2f}  {t:5.2f}  " +
          '  '.join(f"{r['parts'].get(p, {}).get('detected%', 0):5.2f}" for p in PARTS))
print()
print('| Class | bound, detected, median | range | streams at 1.5% or more | without the store term | time share of R and K |'
      + ''.join(f' {p[2:] if p != "K" else "K"} |' for p in PARTS))
print('|---|---|---|---|---|---|' + '---|' * len(PARTS))
for c in CLS:
    rs = [r for r in rows if cls(r) == c]; b = [r['bound_detected'] for r in rs]
    t = [sum(p['time%'] for p in r['parts'].values()) for r in rs]
    print(f"| {c} | {st.median(b):.2f}% | {min(b):.2f}% to {max(b):.2f}% | {sum(x >= 1.5 for x in b)} of {len(b)} | "
          f"{st.median(r['bound_no_store_term_detected'] for r in rs):.2f}% | {st.median(t):.2f}% |" +
          ''.join(f" {st.median(r['parts'].get(p, {}).get('detected%', 0) for r in rs):.2f}% |" for p in PARTS))
print()
print('instructions a macroblock, median by class: ' + '; '.join(
    f"{c} " + ', '.join(f"{p[2:] if p != 'K' else 'K'} {st.median(r['parts'].get(p, {}).get('ir_mb', 0) for r in rows if cls(r) == c):.0f}" for p in PARTS)
    for c in CLS))
