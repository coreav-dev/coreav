import io
import json
import os
from pathlib import Path
import tarfile
import sys
import hashlib

from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from reference import SOURCES, prepare, test_oracle

root, oracle_dir = map(Path, sys.argv[1:])
root.mkdir(parents=True, exist_ok=True)
oracle = test_oracle.require(oracle_dir / 'ffmpeg', oracle_dir / 'ffprobe')
requests = {}
with tarfile.open(root / 'inputs.tar', 'w') as archive:
    for label in SOURCES:
        raw, lists, width, height, sha = prepare(label, root / label, oracle)
        archive.add(raw, arcname=label + '.yuv')
        requests[label] = {'list_requests': lists, 'read_requests_upper': 8,
                           'width': width, 'height': height, 'prepared_sha256': sha}
        raw.unlink()
nonce = os.urandom(12)
key = hashlib.sha256(os.environ['R2_SECRET_ACCESS_KEY'].encode()).digest()
payload = AESGCM(key).encrypt(nonce, (root / 'inputs.tar').read_bytes(), b'h264-arm64-ceiling')
(root / 'inputs.enc').write_bytes(nonce + payload)
(root / 'inputs.tar').unlink()
(root / 'preparation.json').write_text(json.dumps(requests, indent=2) + '\n')
