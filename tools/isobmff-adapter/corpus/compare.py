#!/usr/bin/env python3
"""Compare probe metadata on the harvested corpus.

Normalizes each saved ffprobe capture with `coreav-oracle normalize streams`,
runs `isobmff-adapter streams` on the sparse copy, then runs
`coreav-oracle compare --profile corpus --levels streams`.

Usage: compare.py ORACLE_BIN ADAPTER_BIN HARVEST_DIR OUT_DIR
Writes OUT_DIR/{reference,candidate}/, OUT_DIR/report.json,
OUT_DIR/compare.log and OUT_DIR/summary.json (no media, no file contents).
"""
import json
import os
import subprocess
import sys
import tempfile


def main():
    oracle, adapter, harvest, out = sys.argv[1:5]
    os.makedirs(f'{out}/reference')
    os.makedirs(f'{out}/candidate')
    names = sorted(n[:-5] for n in os.listdir(f'{harvest}/ffprobe') if n.endswith('.json'))
    for name in names:
        with open(f'{harvest}/ffprobe/{name}.json') as handle:
            record = json.load(handle)
        exit_code = record['exit'] if isinstance(record['exit'], int) else 124
        with tempfile.NamedTemporaryFile('w', suffix='.json') as raw:
            raw.write(record['stdout'])
            raw.flush()
            normalized = subprocess.run([oracle, 'normalize', 'streams', '--file', name, '--exit-code',
                                         str(exit_code), '--input', raw.name],
                                        capture_output=True, check=True, text=True).stdout
        with open(f'{out}/reference/{name}.streams.json', 'w') as handle:
            handle.write(normalized)
        try:
            run = subprocess.run([adapter, 'streams', f'{harvest}/sparse/{name}'],
                                 capture_output=True, text=True, timeout=120)
            document = run.stdout if run.returncode == 0 and run.stdout.strip() else None
        except subprocess.TimeoutExpired:
            document = None
        if document is None:
            document = json.dumps({'schema': 'coreav-oracle/streams/1', 'file': name, 'probe_ok': False})
        with open(f'{out}/candidate/{name}.streams.json', 'w') as handle:
            handle.write(document)
    with open(f'{out}/compare.log', 'w') as log:
        status = subprocess.run([oracle, 'compare', '--reference', f'{out}/reference', '--candidate',
                                 f'{out}/candidate', '--levels', 'streams', '--profile', 'corpus',
                                 '--max-diffs', '1000000', '--report', f'{out}/report.json'],
                                stdout=log).returncode
    report = json.load(open(f'{out}/report.json'))
    streams = report['levels'][0]
    metadata = streams['metadata']
    bit_rate = {k: v for k, v in metadata['bit_rate'].items() if k != 'streams_detail'}
    summary = {
        'files': len(names),
        'reference_failures': streams['reference_failures'],
        'candidate_rejections': sum(1 for d in streams['details']
                                    if any(x['path'] == 'probe_ok' for x in d['gating_differences'])),
        'stream_oracle_agreement': [streams['agree'], streams['total']],
        'profile_level': {k: v for k, v in metadata['profile_level'].items() if k != 'failures'},
        'profile_level_failures': metadata['profile_level']['failures'],
        'bit_rate': bit_rate,
        'compare_exit_status': status,
    }
    with open(f'{out}/summary.json', 'w') as handle:
        json.dump(summary, handle, indent=1)
    print(json.dumps({k: v for k, v in summary.items() if k != 'profile_level_failures'}, indent=1))


if __name__ == '__main__':
    main()
