#!/usr/bin/env python3
"""The files of the 0.1.0 round (docs/measurements/compat-0.1.0/README.md,
"Inputs"), chosen from the private corpus manifests and fetched from the
corpus store. Everything this writes names files by hash: keep it private.

    round_files.py select --set NAME --out FILES.json [--manifests DIR] [--per-family N]
        The files of one set, in their order, as {family, sha256, ext,
        size, key}. The manifests are read from DIR (corpus/<name>/
        manifest.json under it) or else from the corpus store.

    round_files.py fetch --files FILES.json --dest DIR [--shard I/N]
        Downloads the files (or every Nth from the Ith, counting from 0)
        to DIR/<sha256>.<ext>, checks each one's size and SHA-256, and
        writes DIR/files.json as {family, path}, in the same order.

Sets:
  draw6          every `real` file of v2-heldout draw 6 (R2, R3)
  r4-draw6       the R4 selection: per family, the first 20 `real` files
                 of draw 6 of at most 256 MB in the order
                 sha256("coreav-compat-r4:" + sha256)
  r4-gate        the R4 gate: the same rule over the spent held-out sets,
                 corpus v1 (ISO base media, Matroska and WebM) and
                 v1-ts-audio (transport streams)
  rehearsal      spent files for the rehearsal of R2 and R3: the held-out
                 `real` files of corpus v1, v1-ts-audio and v2-heldout
                 draws 1 to 5, of at most 200 MB (the draw's form), at most
                 100 per family (the draw's size), in the order
                 sha256("coreav-compat-rehearsal:" + sha256)
  r4-rehearsal   the R4 rule over v2-heldout draws 1 to 5, spent files
                 standing in for draw 6 in a rehearsal of the R4 host

Credentials: R2_ENDPOINT, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and
R2_BUCKET; never printed. Python 3 standard library only.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / 'stage3-report'))
import release_round  # noqa: E402

MB = 1024 * 1024
PART = 8 * MB
# The family names of v2-heldout's manifest.
V2_FAMILIES = {'iso': 'mp4-mov', 'matroska': 'matroska-webm', 'ts': 'mpegts'}


def pool():
    import r2pool
    creds = {'access': os.environ.get('R2_ACCESS_KEY_ID'), 'secret': os.environ.get('R2_SECRET_ACCESS_KEY'),
             'endpoint': os.environ.get('R2_ENDPOINT')}
    if not all(creds.values()) or not os.environ.get('R2_BUCKET'):
        raise SystemExit('R2_ENDPOINT, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and R2_BUCKET are required')
    return r2pool.Pool(creds, 4, r2pool.Limits(requests=1 << 20, bytes_=1 << 40), timeout=300)


def manifest(name, local):
    if local:
        data = json.loads((pathlib.Path(local) / 'corpus' / name / 'manifest.json').read_text())
    else:
        p = pool()
        try:
            key = f'corpus/{name}/manifest.json'
            size, etag = p.head(key)
            data = json.loads(p.get(key, size, etag))
        finally:
            p.close()
    return data['files'] if isinstance(data, dict) else data


def candidates(corpus, files):
    """(family, entry with its full key) for the `real` held-out files of a
    corpus whose extension is one of a family's."""
    out = []
    for e in files:
        if e.get('set') != 'real' or not e.get('holdout'):
            continue
        fam = release_round.family('x.' + e['ext'])
        if e.get('family') and V2_FAMILIES.get(e['family']) != fam and corpus == 'v2-heldout':
            fam = None
        if corpus == 'v1' and fam == 'mpegts' or corpus == 'v1-ts-audio' and fam != 'mpegts':
            continue
        if fam is None:
            continue
        key = e['key'] if e['key'].startswith('corpus/') else f"corpus/{corpus}/{e['key']}"
        out.append((fam, {'family': fam, 'sha256': e['sha256'], 'ext': e['ext'], 'size': e['size'],
                          'key': key, 'draw': e.get('draw')}))
    return out


def ordered(pool_, salt, limit, per_family):
    """Per family, in the order sha256(salt + sha256), the first
    `per_family` files of at most `limit` bytes."""
    out = []
    for fam in release_round.FAMILIES:
        chosen = [e for f, e in pool_ if f == fam and e['size'] <= limit]
        chosen.sort(key=lambda e: hashlib.sha256((salt + e['sha256']).encode()).hexdigest())
        out.extend(chosen[:per_family])
    return out


def select(name, local=None, per_family=None):
    v2 = candidates('v2-heldout', manifest('v2-heldout', local))
    draw6 = [(f, e) for f, e in v2 if e['draw'] == 6]
    spent_v2 = [(f, e) for f, e in v2 if e['draw'] in (1, 2, 3, 4, 5)]
    if name == 'draw6':
        return [e for f in release_round.FAMILIES for g, e in draw6 if g == f]
    if name == 'r4-draw6':
        return ordered(draw6, 'coreav-compat-r4:', 256 * MB, per_family or 20)
    v1 = candidates('v1', manifest('v1', local)) + candidates('v1-ts-audio', manifest('v1-ts-audio', local))
    if name == 'r4-gate':
        return ordered(v1, 'coreav-compat-r4:', 256 * MB, per_family or 20)
    if name == 'rehearsal':
        return ordered(v1 + spent_v2, 'coreav-compat-rehearsal:', 200 * MB, per_family or 100)
    if name == 'r4-rehearsal':
        return ordered(spent_v2, 'coreav-compat-r4:', 256 * MB, per_family or 20)
    raise SystemExit(f'no set {name!r}')


SETS = ('draw6', 'r4-draw6', 'r4-gate', 'rehearsal', 'r4-rehearsal')


def fetch_one(p, entry, dest):
    path = dest / f"{entry['sha256']}.{entry['ext']}"
    if path.exists() and path.stat().st_size == entry['size'] and digest(path) == entry['sha256']:
        return path
    part = path.with_name(path.name + '.part')
    size, etag = p.head(entry['key'])
    if size != entry['size']:
        raise SystemExit(f"a file of the set has {size} bytes in the store, not {entry['size']}")
    with open(part, 'wb') as f:
        def sink(position, chunk):
            f.seek(position)
            f.write(chunk)
        for offset in range(0, size, PART):
            p.get(entry['key'], size, etag, offset, min(PART, size - offset), sink=sink)
    if digest(part) != entry['sha256']:
        part.unlink()
        raise SystemExit('a fetched file does not have the SHA-256 of the manifest')
    part.rename(path)
    return path


def digest(path):
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for block in iter(lambda: f.read(MB), b''):
            h.update(block)
    return h.hexdigest()


def fetch(files, dest, shard=None):
    if shard:
        i, n = shard
        files = files[i::n]
    dest.mkdir(parents=True, exist_ok=True)
    p = pool()
    try:
        with concurrent.futures.ThreadPoolExecutor(4) as workers:
            paths = list(workers.map(lambda e: fetch_one(p, e, dest), files))
    finally:
        p.close()
    listing = [{'family': e['family'], 'path': str(path)} for e, path in zip(files, paths)]
    (dest / 'files.json').write_text(json.dumps(listing, indent=1) + '\n')
    return listing


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest='mode', required=True)
    p = sub.add_parser('select')
    p.add_argument('--set', choices=SETS, required=True)
    p.add_argument('--out', required=True)
    p.add_argument('--manifests')
    p.add_argument('--per-family', type=int)
    p = sub.add_parser('fetch')
    p.add_argument('--files', required=True)
    p.add_argument('--dest', required=True)
    p.add_argument('--shard', help='I/N: every Nth file from the Ith, counting from 0')
    opts = parser.parse_args()
    if opts.mode == 'select':
        files = select(opts.set, opts.manifests, opts.per_family)
        pathlib.Path(opts.out).write_text(json.dumps(files, indent=1) + '\n')
        counts = {f: sum(e['family'] == f for e in files) for f in release_round.FAMILIES}
        print(json.dumps({'set': opts.set, 'files': counts,
                          'megabytes': round(sum(e['size'] for e in files) / MB)}))
        return 0
    shard = tuple(int(x) for x in opts.shard.split('/')) if opts.shard else None
    listing = fetch(json.loads(pathlib.Path(opts.files).read_text()), pathlib.Path(opts.dest), shard)
    print(json.dumps({'fetched': len(listing)}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
