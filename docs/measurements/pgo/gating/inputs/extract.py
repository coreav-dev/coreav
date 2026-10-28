import json, os, subprocess, hashlib, sys, boto3
L = json.load(open(os.path.expanduser('~/hd/list.json')))
F = L['meta']['frames_per_class']
s3 = boto3.client('s3', endpoint_url=os.environ['R2_ENDPOINT'], aws_access_key_id=os.environ['R2_ACCESS_KEY_ID'],
                  aws_secret_access_key=os.environ['R2_SECRET_ACCESS_KEY'], region_name='auto')
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
out = {}
for s in L['streams']:
    name = f"{s['sha256'][:12]}.{s['track']}.{s['class']}.h264"
    dst = os.path.expanduser(f'~/hd/es/{name}')
    tmp = os.path.expanduser('~/hd/tmp/src' + os.path.splitext(s['key'])[1])
    if os.path.exists(dst) and os.path.getsize(dst) > 0:
        pass
    else:
        s3.download_file(os.environ['R2_BUCKET'], store_key(s3, s['key']), tmp)
        r = subprocess.run(['/opt/coreav-oracle-bin/ffmpeg', '-nostdin', '-v', 'error', '-y', '-i', tmp, '-map', f"0:v:{s['track']}",
                            '-c:v', 'copy', '-bsf:v', 'h264_mp4toannexb', '-frames:v', str(F[s['class']]), '-f', 'h264', dst],
                           capture_output=True, text=True)
        os.remove(tmp)
        if r.returncode != 0:
            print('FAIL', name, r.stderr[:200]); out[name] = {'error': r.stderr[:200]}; continue
    h = hashlib.sha256(open(dst, 'rb').read()).hexdigest()
    out[name] = {'es_sha256': h, 'es_size': os.path.getsize(dst), 'class': s['class'], 'frames': F[s['class']]}
    print(name, out[name]['es_size'], flush=True)
json.dump(out, open(os.path.expanduser('~/hd/es.json'), 'w'), indent=1)
