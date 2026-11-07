import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import test_oracle

SOURCES = {
    'A': ('0639188c8624cc0b91d71ffec43ea67aebe6a0b414f57c6d8d3e820a6c55e3a0', 'fps=30', 640, 480, '756fa818691a60fa95f987bc606b00c95776a81142e7500cb39fac263cae9583'),
    'B': ('8d78eeaf2f396f1b090b86e068e04247ed81f3008bfda6f6cd08c1a715917d77', 'fps=30,scale=368:800', 368, 800, '927826f9a28494d3e5230cead67555bbf20aab63817b94bbd595eb8b11f99efd'),
    'C': ('de7348e23632ef363b4ec9516454784fb508209d4747ebc8ff34e7e848c6ac69', 'fps=30,scale=480:406,pad=480:416:0:0:color=black', 480, 416, '9e988d8b0d5cb127ff7deac970c9f5657457ee21e51f10878ace941532d7da45'),
    'D': ('81b00f13a401575279a777593957ba8d42a38d90eab3826dabfab9a3e3643f25', 'fps=30,scale=640:360,pad=640:368:0:0:color=black', 640, 368, '5ae66071812260dd7efa10adcbe1a687a3a5c9cfd5084266a7514a6451544ca3'),
    'E': ('af234b239540e894ac419669e80eeca534e00abcbe27b80b861ec5f292d62ca1', 'fps=30,scale=640:360,pad=640:368:0:0:color=black', 640, 368, '1ae5e766dfa9852948c7b1e5c22908a328684fc1b8d496ef084f7e81707f735c'),
}

def digest(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

def run(command):
    env = {k: v for k, v in os.environ.items() if not k.startswith(('R2_', 'AWS_', 'GITHUB_TOKEN', 'GH_TOKEN'))}
    return subprocess.run(list(map(str, command)), env=env, check=True, capture_output=True, text=True, timeout=600).stdout

def prepare(label, root, oracle):
    import boto3
    root = Path(root)
    root.mkdir(parents=True, exist_ok=True)
    stem, chain, w, h, sha = SOURCES[label]
    client = boto3.client('s3', endpoint_url=os.environ['R2_ENDPOINT'], aws_access_key_id=os.environ['R2_ACCESS_KEY_ID'], aws_secret_access_key=os.environ['R2_SECRET_ACCESS_KEY'], region_name='auto')
    keys = []
    lists = 0
    for page in client.get_paginator('list_objects_v2').paginate(Bucket=os.environ['R2_BUCKET']):
        lists += 1
        if lists > 32:
            raise ValueError('listing cap exceeded')
        for item in page.get('Contents', []):
            name = item['Key'].rsplit('/', 1)[-1]
            if stem in (name.split('.')[0], hashlib.sha256(name.encode()).hexdigest()):
                keys.append(item['Key'])
    if len(keys) != 1:
        raise ValueError('source selection failed')
    original = root / 'original'
    client.download_file(os.environ['R2_BUCKET'], keys[0], str(original))
    if stem not in (digest(original), hashlib.sha256(keys[0].rsplit('/', 1)[-1].encode()).hexdigest()):
        raise ValueError('source identity failed')
    raw = root / 'input.yuv'
    run([oracle.ffmpeg, '-nostdin', '-v', 'error', '-y', '-i', original, '-vf', chain, '-frames:v', 60, '-pix_fmt', 'yuv420p', '-f', 'rawvideo', raw])
    original.unlink()
    if digest(raw) != sha or raw.stat().st_size != w * h * 3 // 2 * 60:
        raise ValueError('preparation mismatch')
    return raw, lists, w, h, sha

def main():
    label, root, oracle_bin = sys.argv[1:]
    root = Path(root)
    oracle = test_oracle.require(Path(oracle_bin) / 'ffmpeg', Path(oracle_bin) / 'ffprobe')
    raw, lists, w, h, sha = prepare(label, root, oracle)
    cpu = min(os.sched_getaffinity(0))
    report = {'label': label, 'prepared_sha256': sha, 'cpu': cpu, 'r2_lists': lists, 'r2_reads_upper': 8, 'host': run(['lscpu']), 'load': run(['uptime']), 'rustc': run(['rustc', '-Vv']), 'arms': json.loads(Path('ab/arms.json').read_text()), 'binary_sha256': {}, 'calls': [], 'cells': []}
    arms = ['main', 'candidate', 'scalar']
    for arm in arms:
        report['binary_sha256'][arm] = digest(root.parent / f'{arm}-encode')
    if len(set(report['binary_sha256'].values())) != 3:
        raise ValueError('build arms are not distinct')
    for qp in [10, 22, 28, 34, 40, 51]:
        expected = None
        def call(arm, repeat):
            stream, recon = root / f'{arm}.h264', root / f'{arm}.rec'
            seconds = float(run(['taskset', '-c', cpu, root.parent / f'{arm}-encode', w, h, qp, 60, raw, stream, recon, 32]).strip())
            if seconds <= 0 or recon.stat().st_size != raw.stat().st_size:
                raise ValueError('incomplete encode')
            hashes = digest(stream), digest(recon)
            if expected is not None and hashes != expected:
                raise ValueError('stream or reconstruction identity failed')
            report['calls'].append({'qp': qp, 'arm': arm, 'repeat': repeat, 'seconds': seconds, 'stream_sha256': hashes[0], 'reconstruction_sha256': hashes[1]})
            return hashes, seconds
        for arm in arms:
            hashes, _ = call(arm, -1)
            if expected is None:
                expected = hashes
            decoded = root / 'decoded.yuv'
            run([oracle.ffmpeg, '-nostdin', '-v', 'error', '-y', '-i', root / f'{arm}.h264', '-f', 'rawvideo', '-pix_fmt', 'yuv420p', decoded])
            if digest(decoded) != hashes[1]:
                raise ValueError('oracle decode mismatch')
        if qp in [22, 28, 34, 40]:
            times = {arm: [] for arm in arms[:2]}
            for repeat in range(7):
                order = arms[:2] if repeat % 2 == 0 else list(reversed(arms[:2]))
                for arm in order:
                    _, seconds = call(arm, repeat)
                    times[arm].append(seconds)
            med = {arm: statistics.median(values) for arm, values in times.items()}
            row = {'qp': qp, 'seconds': {arm: {'median': med[arm], 'min': min(values), 'max': max(values)} for arm, values in times.items()}, 'speedup': med['main'] / med['candidate']}
            report['cells'].append(row)
            print(label, qp, json.dumps(row), flush=True)
        (root / 'report.json').write_text(json.dumps(report, indent=2) + '\n')

if __name__ == '__main__':
    main()
