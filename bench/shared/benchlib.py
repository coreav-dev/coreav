"""Shared parts of the benchmark drivers under bench/: R2 presigning and
verified downloads from the environment's credentials, JSON evidence
files, and the statistics (medians with bootstrap 95% intervals).

R2 credentials come from R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY and
R2_ENDPOINT, and the bucket from R2_BUCKET. Presigned URLs are
bearer tokens: they are passed to the engines measured, never written to
evidence or logs.

Statistics: per-file values, medians over files, bootstrap 95% intervals
over files (10,000 resamples, seed 20261018).

Python 3.11 standard library only."""
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timezone
import gzip
import hashlib
import hmac
import json
import os
from pathlib import Path
import random
import statistics as st
import subprocess
import time
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[2]


class R2:
    def __init__(self):
        env = os.environ
        try:
            self.access = env['R2_ACCESS_KEY_ID']
            self.secret = env['R2_SECRET_ACCESS_KEY']
            self.endpoint = env['R2_ENDPOINT'].rstrip('/')
            self.bucket = env['R2_BUCKET']
        except KeyError as missing:
            raise SystemExit(f'R2 credentials missing from the environment ({missing})') from None
        if urllib.parse.urlsplit(self.endpoint).scheme != 'https':
            raise SystemExit('R2_ENDPOINT must be https')

    def _sign(self, stamp, scope, canonical):
        to_sign = '\n'.join(['AWS4-HMAC-SHA256', stamp, scope, hashlib.sha256(canonical.encode()).hexdigest()])
        key = ('AWS4' + self.secret).encode()
        for part in (stamp[:8], 'auto', 's3', 'aws4_request'):
            key = hmac.new(key, part.encode(), hashlib.sha256).digest()
        return hmac.new(key, to_sign.encode(), hashlib.sha256).hexdigest()

    def presign(self, key, expires=3600):
        """A query-signed GET URL (SigV4, host header only)."""
        stamp = datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')
        scope = f'{stamp[:8]}/auto/s3/aws4_request'
        host = urllib.parse.urlsplit(self.endpoint).netloc
        path = '/' + urllib.parse.quote(f'{self.bucket}/{key}', safe='/~')
        params = {'X-Amz-Algorithm': 'AWS4-HMAC-SHA256', 'X-Amz-Credential': f'{self.access}/{scope}',
                  'X-Amz-Date': stamp, 'X-Amz-Expires': str(expires), 'X-Amz-SignedHeaders': 'host'}
        query = '&'.join(f"{urllib.parse.quote(k, safe='-_.~')}={urllib.parse.quote(v, safe='-_.~')}"
                         for k, v in sorted(params.items()))
        canonical = '\n'.join(['GET', path, query, f'host:{host}\n', 'host', 'UNSIGNED-PAYLOAD'])
        return f'{self.endpoint}{path}?{query}&X-Amz-Signature={self._sign(stamp, scope, canonical)}'


def download(r2, entry, dest, parts=8 << 20, workers=16):
    """Parallel ranged download of one object, verified by SHA-256.
    Returns seconds."""
    url = r2.presign(entry['key'])
    size = entry['size']
    start = time.perf_counter()
    fd = os.open(dest, os.O_CREAT | os.O_TRUNC | os.O_WRONLY, 0o600)
    try:
        os.ftruncate(fd, size)

        def part(offset):
            end = min(offset + parts, size) - 1
            request = urllib.request.Request(url, headers={'Range': f'bytes={offset}-{end}'})
            for attempt in range(4):
                try:
                    with urllib.request.urlopen(request, timeout=120) as response:
                        data = response.read()
                    if len(data) != end - offset + 1:
                        raise IOError('short part')
                    os.pwrite(fd, data, offset)
                    return
                except Exception:
                    if attempt == 3:
                        raise
                    time.sleep(1 + attempt)

        with ThreadPoolExecutor(workers) as pool:
            list(pool.map(part, range(0, size, parts)))
    finally:
        os.close(fd)
    if sha256_file(dest) != entry['sha256']:
        Path(dest).unlink(missing_ok=True)
        raise IOError(f'SHA-256 mismatch for {entry["sha256"][:12]}')
    return time.perf_counter() - start


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, 'rb') as handle:
        while chunk := handle.read(1 << 22):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + '.tmp')
    tmp.write_text(json.dumps(value, indent=1, sort_keys=True) + '\n')
    tmp.replace(path)


def append_jsonl(path, value):
    with open(path, 'a') as handle:
        handle.write(json.dumps(value, sort_keys=True) + '\n')


def machine():
    info = {'nproc': os.cpu_count()}
    try:
        for line in Path('/proc/cpuinfo').read_text().splitlines():
            if line.startswith('model name'):
                info['cpu'] = line.split(':', 1)[1].strip()
                break
    except OSError:
        pass
    return info



def tool_versions():
    """The first line each tool prints for its version."""
    def first_line(cmd):
        try:
            return subprocess.run(cmd, capture_output=True, text=True, timeout=30).stdout.splitlines()[0]
        except Exception:
            return None
    return {'ffmpeg': first_line(['ffmpeg', '-version']), 'node': first_line(['node', '--version'])}


SEED, RESAMPLES = 20261018, 10000


def boot(values, stat):
    if len(values) < 2:
        return [None, None]
    rng = random.Random(SEED)
    out = sorted(stat([rng.choice(values) for _ in values]) for _ in range(RESAMPLES))
    return [out[int(0.025 * RESAMPLES)], out[int(0.975 * RESAMPLES) - 1]]


def nearest(values, p):
    s = sorted(values)
    return s[max(0, min(len(s) - 1, -(-len(s) * p // 100) - 1))] if s else None


def dist(values):
    if not values:
        return None
    return {'n': len(values), 'median': st.median(values), 'median_ci': boot(values, st.median),
            'p10': nearest(values, 10), 'p90': nearest(values, 90), 'min': min(values), 'max': max(values)}


def lines(path):
    """The records of a results.jsonl, or of the results.jsonl.gz kept as
    evidence beside it."""
    path = Path(path)
    if not path.exists() and path.with_suffix('.jsonl.gz').exists():
        text = gzip.decompress(path.with_suffix('.jsonl.gz').read_bytes()).decode()
    else:
        text = path.read_text()
    return [json.loads(line) for line in text.splitlines() if line.strip()]
