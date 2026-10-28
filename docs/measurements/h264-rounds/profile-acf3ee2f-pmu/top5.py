"""top5.py STAGES_JSON: the five stages per class ranked by excess over parity (CoreAV's cycles less 1.00 x FFmpeg's,
as shares of FFmpeg's), with entropy and the MB layer counted as one stage, because FFmpeg inlines its motion
prediction and neighbour caches into its macroblock parse and a function-level profile cannot split them there."""
import json, sys
p = json.load(open(sys.argv[1]))['parity']
NAMES = {'em': 'entropy + MB layer', 'inter': 'inter prediction', 'deblock': 'deblocking', 'recon': 'reconstruction',
         'other': 'other'}
for lab in ('sd', '720p', '1080p', '4k', 'all'):
    r = p[lab]
    rows = {'em': {k: r['entropy'][k] + r['mb'][k] for k in ('core', 'ffmpeg', 'ins_per_mb_core', 'ins_per_mb_ffmpeg')}}
    rows.update({x: r[x] for x in ('inter', 'deblock', 'recon', 'other')})
    print(f'\n{lab}\n| rank | stage | CoreAV | FFmpeg | ratio | excess | instructions/MB CoreAV | FFmpeg |\n|---|---|---|---|---|---|---|---|')
    for i, (x, v) in enumerate(sorted(rows.items(), key=lambda kv: kv[1]['ffmpeg'] - kv[1]['core']), 1):
        print(f"| {i} | {NAMES[x]} | {v['core']:.3f} | {v['ffmpeg']:.3f} | {v['core'] / v['ffmpeg']:.2f} | "
              f"{v['core'] - v['ffmpeg']:+.3f} | {v['ins_per_mb_core']:,.0f} | {v['ins_per_mb_ffmpeg']:,.0f} |")
