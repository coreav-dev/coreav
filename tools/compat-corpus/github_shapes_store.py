#!/usr/bin/env python3
"""Retain a bounded evidence tree and its verified receipt."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import zipfile

LIMIT = 512 * 1024 * 1024
REGISTRATIONS = ('github-shapes-200', 'github-shapes-200-pns-free')
ENVIRONMENT = ('R2_ENDPOINT', 'R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY', 'R2_BUCKET')


def store(root, prefix, client, bucket):
    root = Path(root)
    if not stat.S_ISDIR(root.lstat().st_mode) or not prefix or prefix.startswith('/') or any(
            part in ('', '.', '..') for part in prefix.split('/')):
        raise ValueError('invalid input')
    files = []
    for directory, dirs, names in os.walk(root, followlinks=False):
        for name in dirs:
            if not stat.S_ISDIR((Path(directory) / name).lstat().st_mode):
                raise ValueError('invalid directory')
        for name in names:
            path = Path(directory) / name
            if not stat.S_ISREG(path.lstat().st_mode):
                raise ValueError('invalid file')
            files.append(path)
    if not files:
        raise ValueError('empty evidence')
    inventory = {}
    total = 0
    with tempfile.TemporaryFile() as archive:
        with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED, allowZip64=True) as output:
            for path in sorted(files):
                name = path.relative_to(root).as_posix()
                flags = os.O_RDONLY | getattr(os, 'O_NOFOLLOW', 0)
                with os.fdopen(os.open(path, flags), 'rb') as source:
                    info = os.fstat(source.fileno())
                    if not stat.S_ISREG(info.st_mode):
                        raise ValueError('invalid file')
                    total += info.st_size
                    if total > LIMIT:
                        raise ValueError('evidence limit')
                    checksum = hashlib.sha256()
                    size = 0
                    with output.open(name, 'w', force_zip64=True) as target:
                        while chunk := source.read(1024 * 1024):
                            size += len(chunk)
                            if size > info.st_size:
                                raise ValueError('file changed')
                            checksum.update(chunk)
                            target.write(chunk)
                    if size != info.st_size:
                        raise ValueError('file changed')
                    inventory[name] = {'sha256': checksum.hexdigest(), 'size': size}
        size = archive.tell()
        if size > LIMIT:
            raise ValueError('archive limit')
        archive.seek(0)
        checksum = hashlib.sha256()
        while chunk := archive.read(1024 * 1024):
            checksum.update(chunk)
        archive_hash = checksum.hexdigest()
        archive.seek(0)
        client.put_object(Bucket=bucket, Key=f'{prefix}/evidence.zip', Body=archive)
        response = client.get_object(Bucket=bucket, Key=f'{prefix}/evidence.zip')['Body']
        try:
            checksum = hashlib.sha256()
            received = 0
            while chunk := response.read(1024 * 1024):
                received += len(chunk)
                if received > size:
                    raise ValueError('archive verification failed')
                checksum.update(chunk)
        finally:
            response.close()
        if received != size or checksum.hexdigest() != archive_hash:
            raise ValueError('archive verification failed')
    summaries = {name: inventory[name] for name in inventory if name.endswith('/summary.json')}
    complete = (inventory.get('completion.json') == {
        'sha256': hashlib.sha256(b'{"complete":true}\n').hexdigest(), 'size': len(b'{"complete":true}\n')}
        and 'build.json' in inventory and all(f'{name}/summary.json' in summaries for name in REGISTRATIONS))
    receipt = {'schema': 'coreav.github-shapes-retention/1', 'archive': {'sha256': archive_hash, 'size': size},
               'files': inventory, 'summaries': summaries, 'complete': complete}
    body = json.dumps(receipt, sort_keys=True, separators=(',', ':')).encode()
    client.put_object(Bucket=bucket, Key=f'{prefix}/retention.json', Body=body)
    response = client.get_object(Bucket=bucket, Key=f'{prefix}/retention.json')['Body']
    try:
        if response.read(len(body) + 1) != body:
            raise ValueError('receipt verification failed')
    finally:
        response.close()
    return complete


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise ValueError('invalid arguments')


def main(argv=None):
    try:
        parser = Parser(add_help=False)
        parser.add_argument('--root', required=True)
        parser.add_argument('--prefix', required=True)
        args = parser.parse_args(argv)
        env = {name: os.environ.get(name) for name in ENVIRONMENT}
        if not all(env.values()):
            raise ValueError('missing credentials')
        import boto3
        from botocore.config import Config
        client = boto3.client('s3', endpoint_url=env['R2_ENDPOINT'],
                              aws_access_key_id=env['R2_ACCESS_KEY_ID'],
                              aws_secret_access_key=env['R2_SECRET_ACCESS_KEY'], region_name='auto',
                              config=Config(connect_timeout=10, read_timeout=60,
                                            retries={'total_max_attempts': 3, 'mode': 'standard'}))
        complete = store(args.root, args.prefix, client, env['R2_BUCKET'])
        print('retention_complete' if complete else 'retention_incomplete')
        return 0 if complete else 1
    except Exception:
        print('retention_failed', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
