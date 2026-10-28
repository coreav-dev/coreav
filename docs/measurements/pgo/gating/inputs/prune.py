import os, json
M = {s['name']: os.path.basename(s['bitstream']) for s in json.load(open(os.path.expanduser('~/jvt/conformance-streams.json')))['streams']}
root = os.path.expanduser('~/jvt/extracted')
for name in os.listdir(root):
    for r, _, fs in os.walk(os.path.join(root, name)):
        for f in fs:
            if f != M[name] and f != '.done':
                os.remove(os.path.join(r, f))
p = os.path.expanduser('~/jvt/fetch.py'); s = open(p).read()
if 'keep =' not in s:
    a = s.index("    os.remove(zp)")
    s = s[:a] + "    keep = os.path.basename(s['bitstream'])\n    for root, _, fs in os.walk(dst):\n        for f in fs:\n            if f not in (keep, '.done'): os.remove(os.path.join(root, f))\n" + s[a:]
    open(p, 'w').write(s)
