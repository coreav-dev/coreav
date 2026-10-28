import subprocess, collections, sys
rows = [l.split() for l in open('/tmp/mbc/ips.txt')]
res = []
for n, a, sym in rows:
    chain = subprocess.run(['addr2line', '-i', '-e', '/tmp/mbc/decode', a], capture_output=True, text=True).stdout.split('\n')
    chain = [c.split('/src/')[-1].split(' ')[0] for c in chain if c]
    # first frame in a coreav file
    own = next((c for c in chain if c.startswith(('mb.rs', 'inter.rs', 'work.rs', 'cabac.rs', 'picture.rs'))), chain[0] if chain else '?')
    res.append((sym.split('>::')[-1][:16], own, int(n)))
agg = collections.Counter()
for s, l, n in res: agg[(s, l)] += n
for s in sys.argv[1:]:
    t = sum(n for (ss, l), n in agg.items() if ss == s)
    print(s, round(t/3/4.999), 'ms')
    for (ss, l), n in sorted(agg.items(), key=lambda r: -r[1]):
        if ss == s and n/3/4.999 >= 4: print(f"  {n/3/4.999:5.0f} ms  {l}")
