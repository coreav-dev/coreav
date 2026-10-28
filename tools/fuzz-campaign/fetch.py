#!/usr/bin/env python3
"""Download one object of the corpus bucket and check its SHA-256.

  fetch.py KEY SHA256 OUT

Reads R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_ENDPOINT and R2_BUCKET, as
tools/stage3-report/corpus.py does; read access is enough.
"""
import hashlib
import importlib.util
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent.parent
SPEC = importlib.util.spec_from_file_location('corpus', ROOT / 'tools' / 'stage3-report' / 'corpus.py')
corpus = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(corpus)


def main():
    key, expected, out = sys.argv[1], sys.argv[2], pathlib.Path(sys.argv[3])
    pool = corpus.r2pool.Pool(corpus.credentials(), 8, corpus.r2pool.Limits(requests=10_000, bytes_=1 << 31),
                              timeout=300)
    try:
        size, etag = pool.head(key)
        data = bytearray(size)

        def sink(pos, chunk):
            data[pos:pos + len(chunk)] = chunk
        part = 16 << 20
        for offset in range(0, size, part):
            pool.get(key, size, etag, offset, min(part, size - offset), sink=sink)
    finally:
        pool.close()
    digest = hashlib.sha256(data).hexdigest()
    if digest != expected:
        raise SystemExit(f'{key}: SHA-256 {digest}, expected {expected}')
    out.write_bytes(data)
    print(f'{key}: {size} bytes, SHA-256 checked')
    return 0


if __name__ == '__main__':
    sys.exit(main())
