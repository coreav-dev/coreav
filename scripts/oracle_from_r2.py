#!/usr/bin/env python3
"""Fetch the pinned oracle executables from R2 when their publisher cannot
be reached.

The origin of record stays the pinned tarball
`ffmpeg-7.0.2-amd64-static.tar.xz`
(`scripts/install-oracle-ffmpeg.sh`); this is a copy of the two
executables it holds, kept in the bench bucket so that a host without a
route to the publisher can still run the oracle. Each is checked against
the SHA-256 below before it is installed, so the binary is the one every
committed correctness number used.

  oracle_from_r2.py /absolute/bin/dir

Credentials from R2_ENDPOINT, R2_ACCESS_KEY_ID and R2_SECRET_ACCESS_KEY.
Uses only the standard library (through tools/stage3-report/r2pool.py).
Exit 0 when both executables are installed, 1 otherwise.
"""
import hashlib
import os
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools" / "stage3-report"))
import r2pool  # noqa: E402

PREFIX = "tools/ffmpeg-7.0.2-amd64-static/"
# The executables of the pinned tarball (sha256 abda8d77…).
BINARIES = {
    "ffmpeg": "e7e7fb30477f717e6f55f9180a70386c62677ef8a4d4d1a5d948f4098aa3eb99",
    "ffprobe": "4f231a1960d83e403d08f7971e271707bec278a9ae18e21b8b5b03186668450d",
}
PART = 8 << 20


def credentials():
    creds = {"access": os.environ.get("R2_ACCESS_KEY_ID"), "secret": os.environ.get("R2_SECRET_ACCESS_KEY"),
             "endpoint": os.environ.get("R2_ENDPOINT")}
    if not all(creds.values()):
        print("oracle_from_r2.py: R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and R2_ENDPOINT are required",
              file=sys.stderr)
        return None
    return creds


def fetch(pool, name, want, dest):
    key = PREFIX + name
    size, etag = pool.head(key)
    data = bytearray(size)

    def sink(pos, chunk):
        data[pos:pos + len(chunk)] = chunk

    for offset in range(0, size, PART):
        pool.get(key, size, etag, offset, min(PART, size - offset), sink=sink)
    got = hashlib.sha256(data).hexdigest()
    if got != want:
        print(f"oracle_from_r2.py: {name} is {got}, expected {want}", file=sys.stderr)
        return False
    path = dest / name
    path.write_bytes(bytes(data))
    path.chmod(0o755)
    print(f"{name}: {size} bytes, sha256 {got}")
    return True


def main():
    if len(sys.argv) != 2:
        print(__doc__.strip().splitlines()[-4], file=sys.stderr)
        return 1
    creds = credentials()
    if creds is None:
        return 1
    dest = pathlib.Path(sys.argv[1])
    dest.mkdir(parents=True, exist_ok=True)
    budget = r2pool.Limits(requests=200, bytes_=512 << 20)
    pool = r2pool.Pool(creds, 4, budget, timeout=300)
    try:
        for name, want in BINARIES.items():
            if not fetch(pool, name, want, dest):
                return 1
    finally:
        pool.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
