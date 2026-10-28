"""cgparse.py OUT OBJ_SUBSTR: self Ir per address (dump-instr) for objects matching OBJ_SUBSTR -> dict addr->Ir"""
import re, collections
def parse(path, obj):
    names = {}; cost = collections.Counter(); ob_ok = False; last = 0; skip = False
    def nm(v):
        m = re.match(r'\((\d+)\)(?: (.*))?', v)
        if not m: return v
        if m.group(2): names[m.group(1)] = m.group(2)
        return names.get(m.group(1), '?')
    for line in open(path):
        c = line[0]
        if c == 'o' and line.startswith('ob='):
            ob_ok = obj in nm(line[3:].strip()); continue
        if c == 'c':
            if line.startswith('calls='): skip = True
            elif line.startswith('cob='): nm(line[4:].strip())
            continue
        if not (c == '0' or c in '+-*'): continue
        p = line.split()
        a = p[0]
        if a == '*': pos = last
        elif a[0] == '+': pos = last + int(a[1:], 0)
        elif a[0] == '-': pos = last - int(a[1:], 0)
        else: pos = int(a, 0)
        last = pos
        if skip: skip = False; continue
        if ob_ok and len(p) > 1: cost[pos] += int(p[1])
    return cost
