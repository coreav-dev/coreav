import hashlib, json, os, statistics, subprocess, sys
from pathlib import Path
SOURCES = {
    'A': ('0639188c8624cc0b91d71ffec43ea67aebe6a0b414f57c6d8d3e820a6c55e3a0', 'fps=30', 640, 480, '756fa818691a60fa95f987bc606b00c95776a81142e7500cb39fac263cae9583'),
    'B': ('8d78eeaf2f396f1b090b86e068e04247ed81f3008bfda6f6cd08c1a715917d77', 'fps=30,scale=368:800', 368, 800, '927826f9a28494d3e5230cead67555bbf20aab63817b94bbd595eb8b11f99efd'),
    'C': ('de7348e23632ef363b4ec9516454784fb508209d4747ebc8ff34e7e848c6ac69', 'fps=30,scale=480:406,pad=480:416:0:0:color=black', 480, 416, '9e988d8b0d5cb127ff7deac970c9f5657457ee21e51f10878ace941532d7da45'),
    'D': ('81b00f13a401575279a777593957ba8d42a38d90eab3826dabfab9a3e3643f25', 'fps=30,scale=640:360,pad=640:368:0:0:color=black', 640, 368, '5ae66071812260dd7efa10adcbe1a687a3a5c9cfd5084266a7514a6451544ca3'),
    'E': ('af234b239540e894ac419669e80eeca534e00abcbe27b80b861ec5f292d62ca1', 'fps=30,scale=640:360,pad=640:368:0:0:color=black', 640, 368, '1ae5e766dfa9852948c7b1e5c22908a328684fc1b8d496ef084f7e81707f735c'),
}
QPS = [22, 27, 32, 37, 40]
ARMS = ['main', 'cand', 'pad']
REPEATS = 7
WORK, BINS = Path(sys.argv[1]), Path(sys.argv[2])
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import test_oracle
FFMPEG = str(test_oracle.require(BINS / 'ffmpeg', BINS / 'ffprobe').ffmpeg)

def digest(path):
    with open(path, 'rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

def fetch():
    import boto3
    c = boto3.client('s3', endpoint_url=os.environ['R2_ENDPOINT'], aws_access_key_id=os.environ['R2_ACCESS_KEY_ID'],
                     aws_secret_access_key=os.environ['R2_SECRET_ACCESS_KEY'], region_name='auto')
    wanted = {v[0] for v in SOURCES.values()}
    index = {}
    for page in c.get_paginator('list_objects_v2').paginate(Bucket=os.environ['R2_BUCKET']):
        for item in page.get('Contents', []):
            name = item['Key'].rsplit('/', 1)[-1]
            for stem in (name.split('.')[0], hashlib.sha256(name.encode()).hexdigest()):
                if stem in wanted:
                    index[stem] = item['Key']
    assert set(index) == wanted, 'missing sources'
    for label, (stem, chain, w, h, sha) in SOURCES.items():
        original = WORK / 'original'
        c.download_file(os.environ['R2_BUCKET'], index[stem], str(original))
        assert stem in (digest(original), hashlib.sha256(index[stem].rsplit('/', 1)[-1].encode()).hexdigest())
        out = WORK / f'{label}.yuv'
        subprocess.run([FFMPEG, '-nostdin', '-v', 'error', '-y', '-i', str(original), '-vf', chain, '-frames:v', '60',
                        '-pix_fmt', 'yuv420p', '-f', 'rawvideo', str(out)], check=True, timeout=600)
        original.unlink()
        assert digest(out) == sha, f'prepared {label} mismatch'

def call(arm, label, qp):
    _, _, w, h, _ = SOURCES[label]
    s, r = WORK / f'{arm}.h264', WORK / f'{arm}.rec'
    out = subprocess.run(['taskset', '-c', '1', str(BINS / f'{arm}-abt'), str(w), str(h), str(qp), '60',
                          str(WORK / f'{label}.yuv'), str(s), str(r), '32'], check=True, capture_output=True, text=True, timeout=600)
    return float(out.stdout.strip()), digest(s), digest(r)

def decodes(arm, label):
    d = WORK / 'dec.yuv'
    subprocess.run([FFMPEG, '-nostdin', '-v', 'error', '-y', '-i', str(WORK / f'{arm}.h264'), '-f', 'rawvideo', '-pix_fmt', 'yuv420p', str(d)], check=True, timeout=600)
    return digest(d) == digest(WORK / f'{arm}.rec')




def call(arm, label, qp, threads=1):
    _, _, w, h, _ = SOURCES[label]
    stream, recon = WORK / f'{arm}.h264', WORK / f'{arm}.rec'
    result = subprocess.run([str(BINS / f'{arm}-abt'), str(w), str(h), str(qp), '60',
        str(WORK / f'{label}.yuv'), str(stream), str(recon), '32', str(threads)],
        check=True, capture_output=True, text=True, timeout=600)
    return float(result.stdout.strip()), digest(stream), digest(recon)


WORK.mkdir(parents=True, exist_ok=True)
fetch()
print("prepared A-E with matching digests", flush=True)
