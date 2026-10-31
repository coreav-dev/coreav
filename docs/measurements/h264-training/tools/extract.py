"""Extract the training set's elementary streams.

    python -I extract.py LIST.json OUT_DIR

Downloads each listed file from corpus v1 (the bucket and credentials in
the R2_* environment variables, R2_BUCKET included; `key` in the list is `<sha256>.<ext>` and is looked up in the bucket), stream-copies its video track to Annex B with the
training extractor installed under RUNNER_TEMP, the first F access units of the class (no
decode), writes OUT_DIR/<sha256[:12]>.<track>.<class>.h264 and prints
es.json (the SHA-256 and size of every elementary stream) to stdout.
"""
import hashlib
import json
import os
import subprocess
import sys

import boto3


_INDEX = {}


def store_key(s3, name):
    """The object key of `<sha256>.<ext>` in the store: the bucket is listed once
    (under R2_KEY_PREFIX, default the whole bucket) and objects are matched by name,
    so the store's own layout is not written here."""
    if not _INDEX:
        for page in s3.get_paginator('list_objects_v2').paginate(
                Bucket=os.environ['R2_BUCKET'], Prefix=os.environ.get('R2_KEY_PREFIX', '')):
            for o in page.get('Contents', []):
                _INDEX[o['Key'].rsplit('/', 1)[-1]] = o['Key']
    return _INDEX[name]


def main():
    listing, out = sys.argv[1], sys.argv[2]
    L = json.load(open(listing))
    F = L['meta']['frames_per_class']
    ffmpeg = os.path.join(os.environ['RUNNER_TEMP'], 'coreav-training-extractor', 'ffmpeg')
    s3 = boto3.client('s3', endpoint_url=os.environ['R2_ENDPOINT'],
                      aws_access_key_id=os.environ['R2_ACCESS_KEY_ID'],
                      aws_secret_access_key=os.environ['R2_SECRET_ACCESS_KEY'], region_name='auto')
    os.makedirs(out, exist_ok=True)
    es = {}
    for s in L['streams']:
        name = f"{s['sha256'][:12]}.{s['track']}.{s['class']}.h264"
        dst = os.path.join(out, name)
        if not (os.path.exists(dst) and os.path.getsize(dst) > 0):
            tmp = os.path.join(out, 'src' + os.path.splitext(s['key'])[1])
            s3.download_file(os.environ['R2_BUCKET'], store_key(s3, s['key']), tmp)
            r = subprocess.run([ffmpeg, '-nostdin', '-v', 'error', '-y', '-i', tmp,
                                '-map', f"0:v:{s['track']}", '-c:v', 'copy',
                                '-bsf:v', 'h264_mp4toannexb', '-frames:v', str(F[s['class']]),
                                '-f', 'h264', dst], capture_output=True, text=True)
            os.remove(tmp)
            if r.returncode != 0:
                es[name] = {'error': r.stderr[:200]}
                print('FAIL', name, r.stderr[:200], file=sys.stderr)
                continue
        h = hashlib.sha256(open(dst, 'rb').read()).hexdigest()
        es[name] = {'es_sha256': h, 'es_size': os.path.getsize(dst), 'class': s['class'],
                    'frames': F[s['class']]}
        print(name, es[name]['es_size'], file=sys.stderr, flush=True)
    json.dump(es, sys.stdout, indent=1)


if __name__ == '__main__':
    main()
