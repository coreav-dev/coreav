#!/usr/bin/env python3
"""Fetch the corpus v1 development split for the metadata comparison.

For every `real` file with `holdout: false` and an MP4/MOV family extension:
download it once from R2, check its SHA-256 against the manifest, run the
pinned ffprobe 7.0.2 (`-show_format -show_streams`, as
`scripts/oracle-ffprobe.sh` does) on the full file, and keep a sparse copy in
which top-level `mdat` payloads are holes (the probe never reads them). The
full file is deleted afterwards. Held-out files are never fetched.

Usage: harvest.py FFPROBE R2_ENV_FILE OUT_DIR
OUT_DIR must contain manifest.json. Writes
OUT_DIR/ffprobe/NAME.json ({"exit", "stdout"}) and OUT_DIR/sparse/NAME.
Credentials are read from R2_ENV_FILE and never printed; the bucket is its
`Bucket`, or else the environment's R2_BUCKET.
Needs boto3 (run it from a virtual environment).
"""
import concurrent.futures
import hashlib
import json
import os
import subprocess
import sys

import boto3
from botocore.config import Config

EXTS = ('mp4', 'mov', 'm4a', '3gp', 'm4v', 'm4b')


def client(env_file):
    env = {}
    with open(env_file) as handle:
        for line in handle:
            if '=' in line:
                key, value = line.strip().split('=', 1)
                env[key] = value.strip().strip('"').strip("'")
    bucket = env.get('Bucket') or os.environ.get('R2_BUCKET')
    if not bucket:
        raise SystemExit('harvest.py: R2_ENV_FILE has no Bucket and R2_BUCKET is not set')
    return bucket, boto3.client('s3', endpoint_url=env['Endpoint'], aws_access_key_id=env['AccessKeyID'],
                                aws_secret_access_key=env['SecretAccessKey'], region_name='auto',
                                config=Config(retries={'max_attempts': 3}, max_pool_connections=16))


def development_files(manifest):
    return sorted((x for x in manifest['files']
                   if x['set'] == 'real' and not x['holdout'] and x['ext'] in EXTS),
                  key=lambda x: x['size'])


def copy_range(src, dst, start, length):
    src.seek(start)
    dst.seek(start)
    while length:
        block = src.read(min(length, 1 << 20))
        if not block:
            break
        dst.write(block)
        length -= len(block)


def sparse(path, target):
    """Copy every top-level box except mdat payloads; on a malformed box,
    copy the rest of the file as is."""
    size = os.path.getsize(path)
    with open(path, 'rb') as src, open(target, 'wb') as dst:
        dst.truncate(size)
        at = 0
        while at < size:
            src.seek(at)
            head = src.read(16)
            length, kind, header = int.from_bytes(head[:4], 'big'), head[4:8], 8
            if len(head) >= 16 and length == 1:
                length, header = int.from_bytes(head[8:16], 'big'), 16
            elif length == 0:
                length = size - at
            if len(head) < 8 or length < header or at + length > size:
                copy_range(src, dst, at, size - at)
                break
            copy_range(src, dst, at, header if kind == b'mdat' else length)
            at += length


def main():
    ffprobe, env_file, out = sys.argv[1:4]
    bucket, s3 = client(env_file)
    with open(f'{out}/manifest.json') as handle:
        files = development_files(json.load(handle))
    for sub in ('ffprobe', 'sparse', 'download'):
        os.makedirs(f'{out}/{sub}', exist_ok=True)

    def one(entry):
        name = f"{entry['sha256']}.{entry['ext']}"
        if os.path.exists(f'{out}/ffprobe/{name}.json') and os.path.exists(f'{out}/sparse/{name}'):
            return name, 'cached'
        full = f'{out}/download/{name}'
        s3.download_file(bucket, entry['key'], full)
        digest = hashlib.sha256()
        with open(full, 'rb') as handle:
            for block in iter(lambda: handle.read(1 << 20), b''):
                digest.update(block)
        if digest.hexdigest() != entry['sha256']:
            os.remove(full)
            return name, 'sha256-mismatch'
        try:
            run = subprocess.run([ffprobe, '-v', 'error', '-of', 'json', '-show_format', '-show_streams', full],
                                 capture_output=True, timeout=120)
            record = {'exit': run.returncode, 'stdout': run.stdout.decode('utf-8', 'replace')}
        except subprocess.TimeoutExpired:
            record = {'exit': 'timeout', 'stdout': ''}
        sparse(full, f'{out}/sparse/{name}.part')
        os.replace(f'{out}/sparse/{name}.part', f'{out}/sparse/{name}')
        with open(f'{out}/ffprobe/{name}.json', 'w') as handle:
            json.dump(record, handle)
        os.remove(full)
        return name, 'ok'

    with concurrent.futures.ThreadPoolExecutor(6) as pool:
        for done, (name, status) in enumerate(pool.map(one, files), 1):
            print(done, len(files), status, name, flush=True)


if __name__ == '__main__':
    main()
