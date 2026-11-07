import collections
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tarfile

from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from reference import SOURCES, digest

root, bins, label = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3]
root.mkdir(parents=True, exist_ok=True)
payload = (root.parent / 'inputs/inputs.enc').read_bytes()
key = hashlib.sha256(os.environ['R2_SECRET_ACCESS_KEY'].encode()).digest()
plain = AESGCM(key).decrypt(payload[:12], payload[12:], b'h264-arm64-ceiling')
archive_path = root / 'inputs.tar'
archive_path.write_bytes(plain)
with tarfile.open(archive_path) as archive:
    member = archive.getmember(label + '.yuv')
    if not member.isfile():
        raise ValueError('input is not a regular file')
    source = archive.extractfile(member)
    with (root / 'input.yuv').open('wb') as destination:
        while block := source.read(1 << 20):
            destination.write(block)
archive_path.unlink()
del plain, payload, key
_, _, width, height, expected_sha = SOURCES[label]
if digest(root / 'input.yuv') != expected_sha:
    raise ValueError('prepared input mismatch')

def run(command, timeout=1800):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('R2_', 'AWS_', 'GH_', 'GITHUB_TOKEN'))}
    return subprocess.run(list(map(str, command)), env=env, capture_output=True,
                          text=True, check=True, timeout=timeout)

def functions(profile, inclusive):
    result = run(['callgrind_annotate', '--auto=no', '--show-percs=no',
                  '--inclusive=' + ('yes' if inclusive else 'no'), '--threshold=100', profile])
    (root / (profile.name + ('.inclusive.txt' if inclusive else '.exclusive.txt'))).write_text(result.stdout)
    rows = collections.defaultdict(int)
    for line in result.stdout.splitlines():
        match = re.match(r'^\s*([\d,]+)\s+(.*)$', line)
        if not match or 'PROGRAM TOTALS' in match[2]:
            continue
        count, name = int(match[1].replace(',', '')), match[2]
        name = re.sub(r'\s*\[[^\]]*\]\s*$', '', name)
        if 'coreav_h264_encoder::' not in name:
            continue
        name = name[name.index('coreav_h264_encoder::'):]
        rows[name] += count
    return dict(rows)

record = {'base_sha': os.environ['BASE'], 'workflow_sha': os.environ['GITHUB_SHA'],
          'architecture': run(['uname', '-m']).stdout.strip(),
          'host': run(['lscpu']).stdout, 'load': run(['uptime']).stdout,
          'rustc': run(['rustc', '-Vv']).stdout,
          'valgrind': run(['valgrind', '--version']).stdout.strip(),
          'binaries': {arm: digest(bins / arm) for arm in ['plain', 'attribute']},
          'input': label, 'prepared_sha256': expected_sha, 'frames': 60,
          'cells': [], 'preparation': json.loads((root.parent / 'inputs/preparation.json').read_text())}
if record['architecture'] != 'aarch64':
    raise ValueError('profiling host is not native arm64')
for qp in [22, 28]:
    hashes = None
    cell = {'qp': qp, 'arms': {}}
    for arm in ['plain', 'attribute']:
        profile = root / f'{qp}-{arm}.callgrind'
        command = ['valgrind', '--tool=callgrind', '--cache-sim=no', '--branch-sim=no',
                   '--collect-atstart=no', '--toggle-collect=*measured_encode*',
                   '--callgrind-out-file=' + str(profile), bins / arm, width, height,
                   qp, 60, root / 'input.yuv', root / 'out.h264', root / 'out.rec', 32]
        result = run(command)
        (root / f'{qp}-{arm}.log').write_text(result.stderr)
        observed = [digest(root / 'out.h264'), digest(root / 'out.rec')]
        if (root / 'out.rec').stat().st_size != width * height * 3 // 2 * 60:
            raise ValueError('incomplete reconstruction')
        if hashes is not None and observed != hashes:
            raise ValueError('attribution changes encoded output')
        hashes = observed
        total = next(int(line.split()[1]) for line in profile.read_text().splitlines()
                     if line.startswith('summary:'))
        if total < 1_000_000:
            raise ValueError('encoder collection did not execute')
        cell['arms'][arm] = {'instructions': total, 'stream_sha256': hashes[0],
                             'reconstruction_sha256': hashes[1],
                             'exclusive': functions(profile, False),
                             'inclusive': functions(profile, True)}
    record['cells'].append(cell)
    (root / 'result.json').write_text(json.dumps(record, indent=2) + '\n')
    print(label, qp, 'complete', flush=True)
for name in ['input.yuv', 'out.h264', 'out.rec']:
    (root / name).unlink()
