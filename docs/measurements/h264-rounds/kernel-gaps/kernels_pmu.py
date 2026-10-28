"""#486's PMU records by kernel family: CoreAV's kernels against FFmpeg's assembly counterparts, per class.

Usage: python3 -I kernels_pmu.py RES CORE_BIN > kernels.txt

RES is #486's `res/` (raw.tar.gz on measured/h264-profile-acf3ee2f-pmu) and CORE_BIN a build of `acf3ee2f` with line
tables that reproduces #486's `run/stages.txt` byte for byte (the check that its code layout is the profiled one).

- A CoreAV sample's family is the first match, from the innermost inline frame outwards, among the frames in the
  `coreav-kernels-h264` crate (KERNEL_RULES, on the function name; `prefetch.rs` by file). Helper frames (`b1`,
  `store`, `bytes`, ...) match nothing, so the sample goes to the kernel entry that inlined them; inside
  `intra4x4_mb`, an IDCT helper's sample is the IDCT's and a prediction helper's the prediction's, as FFmpeg's
  separate functions split them. A sample in the crate that matches no rule is the motion compensation driver's
  (`mc-driver`, against FFmpeg's `mc_part`/`mc_dir_part`/`hl_motion`) where a frame is in a `predict.rs`, the view
  constructors' (`views`) where one is in `views.rs`, else `glue:kernels`. Outside the crate: `deblock.rs` strengths (`bs`), and the rest by #486's stage
  (`glue:<stage>`).
- An FFmpeg sample's family is FF_RULES on its function symbol (function level only), else `glue:<stage>` by #486's
  FF_RULES.
- Cycles are a share of the same stream's FFmpeg cycles (the attribution build, same kind of record), mean of the
  rounds, then mean over the class's streams, as in #486. Instructions per macroblock likewise.
"""
import ast, bisect, collections, glob, json, os, re, subprocess, sys

res, core_bin = sys.argv[1], sys.argv[2]
here = os.path.dirname(os.path.abspath(__file__))
pmu = os.path.join(here, '..', 'profile-acf3ee2f-pmu')

# #486's stage classifiers and constants, taken from its files rather than copied.
src = open(os.path.join(pmu, 'stages_pmu.py')).read()
ns = {'re': re, 'os': os, 'ast': ast, 'here': pmu}
for n in ast.parse(src).body:
    if (isinstance(n, ast.Assign) and any(getattr(t, 'id', '') in ('FR', 'FF_RULES') for t in n.targets)) or \
            (isinstance(n, ast.FunctionDef) and n.name in ('ff_stage', 'load_delta')):
        exec(compile(ast.Module(body=[n], type_ignores=[]), 'stages_pmu.py', 'exec'), ns)
csrc = open(os.path.join(pmu, '..', 'profile-6c9e240', 'classify.py')).read()
fn = next(n for n in ast.parse(csrc).body if isinstance(n, ast.FunctionDef) and n.name == 'core_stage')
exec(compile(ast.Module(body=[fn], type_ignores=[]), 'classify.py', 'exec'), ns)
FR, ff_stage, load_delta, core_stage = ns['FR'], ns['ff_stage'], ns['load_delta'], ns['core_stage']

KERNEL_RULES = [
    ('mc-weight', r'weigh'),
    ('idct-luma-dc', r'luma_dc'),
    ('idct-chroma', r'chroma_residual|chroma_dc'),
    ('idct8', r'idct8'),
    ('idct4', r'idct4|add_residual|residual_4x4|dc_add|pair_rows|transpose4x2'),
    ('intra4x4', r'intra4x4|predict4x4'),
    ('intra8x8', r'intra8x8|predict8x8'),
    ('intra16x16', r'intra16x16|predict16'),
    ('intra-chroma', r'intra_chroma|predict_chroma'),
    ('deblock-chroma', r'chroma16|filter_chroma|chroma_mb'),
    ('deblock-luma', r'luma16|luma_columns|luma_rows|thresholds|deblock_row|filter_edge'),
    ('mc-chroma', r'chroma_block|chroma_mc'),
    ('mc-luma', r'luma_block|luma_mc|interpolate'),
]
FF_KERNEL_RULES = [
    ('mc-weight', r'weight'),
    ('deblock-luma', r'deblock_[vh]_luma|deblock_[vh]8_luma|loop_filter_luma'),
    ('deblock-chroma', r'deblock_[vh]_chroma|loop_filter_chroma'),
    ('bs', r'loop_filter_strength'),
    ('idct-luma-dc', r'luma_dc_dequant'),
    ('idct-chroma', r'chroma_dc_dequant|idct_add8_8|idct_add8$'),
    ('idct8', r'idct8'),
    ('idct4', r'idct|add8x4'),
    ('intra8x8', r'pred8x8l'),
    ('intra4x4', r'pred4x4'),
    ('intra16x16', r'pred16x16'),
    ('intra-chroma', r'pred8x8'),
    ('mc-chroma', r'chroma_mc|chroma\d*_mc'),
    ('mc-luma', r'qpel|pixels'),
    ('prefetch', r'prefetch'),
    ('emu-edge', r'emu_edge|emulated_edge'),
    ('mc-driver', r'mc_part|mc_dir_part|hl_motion'),
]
KERNELS = [k for k, _ in KERNEL_RULES] + ['bs', 'prefetch', 'emu-edge', 'mc-driver', 'views']


def strip(n):
    while re.search(r'<[^<>]*>', n):
        n = re.sub(r'<[^<>]*>', '', n)
    return n.removesuffix('::').split('::')[-1]


def core_family(ch):
    for name, loc in ch:
        if 'coreav-kernels-h264' not in loc:
            continue
        if loc.rsplit(':', 2)[0].endswith('/prefetch.rs'):
            return 'prefetch'
        f = strip(name)
        for fam, rx in KERNEL_RULES:
            if re.search(rx, f):
                return fam
    if any(l.rsplit(':', 2)[0].endswith('coreav-h264/src/deblock.rs') and
           re.search(r'strength|same_motion', strip(n)) for n, l in ch):
        return 'bs'
    files = [l.rsplit(':', 2)[0] for _, l in ch if 'coreav-kernels-h264' in l]
    if any(f.endswith('/predict.rs') for f in files):
        return 'mc-driver'
    if any(f.endswith('/views.rs') for f in files):
        return 'views'
    if files:
        return 'glue:kernels'
    return 'glue:' + core_stage([[n, l.rsplit(':', 2)[0]] for n, l in ch])


def ff_family(name):
    for fam, rx in FF_KERNEL_RULES:
        if re.search(rx, name):
            return fam
    return 'glue:' + ff_stage(name)


syms = []
for l in open(f'{res}/ffattr.nm.txt'):
    p = l.split()
    if len(p) == 3 and p[1] in 'tTwW' and '.' not in p[2]:
        syms.append((int(p[0], 16), p[2]))
syms.sort()
sa = [a for a, _ in syms]
ffd = load_delta(open(f'{res}/ffattr.readelf-l.txt'))


def ff_func(a):
    i = bisect.bisect_right(sa, a + ffd) - 1
    return syms[i][1] if i >= 0 else '??'


recs = sorted(glob.glob(f'{res}/rec/*.json'))
dso = os.path.basename(core_bin)
addrs = sorted({a for p in recs if '.core.' in p for rows in json.load(open(p)).values() for d, a, n in rows if d == dso})
cd = load_delta(subprocess.run(['readelf', '-lW', core_bin], capture_output=True, text=True).stdout.splitlines())
txt = subprocess.run(['llvm-addr2line', '--output-style=LLVM', '--inlines', '-f', '-C', '-e', core_bin],
                     input='\n'.join(hex(a + cd) for a in addrs) + '\n', capture_output=True, text=True).stdout
fam_of = {}
for a, b in zip(addrs, txt.split('\n\n')):
    ls = [l for l in b.strip().split('\n') if l]
    fam_of[a] = core_family([[ls[i], ls[i + 1]] for i in range(0, len(ls) - 1, 2)])

dims = {}
for l in open(os.path.join(pmu, 'dims.txt')):
    f, d = l.split()
    w, h, n = (int(x) for x in d.split(','))
    dims[f.split('.')[0]] = ((w + 15) // 16) * ((h + 15) // 16) * min(n, FR[f.split('.')[2]])

nr = collections.Counter(tuple(os.path.basename(p).split('.')[:4]) for p in recs)
w = collections.defaultdict(lambda: collections.defaultdict(float))  # (stream, class, dec, event) -> family -> weight
for p in recs:
    s, c, dec, g, r = os.path.basename(p)[:-5].split('.')
    k = 1 / nr[(s, c, dec, g)]
    for ev, rows in json.load(open(p)).items():
        for d, a, n in rows:
            if dec == 'core':
                fam = fam_of.get(a, 'glue:other') if d == dso else 'glue:other'
            else:
                fam = ff_family(ff_func(a)) if d == 'ffattr' else 'glue:other'
            w[(s, c, dec, ev.split(':')[0])][fam] += n * k
streams = sorted({(k[0], k[1]) for k in w})
fams = KERNELS + sorted({f for v in w.values() for f in v if f.startswith('glue:')})


def mean(xs):
    return sum(xs) / len(xs) if xs else 0.0


out = {}
print('Kernel families: cycles as a share of FFmpeg\'s (mean over the class\'s streams), instructions per macroblock')
for cls in ('sd', '720p', '1080p', '4k', 'all'):
    ss = [s for s, c in streams if cls in ('all', c)]
    print(f'\n{cls} (n={len(ss)})')
    print(f"  {'family':16} {'CoreAV':>7} {'FFmpeg':>7} {'ratio':>6} {'excess':>7} {'ins/MB C':>9} {'ins/MB F':>9}")
    rows = []
    for fam in fams:
        cy = {dec: mean([w[(s, c, dec, 'cycles')][fam] / sum(w[(s, c, 'ffa', 'cycles')].values())
                         for s, c in streams if s in ss]) for dec in ('core', 'ffa')}
        ins = {dec: mean([w[(s, c, dec, 'instructions')][fam] / dims[s] for s, c in streams if s in ss])
               for dec in ('core', 'ffa')}
        rows.append((fam, cy['core'], cy['ffa'], ins['core'], ins['ffa']))
    out[cls] = rows
    for fam, c, f, ic, iff in sorted(rows, key=lambda r: -(r[1] - r[2])):
        if c < 0.0005 and f < 0.0005:
            continue
        ratio = f'{c / f:6.2f}' if f > 0.0005 else '     -'
        print(f'  {fam:16} {c:7.4f} {f:7.4f} {ratio} {c - f:+7.4f} {ic:9.0f} {iff:9.0f}')
json.dump(out, open(os.environ.get('KERNELS_JSON', '/dev/null'), 'w'), indent=1)

# Each kernel family's cycles as a share of CoreAV's own cycles on the stream, median over the class's streams: the
# most removing the whole part could save, on this host (the part-only bound before any probe on the timing host).
print('\nKernel families as a share of CoreAV\'s cycles, median per stream (%)')
print(f"  {'family':16} " + ' '.join(f'{c:>6}' for c in ('sd', '720p', '1080p', '4k', 'all')))
for fam in KERNELS:
    meds = []
    for cls in ('sd', '720p', '1080p', '4k', 'all'):
        xs = sorted(w[(s, c, 'core', 'cycles')][fam] / sum(w[(s, c, 'core', 'cycles')].values())
                    for s, c in streams if cls in ('all', c))
        meds.append(100 * (xs[len(xs) // 2] + xs[(len(xs) - 1) // 2]) / 2)
    if max(meds) >= 0.05:
        print(f'  {fam:16} ' + ' '.join(f'{m:6.2f}' for m in meds))
