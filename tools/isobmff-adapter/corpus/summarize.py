#!/usr/bin/env python3
"""Summarize a compare.py run for the measurement record.

Usage: summarize.py RUN_DIR
Prints JSON: the profile and level rate, its disagreements by kind, and the
informational bit-rate comparison split by fragmented and non-fragmented
files and by stream kind, plus the declared bit rate coverage. File names
are SHA-256 digests from the corpus manifest; no media content is printed.
"""
import collections
import json
import pathlib
import statistics
import sys


def main():
    run = pathlib.Path(sys.argv[1])
    report = json.loads((run / 'report.json').read_text())
    streams = report['levels'][0]
    metadata = streams['metadata']
    kinds = collections.Counter()
    for failure in metadata['profile_level']['failures']:
        for d in failure['differences']:
            kinds[f"{d['path'].rsplit('/', 1)[1]}: ffprobe {d['reference']!r}, CoreAV {d['candidate']!r}"] += 1
    groups = collections.defaultdict(list)
    declared = collections.Counter()
    for item in metadata['bit_rate']['streams_detail']:
        candidate = json.loads((run / 'candidate' / f"{item['file']}.streams.json").read_text())
        stream = next(s for s in candidate['streams'] if s['index'] == item['stream'])
        fragmented = 'fragmented' if candidate['properties']['fragmented'] else 'regular'
        if item['reference'] > 0:
            ratio = abs(item['candidate'] - item['reference']) / item['reference']
            groups[(fragmented, stream['kind'])].append(ratio)
            groups[(fragmented, 'all')].append(ratio)
    for path in (run / 'candidate').glob('*.json'):
        document = json.loads(path.read_text())
        for stream in document.get('streams', []):
            if stream['kind'] in ('video', 'audio'):
                source = (stream['properties'].get('declared_bit_rate') or {}).get('source')
                declared[source or 'none'] += 1
    table = {}
    for (fragmented, kind), ratios in sorted(groups.items()):
        table[f'{fragmented}/{kind}'] = {
            'streams': len(ratios),
            'exact': sum(r == 0 for r in ratios),
            'within_0_1_percent': sum(r <= 0.001 for r in ratios),
            'within_1_percent': sum(r <= 0.01 for r in ratios),
            'median_relative_difference': statistics.median(ratios),
        }
    print(json.dumps({
        'files': len(streams['details']),
        'profile_level': {k: v for k, v in metadata['profile_level'].items() if k != 'failures'},
        'profile_level_disagreements': dict(kinds),
        'bit_rate': table,
        'declared_bit_rate_sources': dict(declared),
    }, indent=1))


if __name__ == '__main__':
    main()
