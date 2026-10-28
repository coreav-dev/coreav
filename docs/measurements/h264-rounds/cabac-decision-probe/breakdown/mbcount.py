"""mbcount.py: macroblocks each decoder decoded, from callgrind call counts (CoreAV: calls of start_mb's out-of-line
instances; FFmpeg: calls of ff_h264_decode_mb_cabac / ff_h264_decode_mb_cavlc). Writes mbs.json."""
import glob, os, re, json, collections, sys
sys.path.insert(0, '/tmp/ent'); from sig import dims
def calls(path):
    names = {}; c = collections.Counter(); cfn = None
    for line in open(path):
        if line.startswith(('fn=', 'cfn=')):
            k = line.index('=') + 1; m = re.match(r'\((\d+)\)(?: (.*))?', line[k:].strip())
            if m and m.group(2): names[m.group(1)] = m.group(2)
            if line.startswith('cfn='): cfn = names.get(m.group(1), '?') if m else line[k:].strip()
        elif line.startswith('calls='):
            c[cfn] += int(line.split()[0][6:])
    return c
out = {'core': {}, 'ff': {}, 'nominal': {}}
for p in sorted(glob.glob('/tmp/ent/cg/*.out')):
    s, cl, dec = os.path.basename(p).split('.')[:3]
    c = calls(p)
    if dec == 'core':
        n = sum(v for k, v in c.items() if 'start_mb' in k and ('enter' in k or 'tier::Base' in k) and 'closure' not in k.split('start_mb')[0][-3:])
        out['core'][s] = n
    else:
        out['ff'][s] = c.get('ff_h264_decode_mb_cabac', 0) + c.get('ff_h264_decode_mb_cavlc', 0)
    out['nominal'][s] = dims[s]
json.dump(out, open('/tmp/ent/mbs.json', 'w'), indent=0)
for s in sorted(out['nominal']):
    print(f"{s} nominal {out['nominal'][s]:8} core {out['core'].get(s,0):8} ({out['core'].get(s,0)/out['nominal'][s]:.3f}) ff {out['ff'].get(s,0):8} ({out['ff'].get(s,0)/out['nominal'][s]:.3f})")
