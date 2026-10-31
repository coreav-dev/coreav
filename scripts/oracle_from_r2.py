#!/usr/bin/env python3
"""Fetch the pinned oracle FFmpeg archive from R2 when its publisher cannot
be reached.

The origin of record stays the publisher's archive named in
`scripts/oracle-ffmpeg.env`; the bench bucket keeps the same archive under
`tools/`, so that a host without a route to the publisher can still run the
oracle. It is checked against the pin's SHA-256 before it is written, and
`scripts/install-oracle-ffmpeg.sh` checks it again and extracts the two
executables.

  oracle_from_r2.py /absolute/path/of/the/archive

Credentials from R2_ENDPOINT, R2_ACCESS_KEY_ID and R2_SECRET_ACCESS_KEY.
Uses only the standard library (through tools/stage3-report/r2pool.py).
Exit 0 when the archive is written, 1 otherwise.
"""
import hashlib
import os
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tools" / "stage3-report"))
sys.path.insert(0, str(ROOT / "scripts"))
import oracle_pin  # noqa: E402
import r2pool  # noqa: E402

PART = 8 << 20
CAP = 512 << 20


def credentials():
    creds = {"access": os.environ.get("R2_ACCESS_KEY_ID"), "secret": os.environ.get("R2_SECRET_ACCESS_KEY"),
             "endpoint": os.environ.get("R2_ENDPOINT")}
    if not all(creds.values()):
        print("oracle_from_r2.py: R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and R2_ENDPOINT are required",
              file=sys.stderr)
        return None
    return creds


def main():
    if len(sys.argv) != 2:
        print(__doc__.strip().splitlines()[-6], file=sys.stderr)
        return 1
    creds = credentials()
    if creds is None:
        return 1
    pin = oracle_pin.load()
    key = f"tools/{pin['ARCHIVE']}.tar.xz"
    pool = r2pool.Pool(creds, 4, r2pool.Limits(requests=200, bytes_=CAP), timeout=300)
    try:
        size, etag = pool.head(key)
        if size > CAP:
            print(f"oracle_from_r2.py: {key} is {size} bytes, more than {CAP}", file=sys.stderr)
            return 1
        data = bytearray(size)

        def sink(pos, chunk):
            data[pos:pos + len(chunk)] = chunk

        for offset in range(0, size, PART):
            pool.get(key, size, etag, offset, min(PART, size - offset), sink=sink)
    finally:
        pool.close()
    got = hashlib.sha256(data).hexdigest()
    if got != pin["SHA256"]:
        print(f"oracle_from_r2.py: {key} is {got}, expected {pin['SHA256']}", file=sys.stderr)
        return 1
    pathlib.Path(sys.argv[1]).write_bytes(bytes(data))
    print(f"{key}: {size} bytes, sha256 {got}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
