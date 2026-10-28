# groups.py CALLGRIND.OUT: cabac.rs Ir split into line ranges (decision chain, engine helpers, residual loops, glue), syntax.rs, mb.rs.
import sys, subprocess, re, collections
out = subprocess.run(['python3', '-I', 'lines.py', sys.argv[1], '100000'], capture_output=True, text=True).stdout.split('\n')
total = int(out[0].split()[1]); g = collections.Counter()
R = [('chain 190-216', 'cabac.rs', 190, 216), ('consume/refill/offset 150-178', 'cabac.rs', 150, 178), ('bypass/egk 217-260', 'cabac.rs', 217, 260),
     ('cabac other <150', 'cabac.rs', 1, 149), ('cabac 261-499', 'cabac.rs', 261, 499), ('unary/table/mvd 500-609', 'cabac.rs', 500, 609),
     ('residual 610-657', 'cabac.rs', 610, 657), ('sigmap 658-750', 'cabac.rs', 658, 750), ('levels 751-900', 'cabac.rs', 751, 900),
     ('cabac >900', 'cabac.rs', 901, 99999), ('cabac line0', 'cabac.rs', 0, 0)]
for l in out[1:]:
    m = re.match(r'\s*([\d.]+)% (\S+):(\d+)$', l)
    if not m: continue
    p, f, ln = float(m.group(1)), m.group(2), int(m.group(3))
    for name, ff, a, b in R:
        if f == ff and a <= ln <= b: g[name] += p
    if f in ('syntax.rs', 'mb.rs'): g[f] += p
for k, v in sorted(g.items(), key=lambda x: -x[1]): print(f'{v:6.2f}% {k}')
