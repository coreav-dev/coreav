#!/usr/bin/env python3
"""Group the uncovered disagreements of a corpus run by cause
(docs/design/stage3-report.md), with the files and packets each accounts
for, and the codecs of the streams involved.

    causes.py --out DIR        (the --out of corpus.py run)

A file counts under every cause it shows (`sole_cause_files`: the files
that cause alone keeps from agreeing); its uncovered packets count under
the cause of the packet field its comparison names first (the comparison
lists a few examples per file, so a file whose packets differ in two ways
is counted under the first). Counts only and SHA-256 prefixes; never values
of private files.
"""
import argparse
import collections
import json
import pathlib
import re

# Cause of a stream-level field (after `streams/N/` is reduced to `streams/N`).
STREAM_CAUSES = [
    (r'^format_duration_us$', 'format-duration'),
    (r'^streams/N/duration_ts$', 'stream-duration'),
    (r'^streams/N/color_', 'color'),
    (r'^streams/count$', 'stream-count'),
    (r'^probe_ok$', 'rejected-by-coreav'),
]
PACKET_CAUSES = [
    (r'/size$', 'packet-size'),
    (r'/(pts|dts)$', 'packet-timestamps'),
    (r'/keyframe$', 'packet-keyframe'),
    (r'/(extra|extra_run|missing)$', 'packet-count'),
    # A constant-size PCM packet CoreAV's samples do not tile, or samples
    # that cannot be laid out by position (scripts/oracle_kd.py,
    # `pcm_reference_units`).
    (r'/pcm_tiling$', 'pcm-tiling'),
    (r'/pcm_layout$', 'pcm-layout'),
    (r'^streams/count$', 'stream-count'),
    (r'^probe_ok$', 'rejected-by-coreav'),
]


def cause(field, table):
    if field.startswith('unit count'):
        # The overlay could not attribute the packet differences to units
        # (scripts/oracle_kd.py): every packet of the file stays uncovered.
        return 'packets-unattributed'
    reduced = re.sub(r'/\d+', '/N', field)
    found = next((name for pattern, name in table if re.search(pattern, reduced)), None)
    if found:
        return found
    # The other fields of one stream (kind, codec, size, ...) differ together.
    return 'other-stream-fields' if reduced.startswith('streams/N/') else 'other:' + reduced


def codec(out, record, index):
    """The reference codec name of stream `index` (from the kept capture)."""
    path = pathlib.Path(out) / 'ffprobe' / f"{record['sha256']}.{record['ext']}.streams.json"
    if not path.exists():
        return '?'
    streams = json.loads(path.read_text()).get('streams', [])
    match = [s for s in streams if s.get('index') == index]
    return (match[0].get('codec_name') or match[0].get('codec_fourcc') or '?') if match else '?'


def group(out):
    causes = collections.defaultdict(lambda: {'files': set(), 'packets': 0, 'codecs': collections.Counter(),
                                              'containers': collections.Counter()})
    checked = 0
    per_file = collections.defaultdict(set)
    for path in sorted((pathlib.Path(out) / 'files').glob('*.json')):
        record = json.loads(path.read_text())
        if 'levels' not in record:
            continue
        checked += 1
        streams = record['levels'].get('streams', {})
        for item in streams.get('uncovered_items', []):
            for field in item.get('fields', []):
                c = causes[cause(field, STREAM_CAUSES)]
                per_file[record['sha256']].add(cause(field, STREAM_CAUSES))
                if record['sha256'] not in c['files']:
                    c['containers'][record['ext']] += 1
                c['files'].add(record['sha256'])
                m = re.match(r'streams/(\d+)/', field)
                if m:
                    c['codecs'][codec(out, record, int(m.group(1)))] += 1
        for item in record['levels'].get('packets', {}).get('uncovered_items', []):
            first = (item.get('examples') or ['?'])[0]
            c = causes[cause(first, PACKET_CAUSES)]
            per_file[record['sha256']].add(cause(first, PACKET_CAUSES))
            if record['sha256'] not in c['files']:
                c['containers'][record['ext']] += 1
            c['files'].add(record['sha256'])
            c['packets'] += item.get('units', 0)
            m = re.match(r'streams/(\d+)/', first)
            if m:
                c['codecs'][codec(out, record, int(m.group(1)))] += item.get('units', 0)
    # Files that only this cause keeps from agreeing: what fixing it alone gains.
    sole = collections.Counter(next(iter(v)) for v in per_file.values() if len(v) == 1)
    return checked, {name: {'files': len(c['files']), 'sole_cause_files': sole.get(name, 0), 'packets': c['packets'],
                            'codecs': dict(c['codecs'].most_common(8)),
                            'containers': dict(c['containers'].most_common()),
                            'examples': sorted(c['files'])[:5]}
                     for name, c in sorted(causes.items(), key=lambda kv: (-len(kv[1]['files']), -kv[1]['packets']))}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n', 1)[0])
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    checked, causes = group(args.out)
    print(json.dumps({'files_checked': checked, 'causes': causes}, indent=1))


if __name__ == '__main__':
    main()
