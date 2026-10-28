"""Clip speed at several commits, interleaved on one machine.

Subset (fixed before the run): the 24 non-held-out inputs of
spikes/clip-bench/evidence/inputs.json with the lowest SHA-256 among those
of at most 150 MB. Clips: `cbench clips` of the main build. Per clip, one
warm-up of every build, then 5 rounds with the build order rotated; a
build's time is the median of its rounds (cbench's own in-process total).
"""
import json, os, pathlib, statistics, subprocess, sys, time
sys.path.insert(0, os.environ['HOME'] + '/coreav/corpus/v1/scripts')
import common
ROOT = pathlib.Path(os.environ['HOME']) / 'coreav'
CS = pathlib.Path('/tmp/cs')
builds = [l.split()[0] for l in (CS / 'commits.txt').read_text().splitlines() if l.strip()]
cbench = {b: str(CS / f'target-{b}' / 'release' / 'cbench') for b in builds}
rows = json.loads((ROOT / 'spikes/clip-bench/evidence/inputs.json').read_text())['files']
subset = sorted((r for r in rows if not r['holdout'] and r['size'] <= 150_000_000), key=lambda r: r['sha256'])[:24]
media = CS / 'media'; media.mkdir(exist_ok=True)
(CS / 'disc').mkdir(exist_ok=True)
out = CS / 'results.jsonl'
done = {json.loads(l)['sha'] for l in out.read_text().splitlines()} if out.exists() else set()
s3 = common.s3()

def ms(v):
    return str(v['num'] * 1000 // v['den'])

def run(build, src, clip, dst):
    end = (ms(clip['end']) if clip.get('end') else '-') + (':noaudio' if clip.get('no_audio') else '')
    p = subprocess.run([cbench[build], 'time', src, ms(clip['start']), end, dst, 'clip'], capture_output=True, text=True, timeout=900)
    try:
        r = json.loads(p.stdout.strip().splitlines()[-1])
    except (ValueError, IndexError):
        return None
    return r.get('total_ns') if r.get('ok') else None

for entry in subset:
    sha = entry['sha256'][:12]
    if sha in done:
        continue
    src = media / f"{entry['sha256']}.{entry['ext']}"
    s3.download_file(common.BUCKET, entry['key'], str(src))
    disc = subprocess.run([cbench['main'], 'clips', str(src), str(CS / 'disc')], capture_output=True, text=True, timeout=900)
    for p in (CS / 'disc').glob('*'):
        p.unlink()
    try:
        clips = [c for c in json.loads(disc.stdout.strip().splitlines()[-1]).get('clips', []) if c.get('ok')]
    except (ValueError, IndexError):
        clips = []
    result = {'sha': sha, 'clips': []}
    for clip in clips:
        dst = str(CS / 'out.mp4')
        for b in builds:
            run(b, str(src), clip, dst)  # warm-up
        times = {b: [] for b in builds}
        for r in range(5):
            order = builds[r % len(builds):] + builds[:r % len(builds)]
            for b in order:
                t = run(b, str(src), clip, dst)
                if t is not None:
                    times[b].append(t)
        result['clips'].append({'label': clip['label'], 'ns': {b: statistics.median(v) if v else None for b, v in times.items()}})
    src.unlink()
    with open(out, 'a') as f:
        f.write(json.dumps(result) + '\n')
    print(sha, len(result['clips']), 'clips', flush=True)
