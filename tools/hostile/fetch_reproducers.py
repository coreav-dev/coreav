#!/usr/bin/env python3
"""Fetch part 4 of the W1 hostile suite: public FFmpeg vulnerability reproducer
media files listed in reproducers.json.

Each file is downloaded, verified against its recorded sha256, and written
into its own directory under OUT_DIR. Files are never executed here and never
committed. A hash mismatch, a 404 or an oversized file is recorded as a
missing entry, not a silent change to the suite. Treat every downloaded file
as untrusted data.

Usage: fetch_reproducers.py OUT_DIR [--manifest reproducers.json] [--max-bytes N]
Writes OUT_DIR/manifest.json describing the files that were retrieved.
"""
import argparse
import hashlib
import io
import json
import pathlib
import sys
import tarfile
import urllib.request
import zipfile

HERE = pathlib.Path(__file__).resolve().parent


def fetch(url: str, max_bytes: int) -> bytes:
    request = urllib.request.Request(url, headers={'User-Agent': 'coreav-w1-benchmark'})
    with urllib.request.urlopen(request, timeout=60) as response:
        data = response.read(max_bytes + 1)
    if len(data) > max_bytes:
        raise ValueError('file exceeds the size cap')
    return data


def member(blob: bytes, name: str, max_bytes: int) -> bytes:
    """One archive member, held to the same size cap as a download: the
    declared size is checked first and the expanded read stops one byte past
    the cap, so a small compressed archive cannot expand without bound."""
    if name.endswith(('.zip',)) or zipfile.is_zipfile(io.BytesIO(blob)):
        with zipfile.ZipFile(io.BytesIO(blob)) as archive:
            if archive.getinfo(name).file_size > max_bytes:
                raise ValueError('archive member exceeds the size cap')
            with archive.open(name) as extracted:
                data = extracted.read(max_bytes + 1)
    else:
        with tarfile.open(fileobj=io.BytesIO(blob)) as archive:
            info = archive.getmember(name)
            if info.size > max_bytes:
                raise ValueError('archive member exceeds the size cap')
            extracted = archive.extractfile(info)
            if extracted is None:
                raise ValueError('archive member is not a file')
            data = extracted.read(max_bytes + 1)
    if len(data) > max_bytes:
        raise ValueError('archive member exceeds the size cap')
    return data


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('out_dir')
    parser.add_argument('--manifest', default=str(HERE / 'reproducers.json'))
    parser.add_argument('--max-bytes', type=int, default=20_000_000)
    args = parser.parse_args()
    out = pathlib.Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    entries = json.loads(pathlib.Path(args.manifest).read_text())['entries']
    retrieved, missing = [], []
    for entry in entries:
        try:
            blob = fetch(entry['download_url'], args.max_bytes)
            if entry.get('archive_member'):
                blob = member(blob, entry['archive_member'], args.max_bytes)
            digest = hashlib.sha256(blob).hexdigest()
            if digest != entry['sha256']:
                raise ValueError(f'sha256 mismatch: got {digest}')
            case_dir = out / entry['id']
            case_dir.mkdir(exist_ok=True)
            path = case_dir / (entry.get('filename') or 'reproducer')
            path.write_bytes(blob)
            retrieved.append({'name': str(path.relative_to(out)), 'part': 'reproducer',
                              'outcome': entry.get('outcome', 'bounded'),
                              'cve': entry.get('cve'), 'component': entry.get('component'),
                              'note': entry.get('page_url')})
        except Exception as error:  # noqa: BLE001 - any failure is a recorded miss
            missing.append({'id': entry['id'], 'reason': str(error)})
            sys.stderr.write(f"reproducer {entry['id']} unavailable: {error}\n")
    manifest = {'schema': 'coreav-w1-hostile/1', 'parts': ['reproducer'],
                'cases': retrieved, 'missing': missing}
    (out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'{len(retrieved)} reproducers retrieved, {len(missing)} unavailable')
    return 0


if __name__ == '__main__':
    sys.exit(main())
