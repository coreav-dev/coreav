# lines.py OUT [OUT...]: from callgrind outputs (--dump-instr=no), self instructions by (source file, line) for the
# files named in FILES, and by function; JSON per output next to it (OUT.lines.json)
import json, re, sys
FILES = ('recon.rs',)
for p in sys.argv[1:]:
    names = {}; fl = fi = None; fn = None; line = 0; after_call = False
    by_line = {}; by_fn = {}; calls = {}; edges = {}; total = 0; cfn = None
    def nm(kind, i, n):
        key = (kind, i)
        if n: names[key] = n
        return names.get(key, i)
    for l in open(p, errors='replace'):
        c = l[:1]
        if c.isdigit() or c in '+-*':
            parts = l.split()
            if len(parts) < 2: continue
            t = parts[0]
            if t == '*': pass
            elif t[0] in '+-': line += int(t)
            else: line = int(t)
            cost = int(parts[1])
            if after_call:
                after_call = False
                edges[(fn, cfn)] = edges.get((fn, cfn), 0) + cost
                continue
            by_fn[fn] = by_fn.get(fn, 0) + cost
            src = fi or fl or ''
            if src.endswith(FILES): by_line[f'{src.rsplit("/",1)[-1]}:{line}'] = by_line.get(f'{src.rsplit("/",1)[-1]}:{line}', 0) + cost
            continue
        m = re.match(r'(fl|fi|fe|fn|cfn|cfi|cfl|cob|ob)=\((\d+)\)(?: (.*))?', l)
        if m:
            k, i, n = m.groups(); n = n.strip() if n else None
            kind = {'fi': 'fl', 'fe': 'fl', 'cfi': 'fl', 'cfl': 'fl', 'cob': 'ob'}.get(k, k)
            kind = 'fn' if k == 'cfn' else kind
            v = nm(kind, i, n)
            if k == 'fl': fl = v; fi = None
            elif k in ('fi', 'fe'): fi = None if v == fl else v
            elif k == 'fn': fn = v; fi = None
            elif k == 'cfn': cfn = v
            continue
        if l.startswith('calls='):
            n_ = int(l.split('=')[1].split()[0]); calls[(fn, cfn)] = calls.get((fn, cfn), 0) + n_; after_call = True; continue
        if l.startswith('summary:') or l.startswith('totals:'): total = max(total, int(l.split()[1]))
    json.dump({'total': total, 'lines': by_line, 'self': by_fn,
               'edge': [[a, b, v, calls.get((a, b), 0)] for (a, b), v in edges.items()]}, open(p + '.lines.json', 'w'))
    print(p, total, sum(by_fn.values()))
