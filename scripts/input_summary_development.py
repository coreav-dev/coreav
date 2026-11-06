#!/usr/bin/env python3
"""Compare native input summaries on the explicit development partition."""

import argparse
import collections
import concurrent.futures
import hashlib
import http.client
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools' / 'compat-corpus'))
import round_files
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_pin
import test_oracle


def select(manifests):
    selected = []
    extensions = {
        'isobmff': {'mp4', 'mov', 'm4a', 'm4b', 'm4v', '3gp', '3g2', 'mj2'},
        'matroska': {'mkv', 'webm', 'mka'},
        'mpegts': {'ts', 'mts', 'm2ts', 'm2t'},
    }
    for name in ('v1', 'v1-ts-audio'):
        for row in manifests[name]:
            if row.get('holdout') is not False or row.get('set') != 'real':
                continue
            family = next((key for key, values in extensions.items() if row['ext'] in values), None)
            if family is None or (name == 'v1-ts-audio') != (family == 'mpegts'):
                continue
            selected.append({
                'family': family, 'sha256': row['sha256'], 'ext': row['ext'],
                'size': row['size'],
                'key': row['key'] if row['key'].startswith('corpus/') else f"corpus/{name}/{row['key']}",
            })
    identities = [row['sha256'] for row in selected]
    if len(identities) != len(set(identities)):
        raise ValueError('duplicate-development-identity')
    return sorted(selected, key=lambda row: (row['family'], row['sha256']))


def execute(args, strict=False):
    env = dict(os.environ)
    if strict:
        env['COREAV_STRICT_SAFE'] = '1'
    else:
        env.pop('COREAV_STRICT_SAFE', None)
    env.pop('COREAV_FALLBACK_LOG', None)
    return subprocess.run(args, stdin=subprocess.DEVNULL, capture_output=True, env=env, timeout=30)


def failure_reason(error):
    if isinstance(error, (subprocess.TimeoutExpired, TimeoutError)):
        return 'timeout'
    if isinstance(error, http.client.HTTPException):
        return 'http-error'
    if isinstance(error, OSError):
        return 'io-error'
    if isinstance(error, SystemExit):
        return 'validation'
    if isinstance(error, RuntimeError):
        return 'operation-error'
    return 'unexpected-error'


class CaptureFailure(Exception):
    def __init__(self, stage, reason, details):
        super().__init__(stage, reason)
        self.stage, self.reason, self.details = stage, reason, details


def capture_record(tool, arguments, results):
    record = {'tool': tool, 'arguments': arguments[:-1]}
    for name, result in results.items():
        record[f'{name}_status'] = result.returncode
        record[f'{name}_stdout'] = result.stdout.decode('utf-8', errors='replace')
        record[f'{name}_stderr'] = result.stderr.decode('utf-8', errors='replace')
    return record


def compare(coreav, oracle, path, stop=None):
    outcomes = []
    details = []
    calls = []
    for tool, arguments, status in (
        ('ffprobe', [str(path)], 0),
        ('ffprobe', ['-i', str(path)], 0),
        ('ffmpeg', ['-i', str(path)], 1),
    ):
        results = {}
        for stage, command, strict in (
            ('reference', [str(getattr(oracle, tool)), '-hide_banner', *arguments], False),
            ('candidate', [str(coreav), tool, *arguments], True),
        ):
            if stop is not None and stop.is_set():
                partial = capture_record(tool, arguments, results)
                partial['failure'] = {'stage': 'capture', 'reason': 'cancelled'}
                raise CaptureFailure('capture', 'cancelled', [*calls, partial])
            try:
                results[stage] = execute(command, strict=strict)
            except (Exception, SystemExit) as error:
                fault = {'stage': stage, 'reason': failure_reason(error)}
                partial = capture_record(tool, arguments, results)
                partial['failure'] = fault
                if isinstance(error, subprocess.TimeoutExpired):
                    for name in ('stdout', 'stderr'):
                        value = getattr(error, name, None)
                        if isinstance(value, (str, bytes)):
                            partial[f'{stage}_{name}'] = value.decode('utf-8', errors='replace') if isinstance(value, bytes) else value
                raise CaptureFailure(stage, fault['reason'], [*calls, partial]) from None
        reference, candidate = results['reference'], results['candidate']
        prefix = reference.stderr.find(b'Input #0, ')
        if reference.returncode != status or reference.stdout or prefix < 0:
            outcomes.append('oracle-failure')
        elif candidate.returncode == 4:
            outcomes.append('unsupported' if not candidate.stdout and b'Input #' not in candidate.stderr else 'partial-refusal')
        elif candidate.returncode != status:
            outcomes.append('status-mismatch')
        elif candidate.stdout:
            outcomes.append('unexpected-stdout')
        elif candidate.stderr != reference.stderr[prefix:]:
            outcomes.append('summary-mismatch')
        else:
            outcomes.append('identical')
        record = capture_record(tool, arguments, results)
        record['outcome'] = outcomes[-1]
        calls.append(record)
        if outcomes[-1] != 'identical':
            details.append(record)
    if len(set(outcomes)) == 1 and outcomes[0] in ('identical', 'unsupported'):
        return outcomes[0], details
    return 'mismatch', details


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', type=Path, required=True)
    parser.add_argument('--oracle-dir', type=Path, required=True)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--evidence-cert', type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != 'linux':
        raise SystemExit('development-media-requires-linux')
    try:
        args.directory.mkdir(parents=True, exist_ok=True)
        for name in ('summary.json', 'details.p7m'):
            (args.directory / name).unlink(missing_ok=True)
    except OSError as error:
        print(f'input-summary-development: failed at publish {failure_reason(error)}', file=sys.stderr)
        return 2
    counts = collections.Counter()
    details = []
    failures = collections.Counter()
    failed = cancelled = 0
    pool = None
    selected = []
    stage = 'preparation'
    try:
        try:
            oracle = test_oracle.require(args.oracle_dir / 'ffmpeg', args.oracle_dir / 'ffprobe')
        except test_oracle.QualificationError:
            raise CaptureFailure('oracle-pin', 'verification', []) from None
        selected = select({name: round_files.manifest(name, None) for name in ('v1', 'v1-ts-audio')})
        if not selected:
            raise CaptureFailure('selection', 'empty', [])
        stage = 'fetch'
        pool = round_files.pool()
        stage = 'capture'
        with tempfile.TemporaryDirectory(prefix='summary-development-') as directory:
            destination = Path(directory)
            stop = threading.Event()

            def check(row):
                path = destination / f"{row['sha256']}.{row['ext']}"
                outcome, evidence, fault = 'cancelled', [], None
                stage = 'fetch'
                try:
                    if not stop.is_set():
                        path = round_files.fetch_one(pool, row, destination)
                        stage = 'capture'
                        if not stop.is_set():
                            outcome, evidence = compare(args.coreav.resolve(), oracle, path, stop=stop)
                except CaptureFailure as error:
                    evidence = error.details
                    if error.reason != 'cancelled':
                        outcome = 'failed'
                        fault = {'stage': error.stage, 'reason': error.reason}
                        stop.set()
                except (Exception, SystemExit) as error:
                    outcome = 'failed'
                    fault = {'stage': stage, 'reason': failure_reason(error)}
                    stop.set()
                finally:
                    for target in (path, path.with_name(path.name + '.part')):
                        try:
                            target.unlink(missing_ok=True)
                        except OSError as error:
                            cleanup = {'stage': 'cleanup', 'reason': failure_reason(error)}
                            evidence.append({'failure': cleanup})
                            if fault is None:
                                outcome, fault = 'failed', cleanup
                            stop.set()
                return row['family'], outcome, {'identity': row['sha256'], 'details': evidence, 'failure': fault}

            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as workers:
                remaining = iter(selected)
                pending = {}
                while True:
                    while not stop.is_set() and len(pending) < 4:
                        row = next(remaining, None)
                        if row is None:
                            break
                        pending[workers.submit(check, row)] = row
                    if not pending:
                        break
                    ready, _ = concurrent.futures.wait(pending, return_when=concurrent.futures.FIRST_COMPLETED)
                    for future in ready:
                        row = pending.pop(future)
                        if future.cancelled():
                            cancelled += 1
                            continue
                        family, outcome, evidence = future.result()
                        if outcome == 'failed':
                            failed += 1
                            fault = evidence['failure']
                            failures[(fault['stage'], fault['reason'])] += 1
                        elif outcome == 'cancelled':
                            cancelled += 1
                        else:
                            counts[(family, outcome)] += 1
                            processed = sum(counts.values())
                            if processed % 100 == 0:
                                print(f'Completed {processed} development inputs', flush=True)
                        if evidence['details'] or outcome in ('failed', 'cancelled'):
                            details.append(evidence)
                    if stop.is_set():
                        for future in pending:
                            future.cancel()
    except CaptureFailure as error:
        fault = {'stage': error.stage, 'reason': error.reason}
        failures[(fault['stage'], fault['reason'])] += 1
        details.append({'failure': fault, 'details': error.details})
    except (Exception, SystemExit) as error:
        fault = {'stage': stage, 'reason': failure_reason(error)}
        failures[(fault['stage'], fault['reason'])] += 1
        details.append({'failure': fault})
    finally:
        if pool is not None:
            try:
                pool.close()
            except (Exception, SystemExit) as error:
                failures[('cleanup', failure_reason(error))] += 1
    report = {
        'schema': 'input-summary-development/1',
        'counts': [{'family': family, 'outcome': outcome, 'files': count}
                   for (family, outcome), count in sorted(counts.items())],
        'inputs': len(selected),
        'processed': sum(counts.values()),
        'failed': failed,
        'cancelled': cancelled,
        'unprocessed': len(selected) - sum(counts.values()) - failed - cancelled,
        'complete': False,
        'evidence_encrypted': False,
    }
    encrypted_path = args.directory / 'details.p7m'
    try:
        with tempfile.TemporaryDirectory(prefix='.summary-evidence-', dir=args.directory) as directory:
            plain = Path(directory) / 'details.json'
            plain.write_text(json.dumps({
                'candidate_sha256': hashlib.sha256(args.coreav.read_bytes()).hexdigest(),
                'details': details,
            }, indent=2) + '\n')
            encrypted = Path(directory) / 'details.p7m'
            result = subprocess.run([
                'openssl', 'cms', '-encrypt', '-binary', '-aes-256-cbc', '-outform', 'DER',
                '-in', str(plain), '-out', str(encrypted), str(args.evidence_cert),
            ], capture_output=True, timeout=30)
            if result.returncode:
                failures[('evidence-encryption', 'process-failed')] += 1
            else:
                encrypted.replace(encrypted_path)
                report['evidence_encrypted'] = True
    except (Exception, SystemExit) as error:
        failures[('evidence-encryption', failure_reason(error))] += 1
    report['failures'] = [{'stage': stage, 'reason': reason, 'files': count}
                          for (stage, reason), count in sorted(failures.items())]
    report['complete'] = report['processed'] == report['inputs'] and not failures and report['evidence_encrypted']
    try:
        summary = args.directory / 'summary.json.tmp'
        summary.write_text(json.dumps(report, indent=2) + '\n')
        summary.replace(args.directory / 'summary.json')
    except OSError as error:
        print(f'input-summary-development: failed at publish {failure_reason(error)}', file=sys.stderr)
        return 2
    for stage, reason in failures:
        print(f'input-summary-development: failed at {stage} {reason}', file=sys.stderr)
    print(json.dumps(report, sort_keys=True))
    if not report['complete']:
        return 2
    return 1 if any(outcome == 'mismatch' for _, outcome in counts) else 0


if __name__ == '__main__':
    raise SystemExit(main())
