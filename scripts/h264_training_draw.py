#!/usr/bin/env python3
"""Draw the H.264 training set (docs/measurements/h264-training/README.md).

    python3 -I scripts/h264_training_draw.py --out docs/measurements/h264-training

Pool: the pool of the held-out draw of step 6's measurement
(docs/measurements/h264-n2-heldout/pool.json), which is already corpus v1
`real` streams that N1's corpus run decoded in agreement with FFmpeg, minus
N2's held-out set, minus every stream named in the tracked text of the
repository at that draw, and minus streams under FFmpeg's time floor. From
it this script removes the 64 streams that draw took
(docs/measurements/h264-n2-heldout/list.json), which are spent.

Draw: per resolution class, order what is left by HMAC-SHA256(salt,
"<sha256>.<track>") ascending and take the first COUNT. Writes list.json.
The streams it takes are named in this repository from then on, so the
held-out draw's own rule leaves them out of any later held-out draw.
"""
import argparse
import hashlib
import hmac
import json

SALT = b'coreav-h264-training-1:20261021'
COUNT = 8
HELDOUT = 'docs/measurements/h264-n2-heldout'


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', required=True)
    args = ap.parse_args()
    pool = json.load(open(f'{HELDOUT}/pool.json'))
    spent = {s['id'] for s in json.load(open(f'{HELDOUT}/list.json'))['streams']}
    frames = pool['meta']['frames_per_class']
    left = []
    for r in pool['pool']:
        if r['id'] in spent:
            continue
        r = dict(r)
        r['digest'] = hmac.new(SALT, r['id'].encode(), hashlib.sha256).hexdigest()
        left.append(r)
    chosen = []
    for c in frames:
        members = sorted((r for r in left if r['class'] == c), key=lambda r: r['digest'])
        chosen += members[:COUNT]
    profiles = {}
    for r in chosen:
        profiles[str(r['profile_idc'])] = profiles.get(str(r['profile_idc']), 0) + 1
    meta = {'salt': SALT.decode(), 'salt_sha256': hashlib.sha256(SALT).hexdigest(),
            'count_per_class': COUNT, 'frames_per_class': frames,
            'heldout_pool_sha256': hashlib.sha256(open(f'{HELDOUT}/pool.json', 'rb').read()).hexdigest(),
            'spent': len(spent),
            'left_per_class': {c: sum(1 for r in left if r['class'] == c) for c in frames},
            'profile_idc_counts': profiles}
    with open(f'{args.out}/list.json', 'w') as f:
        json.dump({'meta': meta, 'streams': chosen}, f, indent=1)
    print(json.dumps(meta, indent=1))


if __name__ == '__main__':
    main()
