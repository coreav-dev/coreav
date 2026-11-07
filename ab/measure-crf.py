import hashlib
import json
import math
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
sys.path.insert(0, str(ROOT / 'ab'))
import test_oracle
import h264_p_conformance as p
import h264_crf_conformance as crf
import metrics

OUT = Path(os.environ['RUNNER_TEMP']) / 'crf'
WORK, BINS = OUT / 'work', OUT / 'bin'
REPORT = ROOT / 'crf-result.json'
TOOLS = test_oracle.require(BINS / 'ffmpeg', BINS / 'ffprobe')
SIZES = dict(A=(640, 480), B=(368, 800), C=(480, 416), D=(640, 368), E=(640, 368))

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def run(command):
    result = subprocess.run([str(v) for v in command], check=True, capture_output=True, timeout=900)
    if result.stderr:
        raise ValueError('unexpected diagnostics')
    return result.stdout

report = dict(baseline=os.environ['BASELINE'], candidate=os.environ['CANDIDATE'],
              harness=subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True, timeout=30).strip(),
              rustc=subprocess.check_output(['rustc', '-vV'], text=True, timeout=30),
              host=platform.platform(), cpu=Path('/proc/cpuinfo').read_text(),
              binaries={arm: digest(BINS / arm) for arm in ('base', 'cand')},
              oracle=test_oracle.oracle_pin.load(), frames=60, threads=1,
              metric_sha256=digest(ROOT / 'ab/metrics.py'), cells=[], complete=False)
REPORT.write_text(json.dumps(report, indent=2) + '\n')
for label, (w, h) in SIZES.items():
    raw_path = WORK / f'{label}.yuv'
    original = raw_path.read_bytes()
    for value in (18, 23, 28):
        results = {}
        for repeat in range(3):
            arms = ('base', 'cqp', 'crf') if repeat == 0 else ('crf', 'cqp') if repeat == 1 else ('cqp', 'crf')
            for arm in arms:
                stream, recon = OUT / 'out.h264', OUT / 'recon.yuv'
                driver = BINS / ('base' if arm == 'base' else 'cand')
                quality = f'crf:{value}' if arm == 'crf' else value
                command = [driver, w, h, quality, 60, raw_path, stream, recon]
                call = json.loads(run(command))
                if not math.isfinite(call['seconds']) or call['seconds'] <= 0:
                    raise ValueError('invalid timing')
                qps = crf.expected_qps(original, w, h, 60, value, 32) if arm == 'crf' else [value] * 60
                p.inspect(stream.read_bytes(), w, h, value, 60, True, 32, picture_qps=qps)
                bits = 8 * sum(len(n) for n in p.g1.units(stream.read_bytes()))
                if call['nal_bits'] != bits:
                    raise ValueError('NAL accounting differs')
                record = dict(bits=bits, stream_sha256=digest(stream), recon_sha256=digest(recon))
                if arm not in results:
                    decoded = OUT / 'decoded.yuv'
                    run([TOOLS.ffmpeg, '-nostdin', '-v', 'error', '-y', '-threads', '1', '-i', stream,
                         '-map', '0:v:0', '-an', '-sn', '-dn', '-pix_fmt', '+yuv420p', '-fps_mode', 'passthrough', '-f', 'rawvideo', decoded])
                    if decoded.read_bytes() != recon.read_bytes():
                        raise ValueError('oracle reconstruction mismatch')
                    scores = metrics.scores(original, recon.read_bytes(), w, h)
                    weighted = sum(scores[plane]['psnr'] * weight for plane, weight in zip(metrics.PLANES, (6, 1, 1))) / 8
                    if not math.isfinite(weighted):
                        raise ValueError('non-finite PSNR')
                    results[arm] = dict(**record, scores=scores, weighted_psnr=weighted, qps=qps, calls=[])
                if any(results[arm][key] != val for key, val in record.items()):
                    raise ValueError('repeat identity mismatch')
                results[arm]['calls'].append(dict(seconds=call['seconds'], command=[str(v) for v in command]))
        if any(results['base'][key] != results['cqp'][key] for key in ('stream_sha256', 'recon_sha256', 'bits')):
            raise ValueError('fixed-QP baseline changed')
        ratio = statistics.median(call['seconds'] for call in results['crf']['calls']) / statistics.median(call['seconds'] for call in results['cqp']['calls'])
        report['cells'].append(dict(label=label, width=w, height=h, value=value, input_sha256=digest(raw_path), arms=results, time_ratio=ratio))
        REPORT.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
        print(label, value, 'complete', flush=True)
report['complete'] = len(report['cells']) == 15
REPORT.write_text(json.dumps(report, indent=2, allow_nan=False) + '\n')
