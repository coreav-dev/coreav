#!/usr/bin/env python3
"""Draw the H.264 held-out set (docs/measurements/h264-n2-heldout/README.md).

    python3 -I scripts/h264_heldout_draw.py --out docs/measurements/h264-n2-heldout

Pool: corpus v1 `real` streams that N1's corpus run decoded and that agreed
with FFmpeg (docs/spikes/h264-n1/evidence/corpus.jsonl.gz), minus

  - the streams of N2's held-out set (docs/spikes/h264-n2/sets/heldout.json), and
  - every stream whose first 12 hex digits of the file's SHA-256 appear in
    the tracked text of the paths below (tuning, profiling and A/B sets, and
    any other place the decoder work names a stream),

and minus streams whose estimated FFmpeg time for the frames measured is
under 0.4 s. The estimate is N1's FFmpeg seconds scaled by
min(frames measured, N1 frames) / N1 frames. N1 measured FFmpeg's seconds
over the whole stream, up to 9,000 frames.

Draw: per resolution class, order the pool by HMAC-SHA256(salt,
"<sha256>.<track>") ascending and take the first COUNT. Writes pool.json,
excluded.json and list.json.
"""
import argparse
import gzip
import hashlib
import hmac
import json
import re
import subprocess

SALT = b'coreav-h264-n2-heldout-1:20261021'
COUNT = 16
# Frames measured per class: chosen so FFmpeg's time per stream is about
# half a second or more, where process start-up no longer dominates.
FRAMES = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
FLOOR_SECONDS = 0.4
# As the paths were at BASE, before the spike notes moved to docs/spikes/.
SCAN_PATHS = ['docs', 'crates', 'bench', 'scripts', 'spikes/h264-n1/README.md',
              'spikes/h264-n2/README.md', 'spikes/h264-n2/sets']
BASE = 'ca5e4bc8'


def klass(w, h):
    if w <= 640 and h <= 480:
        return 'sd'
    if w <= 1280 and h <= 720:
        return '720p'
    if w <= 1920 and h <= 1088:
        return '1080p'
    return '4k'


def named_prefixes():
    """12-hex tokens in the tracked text of SCAN_PATHS at BASE, apart from
    this measurement's own directory."""
    out = subprocess.run(
        ['git', 'grep', '-h', '-o', '-E', '[0-9a-f]{12}', BASE, '--', *SCAN_PATHS,
         ':(exclude)docs/measurements/h264-n2-heldout'],
        capture_output=True, text=True, check=False).stdout
    return set(re.findall(r'[0-9a-f]{12}', out))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', required=True)
    args = ap.parse_args()
    rows = [json.loads(line) for line in
            gzip.open('docs/spikes/h264-n1/evidence/corpus.jsonl.gz', 'rt')]
    spent = {(s['sha256'], s['v']) for s in
             json.load(open('docs/spikes/h264-n2/sets/heldout.json'))['streams']}
    named = named_prefixes()
    pool, excluded = [], {'n2_heldout': [], 'named': [], 'under_floor': []}
    for x in rows:
        if x['set'] != 'real':
            continue
        for y in x['results']:
            if y.get('status') != 'agree' or y['ffmpeg']['seconds'] < 0.5:
                continue
            sps = y['sps'][0]
            c = klass(sps['width'], sps['height'])
            ident = f"{x['sha256']}.{y['v']}"
            frames = y['ffmpeg']['frames']
            est = y['ffmpeg']['seconds'] * min(FRAMES[c], frames) / frames
            rec = {'id': ident, 'key': x['key'], 'sha256': x['sha256'], 'track': y['v'],
                   'class': c, 'width': sps['width'], 'height': sps['height'],
                   'profile_idc': sps['profile_idc'], 'n1_frames': frames,
                   'estimated_ffmpeg_seconds': round(est, 3), 'frames_measured': min(FRAMES[c], frames)}
            if (x['sha256'], y['v']) in spent:
                excluded['n2_heldout'].append(ident)
            elif x['sha256'][:12] in named:
                excluded['named'].append(ident)
            elif est < FLOOR_SECONDS:
                excluded['under_floor'].append(ident)
            else:
                rec['digest'] = hmac.new(SALT, ident.encode(), hashlib.sha256).hexdigest()
                pool.append(rec)
    chosen = []
    for c in FRAMES:
        members = sorted((r for r in pool if r['class'] == c), key=lambda r: r['digest'])
        chosen += members[:COUNT]
    meta = {'salt': SALT.decode(), 'salt_sha256': hashlib.sha256(SALT).hexdigest(),
            'count_per_class': COUNT, 'frames_per_class': FRAMES,
            'floor_seconds': FLOOR_SECONDS, 'base': BASE,
            'pool_per_class': {c: sum(1 for r in pool if r['class'] == c) for c in FRAMES},
            'excluded_counts': {k: len(v) for k, v in excluded.items()}}
    with open(f'{args.out}/pool.json', 'w') as f:
        json.dump({'meta': meta, 'pool': sorted(pool, key=lambda r: r['digest'])}, f, indent=1)
    with open(f'{args.out}/excluded.json', 'w') as f:
        json.dump(excluded, f, indent=1)
    with open(f'{args.out}/list.json', 'w') as f:
        json.dump({'meta': meta, 'streams': chosen}, f, indent=1)
    print(json.dumps(meta, indent=1))


if __name__ == '__main__':
    main()
