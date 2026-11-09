#!/usr/bin/env python3
"""Check owned HEVC boundary frames against already qualified scalar receipts."""

import argparse
import json
import pathlib
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_conformance as harness
import hevc_decode as scalar

require = harness.require
COLOR_FIELDS = ('primaries', 'transfer', 'matrix', 'full_range', 'chroma_location')
FRAME_FIELDS = ('width', 'height', 'bit_depth', 'packing', 'output_bytes', 'output_md5')


def valid_color(value):
    return (isinstance(value, dict) and set(value) == set(COLOR_FIELDS)
            and all(value[key] is None or type(value[key]) is int
                    and 0 <= value[key] <= (5 if key == 'chroma_location' else 255)
                    for key in COLOR_FIELDS if key != 'full_range')
            and (value['full_range'] is None or type(value['full_range']) is bool))


def counter(value, maximum=None):
    return harness.integer(value) and (maximum is None or value <= maximum)


def valid_frame(value):
    if not isinstance(value, dict) or value.get('type') != 'picture':
        return False
    clock = value.get('timestamp')
    return ((clock is None or type(clock) is int and -(1 << 63) <= clock < 1 << 63)
            and all(harness.integer(value.get(key), 1) for key in ('width', 'height', 'bit_depth', 'output_bytes'))
            and value['width'] % 2 == value['height'] % 2 == 0 and value['bit_depth'] in (8, 10)
            and value.get('packing') == ('yuv420p' if value['bit_depth'] == 8 else 'yuv420p10le')
            and value['output_bytes'] == value['width'] * value['height'] * 3 // 2
            * (2 if value['bit_depth'] == 10 else 1)
            and isinstance(value.get('output_md5'), str) and len(value['output_md5']) == 32
            and all(byte in '0123456789abcdef' for byte in value['output_md5'])
            and valid_color(value.get('color')))


def annex_b_c5(value):
    require(isinstance(value, dict) and value.get('complete') is True and isinstance(value.get('results'), list)
            and counter(value.get('starts')) and value['starts'] == len(value['results']) >= 1
            and counter(value.get('keyframe_only_matches'), value['starts'])
            and counter(value.get('matched_seek_outputs')), 'boundary_c5_shape')
    for row in value['results']:
        require(isinstance(row, dict) and harness.integer(row.get('start')) and row.get('unit_type') in range(16, 22)
                and counter(row.get('matched_outputs')) and counter(row.get('following_packets'), 4)
                and type(row.get('keyframe_available')) is bool, 'boundary_c5_start')
    require(value['keyframe_only_matches'] == sum(row['keyframe_available'] for row in value['results'])
            and value['matched_seek_outputs'] == sum(row['matched_outputs'] for row in value['results']),
            'boundary_c5_totals')
    return dict(starts=value['starts'], keyframe_only_matches=value['keyframe_only_matches'],
                matched_seek_outputs=value['matched_seek_outputs'])


def thumbnail_c5(value):
    require(isinstance(value, dict) and value.get('complete') is True and isinstance(value.get('results'), list)
            and counter(value.get('candidates'), 3) and value['candidates'] == len(value['results'])
            and counter(value.get('matched')) and counter(value.get('unsafe_refusals'))
            and value['matched'] + value['unsafe_refusals'] == value['candidates']
            and type(value.get('duplicate_pts_checked')) is bool and type(value.get('dishonest_sync_checked')) is bool
            and (value.get('unavailable') is None) == (value['candidates'] > 0)
            and (value['duplicate_pts_checked'] is False or value['matched'] > 0), 'boundary_c5_shape')
    for row in value['results']:
        require(isinstance(row, dict) and harness.integer(row.get('packet')), 'boundary_c5_candidate')
        if row.get('status') == 'matched':
            require(set(row) == {'packet', 'status', 'frame_md5', 'image_md5', 'pts'} and harness.integer(row['pts'])
                    and all(isinstance(row[key], str) and len(row[key]) == 32 for key in ('frame_md5', 'image_md5')),
                    'boundary_c5_candidate')
        else:
            require(row == dict(packet=row['packet'], status='unsafe_start_refused', category='invalid_data'),
                    'boundary_c5_candidate')
    require(value['matched'] == sum(row['status'] == 'matched' for row in value['results']), 'boundary_c5_totals')
    return {key: value[key] for key in ('candidates', 'matched', 'unsafe_refusals',
                                        'duplicate_pts_checked', 'dishonest_sync_checked')}


def compare_frames(path, pictures, summary, *, coded_clock=False):
    require(0 < path.stat().st_size < harness.CAPTURE_CAP, 'boundary_capture_size')
    rows = [json.loads(line, object_pairs_hook=harness.unique_object)
            for line in path.read_text().splitlines()]
    require(rows and all(isinstance(row, dict) for row in rows), 'boundary_record_shape')
    result = rows[-1]
    require(result.get('type') == 'summary' and result.get('complete') is True
            and result.get('boundary') is True and result.get('backend') == 'native',
            'boundary_native_completion')
    frames = rows[:-1]
    require(harness.integer(result.get('output_pictures'), 1)
            and harness.integer(result.get('input_bytes'), 1)
            and harness.integer(result.get('sent_packets'), 1)
            and len(frames) == len(pictures) > 0 and len(frames) == result.get('output_pictures')
            == summary.get('output_pictures'), 'boundary_output_denominator')
    require(all(row.get('type') == 'picture' for row in frames), 'boundary_record_order')
    inherited = result.get('inherited_color')
    require(valid_color(inherited), 'boundary_container_colour')
    for actual, expected in zip(frames, pictures):
        timestamp = expected['picture_index'] if coded_clock else expected['timestamp']
        require(actual.get('timestamp') is None or type(actual.get('timestamp')) is int
                and -(1 << 63) <= actual['timestamp'] < 1 << 63, 'boundary_clock_type')
        require(valid_frame(actual) and actual.get('timestamp') == timestamp
                and all(actual.get(key) == expected[key] for key in FRAME_FIELDS), 'boundary_frame_mismatch')
        declared = expected.get('color_declarations')
        require(valid_color(declared), 'boundary_colour_declarations')
        color = {key: declared[key] if declared[key] is not None else inherited[key] for key in COLOR_FIELDS}
        require(valid_color(actual.get('color')) and actual.get('color') == color, 'boundary_colour_mismatch')
    require(result.get('output_bytes') == sum(row['output_bytes'] for row in frames)
            == summary.get('output_bytes') and result.get('output_md5') == summary.get('output_md5'),
            'boundary_whole_output_mismatch')
    return frames, result


def compare(path, pictures, summary, *, coded_clock=False):
    frames, result = compare_frames(path, pictures, summary, coded_clock=coded_clock)
    c5 = annex_b_c5(result.get('c5')) if coded_clock else thumbnail_c5(result.get('c5'))
    return dict(complete=True, outputs=len(frames), output_bytes=result['output_bytes'],
                input_bytes=result['input_bytes'], sent_packets=result['sent_packets'],
                native_backend=True, colour_checked=True, c5=c5)


def c1(args):
    require(sys.platform == 'linux', 'linux_actions_required')
    raw = harness.read_json(args.scalar_result)
    require(raw.get('complete') is True and raw.get('scope') == 'complete_c1'
            and raw.get('passed') is True, 'complete_scalar_c1_required')
    require(all(raw['identities'][name] == harness.digest(path) for name, path in (
        ('registration', args.registration), ('census', args.census), ('parser', args.parser),
        ('acquisition', args.data / 'acquisition.json'))), 'boundary_c1_receipt_identity')
    selected = scalar.streams(args, harness.read_json(args.census))
    identities = [dict(test=item['case']['test'], sha256=item['member']['sha256']) for item in selected]
    require(identities == raw['denominator'] and len(selected) == len(raw['results']), 'boundary_c1_membership')
    native_sha = harness.digest(args.native)
    result = dict(complete=False, boundary_native_sha256=native_sha,
                  scalar_sha256=harness.digest(args.scalar_result), denominator=identities, results=[])
    args.out.mkdir(parents=True, exist_ok=False)
    try:
        for ordinal, (stream, expected) in enumerate(zip(selected, raw['results'])):
            require(expected.get('complete') is True and stream['member']['sha256'] == expected['sha256']
                    and stream['case']['test'] == expected['test'], 'boundary_c1_scalar_identity')
            source = stream['source']
            require(harness.digest(source) == expected['sha256'], 'boundary_source_identity')
            row = dict(sha256=expected['sha256'], complete=False)
            result['results'].append(row)
            code, output, _ = harness.child([args.native, 'annex-b', source,
                str(raw['budgets']['max_memory_bytes'])], args.out / str(ordinal), 'boundary')
            require(code == 0, 'boundary_c1_decode')
            row.update(compare(output, expected['pictures'], expected['summary'], coded_clock=True))
            require(row['input_bytes'] == source.stat().st_size, 'boundary_c1_input_bytes')
            require(harness.digest(source) == expected['sha256'], 'boundary_source_changed')
        require(harness.digest(args.native) == native_sha
                and harness.digest(args.scalar_result) == result['scalar_sha256'], 'boundary_run_identity_changed')
        result['complete'] = len(result['results']) == len(identities) and all(row['complete'] for row in result['results'])
        result['outputs'] = sum(row['outputs'] for row in result['results'])
        result['c5'] = {key: sum(row['c5'][key] for row in result['results'])
                        for key in ('starts', 'keyframe_only_matches', 'matched_seek_outputs')}
    finally:
        harness.write_json(args.out / 'result.json', result)
    require(result['complete'], 'boundary_c1_incomplete')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('registration', 'census', 'data', 'out', 'parser', 'native', 'scalar-result'):
        parser.add_argument('--' + name, required=True, type=pathlib.Path)
    args = parser.parse_args()
    try:
        result = c1(args)
        print(json.dumps(dict(complete=result['complete'], streams=len(result['results']), outputs=result['outputs'],
                              c5=result['c5'])))
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print('hevc_boundary.py: ' + type(error).__name__, file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
