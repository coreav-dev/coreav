"""Writes the `npx coreav` input list from corpus v1's manifest
(docs/design/npm-cli.md, "Bars"): the `real` files with extension mp4, mov,
m4v, m4a, m4b, 3gp, mkv or webm, at most 128 MiB, sorted by SHA-256.

    python3 -I inputs.py held-out > OUT.json
    python3 -I inputs.py development > /tmp/npx-dev.json"""
import hashlib
import json
from pathlib import Path
import sys

MANIFEST = Path(__file__).resolve().parents[2] / 'corpus' / 'v1' / 'manifest.json'
EXTENSIONS = ('mp4', 'mov', 'm4v', 'm4a', 'm4b', '3gp', 'mkv', 'webm')
CEILING = 128 << 20


def main():
    which = sys.argv[1] if len(sys.argv) == 2 else None
    if which not in ('held-out', 'development'):
        raise SystemExit('usage: inputs.py held-out|development')
    data = MANIFEST.read_bytes()
    files = [f for f in json.loads(data)['files']
             if f['set'] == 'real' and f['ext'] in EXTENSIONS and f['size'] <= CEILING
             and f['holdout'] == (which == 'held-out')]
    files.sort(key=lambda f: f['sha256'])
    print(json.dumps({
        'source': f'corpus v1 {which} real {", ".join(EXTENSIONS)} files of at most 128 MiB (bench/npx/inputs.py)',
        'manifest_sha256': hashlib.sha256(data).hexdigest(),
        'count': len(files),
        'bytes': sum(f['size'] for f in files),
        'files': [{k: f[k] for k in ('sha256', 'key', 'size', 'ext')} for f in files],
    }, indent=1))


if __name__ == '__main__':
    main()
