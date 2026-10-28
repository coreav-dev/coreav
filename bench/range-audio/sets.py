"""Writes the sets of #31's measurement (README.md, "Sets") from corpus
v1's manifest: the objects with an AAC track that corpus v1 holds out, drawn per stratum by a new salt, and the tuning pool.

    python3 -I sets.py > OUT.json

Population: `real` files tagged `c:isobmff` and `a:aac`. Held-out set:
among those corpus v1 holds out, within each stratum, the files ordered by
sha256(SALT + sha256), the first COUNTS[stratum]. Tuning pool: the
population that corpus v1 does not hold out; development and tuning use
only these."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / 'corpus' / 'v1' / 'manifest.json'
SALT = 'coreav-corpus-v1-holdout-issue31:'
LARGE = 64 << 20
COUNTS = {'video-small': 24, 'video-large': 24, 'audio-only': 12}


def stratum(f):
    if 'audio-only' in f['tags']:
        return 'audio-only'
    return 'video-large' if f['size'] > LARGE else 'video-small'


def entry(f):
    return {**{k: f[k] for k in ('sha256', 'key', 'size', 'ext', 'tags')}, 'stratum': stratum(f)}


def main():
    data = MANIFEST.read_bytes()
    population = [f for f in json.loads(data)['files']
                  if f['set'] == 'real' and 'c:isobmff' in f['tags'] and 'a:aac' in f['tags']]
    held = []
    for name, count in COUNTS.items():
        candidates = [f for f in population if f['holdout'] and stratum(f) == name]
        candidates.sort(key=lambda f: hashlib.sha256((SALT + f['sha256']).encode()).hexdigest())
        held.extend(candidates[:count])
    pool = sorted((f for f in population if not f['holdout']), key=lambda f: f['sha256'])
    print(json.dumps({
        'source': 'corpus v1 real c:isobmff a:aac (bench/range-audio/sets.py)',
        'manifest_sha256': hashlib.sha256(data).hexdigest(),
        'salt': SALT,
        'counts': COUNTS,
        'held_out': [entry(f) for f in held],
        'tuning_pool': [entry(f) for f in pool],
    }, indent=1))


if __name__ == '__main__':
    main()
