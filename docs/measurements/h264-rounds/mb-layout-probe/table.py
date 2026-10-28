# table.py BOUND.JSONL: per stream and per class (median) the probe's bound and its parts, % of time; bytes and
# simulated D1 write misses a macroblock, R's and the whole decoder's.
import sys, json, statistics as st
rows = [json.loads(l) for l in open(sys.argv[1])]
P = ['R:finish_mb', 'R:record init', 'R:store_mb', 'R:motion rows', 'R:reset', 'C']
def part(r, g, k): return r['parts'].get(g, {}).get(k, 0.0)
print(f'{"stream":22} {"I":>5} {"S":>5} {"C":>5} {"bound":>6} | excess%: ' + ' '.join(f'{g[2:] if g != "C" else g:>11}' for g in P)
      + f' | {"R B/mb":>7} {"all B/mb":>8} {"R mw/mb":>7} {"all mw":>6}')
for r in sorted(rows, key=lambda r: (['sd', '720p', '1080p', '4k'].index(r['stream'].split('.')[2]), r['stream'])):
    rb = sum(part(r, g, 'bytes_mb') for g in P[:5]); rmw = sum(part(r, g, 'd1mw_mb') for g in P[:5])
    print(f'{r["stream"]:22} {r["I"]:5.2f} {r["S"]:5.2f} {r["C"]:5.2f} {r["bound"]:6.2f} | ' +
          ' '.join(f'{part(r, g, "excess%"):11.2f}' for g in P) + f' | {rb:7.0f} {r["bytes_mb_all"]:8.0f} {rmw:7.2f} {r["mw_mb_all"]:6.2f}')
print()
for c in ['sd', '720p', '1080p', '4k']:
    rs = [r for r in rows if r['stream'].endswith('.' + c)]
    m = lambda k: st.median(r[k] for r in rs)
    print(f'{c:6} median: I {m("I"):.2f}  S {m("S"):.2f}  C {m("C"):.2f}  bound {m("bound"):.2f}  (min {min(r["bound"] for r in rs):.2f}, '
          f'max {max(r["bound"] for r in rs):.2f}); streams with bound >= 1.5: {sum(r["bound"] >= 1.5 for r in rs)} of {len(rs)}')
print(f'all 32 median bound {st.median(r["bound"] for r in rows):.2f}')
print("\nwithout the share of other lines' excess (I + R excess + C), R's simulated D1 write misses a macroblock, and the")
print("share of macroblocks stored with one motion (store_mb's copy of rows 1 to 3, 144 bytes a call):")
for c in ['sd', '720p', '1080p', '4k']:
    rs = [r for r in rows if r['stream'].endswith('.' + c)]
    n = [r['I'] + r['R_excess'] + r['C'] for r in rs]
    mw = [sum(p['d1mw_mb'] for g, p in r['parts'].items() if g.startswith('R:')) for r in rs]
    nr = [x - r['parts']['R:reset']['instr%'] - r['parts']['R:reset']['excess%'] for x, r in zip(n, rs)]
    print(f'{c:6} median {st.median(n):.2f} (min {min(n):.2f}), without MbData::reset {st.median(nr):.2f}; R D1 write misses a macroblock median {st.median(mw):.2f} '
          f'of {st.median(r["mw_mb_all"] for r in rs):.2f}; one motion median {st.median(r["one_motion"] for r in rs):.2f}')
