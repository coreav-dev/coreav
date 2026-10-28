import pickle, re, ast, collections, os, sys
per, chains = pickle.load(open('/tmp/ent/core.pkl', 'rb'))
here = '$HOME/coreav/docs/measurements/h264-rounds/profile-6c9e240'
src = open(os.path.join(here, 'classify.py')).read()
fn = next(n for n in ast.parse(src).body if isinstance(n, ast.FunctionDef) and n.name == 'core_stage')
ns = {'re': re}; exec(compile(ast.Module(body=[fn], type_ignores=[]), 'c', 'exec'), ns); core_stage = ns['core_stage']
FR = {'sd': 180, '720p': 90, '1080p': 30, '4k': 15}
dims = {}
for l in open('$HOME/coreav/docs/measurements/h264-rounds/pmu-profile/dims.txt'):
    f, d = l.split(); w, h, n = (int(x) for x in d.split(','))
    s, c = f.split('.')[0], f.split('.')[2]
    dims[s] = ((w + 15) // 16) * ((h + 15) // 16) * min(n, FR[c])
# Each decoder by the macroblocks it decoded (mbcount.py): both decode past the N pictures they are asked to output.
import json as _json
_m = _json.load(open('/tmp/ent/mbs.json'))
dims_ff = {k: v for k, v in _m['ff'].items()}
dims = {k: v for k, v in _m['core'].items()}
def clean(n):
    while re.search(r'<[^<>]*>', n): n = re.sub(r'<[^<>]*>', '', n)
    return n.removesuffix('::').split('::')[-1]
def short(ch):  # outer -> inner coreav frames
    fr = [f"{f[1].split(':')[0].rsplit('/',1)[-1]}:{clean(f[0])}" for f in ch if 'coreav-' in f[1]]
    return fr[::-1]
def norm(sel=None):
    out = collections.defaultdict(float)
    for (s, c), d in per.items():
        if sel and sel(s, c) is False: continue
        for a, ir in d.items():
            ch = chains.get(a, [])
            k = (core_stage([[x[0], x[1].rsplit(':', 2)[0]] for x in ch]), tuple(short(ch)))
            out[k] += ir / dims[s] / len(per)
    return out
