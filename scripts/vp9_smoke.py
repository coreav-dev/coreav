#!/usr/bin/env python3
"""Run original-file VP9 comparisons on eight existing pinned public vectors."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time
from types import SimpleNamespace
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import test_oracle
import vp9_corpus

VECTORS = (
    'vp90-2-00-quantizer-10.webm',
    'vp90-2-00-quantizer-40.webm',
    'vp90-2-00-quantizer-63.webm',
    'vp90-2-02-size-320x180-05-resize-12-21-encode_speed-5.webm',
    'vp90-2-10-show-existing-frame.webm',
    'vp90-2-14-resize-10frames-fp-tiles-1-2-4-8.webm',
    'vp90-2-07-frame_parallel-1.webm',
    'vp90-2-17-show-existing-frame.webm',
)
LIMIT = 16 << 20
FAILURE_REASONS = {
    'unknown native eligibility': 'native_eligibility_unknown',
    'invalid native subsampling': 'native_subsampling_invalid',
    'independent eligibility observations disagree': 'eligibility_disagreement',
    'frame count, order, timestamp, size or MD5 differs': 'frame_comparison_mismatch',
    'public vector checksum mismatch': 'input_checksum_mismatch',
    'public vector exceeds smoke limit': 'input_size_limit',
    'smoke did not compare pixels': 'missing_pixel_coverage',
    'probe failed': 'oracle_probe_failed',
    'inspect failed': 'native_inspection_failed',
    'native failed': 'native_decode_failed',
    'oracle failed': 'oracle_decode_failed',
    'oracle reported an error': 'oracle_diagnostic_failure',
}


def failure_reason(error):
    if type(error) is ValueError and len(error.args) == 1 and type(error.args[0]) is str:
        return FAILURE_REASONS.get(error.args[0], 'invalid_evidence')
    if isinstance(error, (TimeoutError, subprocess.TimeoutExpired)):
        return 'timeout'
    if isinstance(error, urllib.error.HTTPError):
        return 'http_failure'
    if isinstance(error, (urllib.error.URLError, ConnectionError)):
        return 'transport_failure'
    if isinstance(error, test_oracle.QualificationError):
        return 'oracle_qualification_failed'
    return 'unexpected_failure'


def registration():
    manifest = vp9_corpus.read_json(ROOT / 'crates/coreav-vp9/tests/data/vectors.json')
    rows = manifest['vectors']
    selected = [next(row for row in rows if row['name'] == name) for name in VECTORS]
    vp9_corpus.require(len(set(VECTORS)) == 8 and all(sum(row['name'] == name for row in rows) == 1
                                                   for name in VECTORS), 'smoke membership changed')
    return manifest['source'], selected


def fetch(source, row, destination):
    for attempt in range(3):
        try:
            with urllib.request.urlopen(source + row['name'], timeout=30) as response:
                data = response.read(LIMIT + 1)
            vp9_corpus.require(len(data) <= LIMIT, 'public vector exceeds smoke limit')
            vp9_corpus.require(hashlib.sha256(data).hexdigest() == row['sha256'], 'public vector checksum mismatch')
            destination.write_bytes(data)
            return
        except urllib.error.HTTPError as error:
            transient = error.code in (408, 429) or 500 <= error.code <= 599
            error.close()
            if not transient or attempt == 2:
                raise
        except (urllib.error.URLError, TimeoutError, ConnectionError):
            if attempt == 2:
                raise
        time.sleep(2 ** attempt)


def run(args):
    vp9_corpus.require(sys.platform == 'linux', 'smoke comparisons require Linux')
    args.out.mkdir(mode=0o700, parents=True, exist_ok=False)
    report = {'complete': False, 'vectors': []}
    stage = 'registration'
    current = None
    try:
        source, rows = registration()
        stage = 'oracle_qualification'
        oracle = test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
        tools = SimpleNamespace(native=str(args.native), ffmpeg=str(oracle.ffmpeg), ffprobe=str(oracle.ffprobe))
        for row in rows:
            current = row['name']
            stage = 'acquisition'
            path = args.out / row['name']
            fetch(source, row, path)
            try:
                stage = 'comparison'
                private = args.out / 'private'
                private.mkdir(mode=0o700, exist_ok=True)
                result = vp9_corpus.compare({'path': str(path), 'sha256': row['sha256']}, tools, set(), private)
                vp9_corpus.require(result.get('status') == 'agree' and type(result.get('frames')) is int
                                   and result['frames'] > 0, 'smoke did not compare pixels')
                report['vectors'].append({'name': row['name'], 'sha256': row['sha256'], 'frames': result['frames']})
            finally:
                path.unlink(missing_ok=True)
        vp9_corpus.require(tuple(row['name'] for row in report['vectors']) == VECTORS, 'incomplete smoke')
        report['complete'] = True
        return 0
    except Exception as error:
        reason = failure_reason(error)
        report.update(failed_stage=stage, failure_reason=reason)
        if current:
            report['failed_vector'] = current
        print(f'vp9-smoke: stage={stage} reason={reason}', file=sys.stderr)
        raise
    finally:
        if not report['complete']:
            report['failed_stage'] = stage
        vp9_corpus.write_json(args.out / 'smoke.json', report)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native', required=True, type=Path)
    parser.add_argument('--oracle-bin', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    try:
        return run(args)
    except Exception:
        print('vp9-smoke: preparation_or_comparison_failed', file=sys.stderr)
        return 1


if __name__ == '__main__':
    sys.exit(main())
