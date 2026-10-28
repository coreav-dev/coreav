"""python3 -I correct_reports.py REPO factors.json -> for every committed scripts/h264_parity.py report (JSON with per-stream `ratio` and
`seconds`) that holds one of the six streams whose `-f rawvideo` run was affected, each `:write` and `:mixed` ratio (ours over FFmpeg's
default-conversion `-f rawvideo` time) divided by that stream's factor (factors.json: passthrough time over default time, FFmpeg alone,
this host class), and the medians before and after, overall and per class. An estimate: the factor is measured here, one thread, and
applied to reports from other hosts and code states; no report is re-run. Discard ratios are not touched. Keys such as `main:write@8` (the `--threads 1,8` reports) are included. Printed: the `:write` rows
that move by 0.003 or more (the `:mixed` rows move alike)."""
import glob, json, os, statistics, sys
repo, fpath = sys.argv[1:3]
F = json.load(open(fpath))
AFF = sorted({k.split('.')[0] for k in F})
def factor(sid, rep):
    ff = str(rep.get('ffmpeg', ''))
    if '9.0.2' in ff: return F[f'{sid}.ff9']['factor']
    if 'coreav-oracle-bin' in ff: return F[f'{sid}.ff7']['factor']
    # `/work/bin/ffmpeg` or `ffmpeg` (a container's): the two versions' factors differ by under 2%
    return (F[f'{sid}.ff9']['factor'] + F[f'{sid}.ff7']['factor']) / 2
print('affected streams:', ', '.join(AFF))
rows = []
for p in sorted(glob.glob(f'{repo}/docs/measurements/**/*.json', recursive=True)):
    if os.path.basename(p).startswith('corrected.') or 'passthrough' in os.path.basename(p): continue
    try: d = json.load(open(p))
    except Exception: continue
    if not (isinstance(d, dict) and isinstance(d.get('streams'), list)): continue
    S = [x for x in d['streams'] if isinstance(x, dict) and isinstance(x.get('ratio'), dict) and any(':write' in k for k in x['ratio'])]
    hit = [x for x in S if x['stream'].split('.')[0] in AFF]
    if not hit: continue
    keys = sorted({k for x in S for k in x['ratio'] if ':write' in k or ':mixed' in k})
    for k in keys:
        def corr(x):
            v = x['ratio'].get(k)
            if v is None: return None
            return v / factor(x['stream'].split('.')[0], d) if x['stream'].split('.')[0] in AFF else v
        for lab in ('overall', 'sd', '720p', '1080p', '4k'):
            sel = [x for x in S if k in x['ratio'] and (lab == 'overall' or x.get('class') == lab)]
            if not any(x['stream'].split('.')[0] in AFF for x in sel): continue
            a = statistics.median(x['ratio'][k] for x in sel); b = statistics.median(corr(x) for x in sel)
            rows.append((os.path.relpath(p, repo + '/docs/measurements'), k, lab, len(sel), sum(x['stream'].split('.')[0] in AFF for x in sel), a, b))
for r in rows:
    if ':write' not in r[1] or abs(r[6] - r[5]) < 0.003: continue
    print('%-70s %-14s %-8s n=%-3d affected=%d median %.3f -> %.3f' % r)
