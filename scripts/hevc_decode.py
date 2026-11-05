#!/usr/bin/env python3
"""Compare complete scalar HEVC output with the existing source-bound census."""

import argparse
import collections
import json
import pathlib
import re
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_conformance as harness
import hevc_intra as intra

INTER_COUNTERS = ('skip_units', 'prediction_units', 'p_units', 'b_units', 'bi_units',
                  'merge_units', 'amvp_lists', 'temporal_units', 'weighted_units',
                  'fractional_luma_units', 'fractional_chroma_units', 'negative_motion_units',
                  'long_term_units', 'list_modification_units')
INTER_ARRAYS = {'weighted_changed_samples': 3, 'partitions': 8, 'boundary_strengths': 3}
HEADERS = ('nals', 'pictures', 'output_flag_pictures', 'output_pictures',
           'independent_slices', 'dependent_slices', 'picture_hashes', 'reserved_hashes')


def require(condition, reason):
    harness.require(condition, reason)


def md5(value):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{32}', value) is not None


def records(path):
    require(0 < path.stat().st_size < harness.CAPTURE_CAP, 'native_capture_size')
    rows = [json.loads(line, object_pairs_hook=harness.unique_object)
            for line in path.read_text().splitlines()]
    require(all(isinstance(row, dict) for row in rows), 'native_record_shape')
    require(rows and rows[-1].get('type') == 'summary' and rows[-1].get('complete') is True,
            'native_complete_summary')
    require(all(row.get('type') == 'picture' for row in rows[:-1]), 'native_record_order')
    return rows[:-1], rows[-1]


def counters(stats, inter, memory, bins):
    require(isinstance(stats, dict) and isinstance(inter, dict)
            and all(harness.integer(stats.get(key)) for key in intra.COUNTERS)
            and all(harness.integer(inter.get(key)) for key in INTER_COUNTERS), 'native_counters')
    for values, fields in [(stats, intra.ARRAY_COUNTERS), (inter, INTER_ARRAYS)]:
        for key, length in fields.items():
            require(isinstance(values.get(key), list) and len(values[key]) == length
                    and all(harness.integer(value) for value in values[key]), 'native_counter_arrays')
    changes = inter.get('boundary_changed_samples')
    require(isinstance(changes, list) and len(changes) == 3
            and all(isinstance(row, list) and len(row) == 3
                    and all(harness.integer(value) for value in row) for row in changes),
            'native_strength_arrays')
    require(stats['ctus'] <= stats['coding_units'] and stats['ctus'] <= stats['bins'] <= bins
            and stats['memory_bytes'] <= memory
            and stats['nonzero_transform_units'] <= stats['transform_units']
            and stats['transform_skips'] <= stats['nonzero_transform_units']
            and stats['pcm_units'] + inter['skip_units'] <= stats['coding_units']
            and stats['nonzero_qp_deltas'] <= stats['qp_deltas']
            and stats['scaling_list_blocks'] <= stats['nonzero_transform_units']
            and sum(stats['dct_blocks']) + stats['dst_blocks'] <= stats['nonzero_transform_units'],
            'native_reconstruction_work')
    prediction = inter['prediction_units']
    require(inter['p_units'] + inter['b_units'] == prediction
            and inter['bi_units'] <= inter['b_units']
            and inter['skip_units'] <= inter['merge_units'] <= prediction
            and prediction - inter['merge_units'] <= inter['amvp_lists'] <= 2 * (prediction - inter['merge_units'])
            and sum(inter['partitions']) <= prediction <= 4 * sum(inter['partitions'])
            and sum(inter['partitions']) <= stats['coding_units']
            and all(inter[key] <= prediction for key in (
                'temporal_units', 'weighted_units', 'fractional_luma_units',
                'fractional_chroma_units', 'negative_motion_units', 'long_term_units',
                'list_modification_units')), 'native_inter_work')
    require(sum(inter['boundary_strengths'][1:]) == stats['deblock_edges'][0]
            and changes[0] == [0, 0, 0] and changes[1][1:] == [0, 0]
            and [sum(row[c] for row in changes) for c in range(3)] == stats['deblock_changed_samples'],
            'native_strength_accounting')


def validate(pictures, summary, expected, outputs, references, header_summary, source_size, published_md5, budgets):
    require(isinstance(summary.get('headers'), dict)
            and all(harness.integer(summary['headers'].get(key))
                    and summary['headers'][key] == header_summary[key] for key in HEADERS),
            'native_header_completion')
    require(summary.get('input_bytes') == source_size
            and summary.get('input_byte_limit') == intra.INPUT_CAP
            and summary.get('max_nal_bytes') == intra.NAL_CAP
            and harness.integer(summary.get('input_capacity'), 1)
            and summary['input_capacity'] <= intra.NAL_CAP + 65536 + 8
            and all(summary.get(key) == value for key, value in budgets.items()), 'native_budget_identity')
    require(harness.integer(summary.get('decoded_pictures')) and harness.integer(summary.get('skipped_pictures'))
            and summary['decoded_pictures'] + summary['skipped_pictures'] == len(expected)
            and summary['decoded_pictures'] >= len(outputs)
            and harness.integer(summary.get('memory_bytes'), 1)
            and summary['memory_bytes'] <= budgets['max_memory_bytes'], 'native_decoded_denominator')
    require(harness.integer(summary.get('output_pictures'), 1)
            and len(pictures) == len(outputs) == len(references) == summary['output_pictures'],
            'native_output_denominator')
    require(md5(published_md5) and summary.get('output_md5') == published_md5
            and harness.integer(summary.get('output_bytes'), 1)
            and summary['output_bytes'] == sum(p['output_bytes'] for p in pictures), 'native_published_whole_output')
    counters(summary['stats'], summary['inter'], budgets['max_memory_bytes'], budgets['max_stream_bins'])
    require((summary['stats']['picture_hashes'], summary['stats']['reserved_hashes'])
            == (header_summary['picture_hashes'], header_summary['reserved_hashes']), 'native_complete_coded_hashes')
    for index, (picture, output, reference) in enumerate(zip(pictures, outputs, references)):
        require(harness.integer(picture.get('picture_index')) and picture['picture_index'] == output['picture_index']
                and type(picture.get('poc')) is int and picture['poc'] == output['poc']
                and picture.get('timestamp') is None, 'native_coded_output_order')
        coded = expected[output['picture_index']]
        require(all(type(picture.get(key)) is int and picture[key] == coded[key] for key in (
            'coded_width', 'coded_height', 'width', 'height', 'bit_depth', 'profile_idc', 'ctb_log2')),
            'native_picture_geometry')
        depth = picture['bit_depth']
        crop = picture.get('conformance_window')
        require(depth in (8, 10) and picture['ctb_log2'] in (4, 5, 6)
                and isinstance(crop, list) and len(crop) == 4
                and all(harness.integer(value) for value in crop)
                and crop == coded['conformance_window']
                and picture['coded_width'] - 2 * (crop[0] + crop[1]) == picture['width']
                and picture['coded_height'] - 2 * (crop[2] + crop[3]) == picture['height'], 'native_crop')
        require(picture.get('packing') == ('yuv420p' if depth == 8 else 'yuv420p10le')
                and reference['output_index'] == index and md5(picture.get('output_md5'))
                and picture['output_md5'] == reference['md5']
                and harness.integer(picture.get('output_bytes'), 1)
                and picture['output_bytes'] == reference['bytes']
                == picture['width'] * picture['height'] * 3 // 2 * (1 if depth == 8 else 2),
                'native_reference_picture')
        components = picture.get('coded_components')
        require(isinstance(components, list) and len(components) == 3, 'native_coded_components')
        for component, values in enumerate(components):
            scale = 1 if component == 0 else 2
            require(values.get('width') == picture['coded_width'] // scale
                    and values.get('height') == picture['coded_height'] // scale
                    and values.get('bytes') == values['width'] * values['height'] * (1 if depth == 8 else 2)
                    and md5(values.get('md5')), 'native_coded_component_layout')
        counters(picture['stats'], picture['inter'], budgets['max_memory_bytes'], budgets['max_picture_bins'])
        require(sum(component['width'] * component['height'] for component in components) * 2
                <= picture['stats']['memory_bytes'] <= summary['memory_bytes'], 'native_live_picture_memory')
        ctb = 1 << picture['ctb_log2']
        require(picture['stats']['ctus'] == ((picture['coded_width'] + ctb - 1) // ctb)
                * ((picture['coded_height'] + ctb - 1) // ctb), 'native_picture_ctus')
    exact = summary['decoded_pictures'] == len(pictures)
    for owner, scalar, arrays in [('stats', intra.COUNTERS, intra.ARRAY_COUNTERS),
                                   ('inter', INTER_COUNTERS, INTER_ARRAYS)]:
        for key in scalar:
            if key == 'memory_bytes':
                continue
            observed = summary[owner][key]
            subtotal = sum(picture[owner][key] for picture in pictures)
            require(observed == subtotal if exact else observed >= subtotal, 'native_aggregate_counters')
        for key, length in arrays.items():
            for index in range(length):
                observed = summary[owner][key][index]
                subtotal = sum(picture[owner][key][index] for picture in pictures)
                require(observed == subtotal if exact else observed >= subtotal, 'native_aggregate_arrays')
    for strength_index in range(3):
        for component in range(3):
            observed = summary['inter']['boundary_changed_samples'][strength_index][component]
            subtotal = sum(picture['inter']['boundary_changed_samples'][strength_index][component] for picture in pictures)
            require(observed == subtotal if exact else observed >= subtotal, 'native_aggregate_strengths')


def streams(args, census):
    require(census.get('complete') is True and census['registration_sha256'] == harness.digest(args.registration)
            and census['parser_sha256'] == harness.digest(args.parser), 'complete_current_census')
    registration = harness.registration(args.registration)
    require(harness.registration_identity(registration) == harness.registration_identity(harness.registration()),
            'registered_archive_membership')
    require(len(census['archives']) == len(registration['archives']), 'archive_denominator')
    result = []
    for case, record in zip(registration['archives'], census['archives']):
        require(record['test'] == case['test'] and record['archive'] == case['archive']
                and record['status'] == 'accounted', 'archive_identity')
        inventory = harness.read_json(args.data / case['test'] / 'inventory.json')
        members = [member for member in inventory['members'] if member['role'] == 'bitstream']
        require(len(members) == len(record['streams']), 'stream_denominator')
        for member, stream in zip(members, record['streams']):
            require(stream['name'] == member['name'] and stream['sha256'] == member['sha256'], 'stream_identity')
            if stream['status'] == 'excluded':
                require(case['test'] in harness.EXCLUSION_IDENTITIES
                        and member['sha256'] == harness.EXCLUSION_IDENTITIES[case['test']]
                        and stream.get('native_headers_complete') is True, 'named_exclusion_identity')
                continue
            if stream['status'] == 'refused':
                require(case.get('expected_refusal') and stream.get('reason') == case['expected_refusal'],
                        'named_scope_refusal')
                continue
            require(stream['status'] == 'headers_checked', 'stream_incomplete')
            result.append(dict(case=case, member=member, record=stream, inventory=inventory,
                               source=args.data / case['test'] / member['path'],
                               directory=args.census.parent / case['test'] / member['sha256']))
    require(result, 'empty_complete_stream_denominator')
    return result


def command(args, source, *, memory=None, stream_bins=None, picture_bins=None):
    return [args.native, source, str(args.max_memory_bytes if memory is None else memory),
            str(args.max_stream_bins if stream_bins is None else stream_bins),
            str(args.max_picture_bins if picture_bins is None else picture_bins)]


def controls(args, stream, directory, expected):
    data = stream['source'].read_bytes()
    cut = intra.entropy_truncation_offset(data, expected[-1])
    truncated = directory / 'truncated.hevc'
    directory.mkdir(parents=True, exist_ok=True)
    truncated.write_bytes(data[:cut])
    results = []
    try:
        for label, source, options in [
            ('truncated_entropy', truncated, {}),
            ('memory_budget', stream['source'], dict(memory=0)),
            ('stream_budget', stream['source'], dict(stream_bins=0)),
            ('picture_budget', stream['source'], dict(picture_bins=0)),
        ]:
            code, stdout, stderr = harness.child(command(args, source, **options), directory, label)
            require(code != 0, 'negative_control_accepted')
            rows = [json.loads(line, object_pairs_hook=harness.unique_object) for line in stdout.read_text().splitlines()]
            require(not any(row.get('type') == 'summary' for row in rows), 'negative_control_complete_summary')
            refusal = harness.read_json(stderr)
            require(refusal.get('kind') == 'refusal' and refusal.get('category')
                    in ('invalid_data', 'unsupported', 'limit'), 'negative_control_refusal')
            if label.endswith('budget'):
                require(refusal['category'] == 'limit', 'budget_control_not_limit')
            results.append(dict(control=label, rejected=True, category=refusal['category']))
        return results
    finally:
        truncated.unlink(missing_ok=True)


def run(args):
    require(sys.platform == 'linux', 'linux_actions_required')
    require(0 < args.max_memory_bytes <= 4 << 30
            and 0 < args.max_picture_bins <= args.max_stream_bins < 1 << 64, 'driver_budgets')
    require(not list(harness.oracle_pin.verify_bin(args.oracle_bin)), 'pinned_oracle_required')
    args.out.mkdir(parents=True, exist_ok=False)
    census = harness.read_json(args.census)
    selected = streams(args, census)
    denominator = [dict(test=item['case']['test'], sha256=item['member']['sha256']) for item in selected]
    if args.select_test:
        require(set(args.select_test) <= {item['test'] for item in denominator}, 'diagnostic_test_identity')
        selected = [item for item in selected if item['case']['test'] in args.select_test]
    identities = dict(registration=harness.digest(args.registration), census=harness.digest(args.census),
                      parser=harness.digest(args.parser), native=harness.digest(args.native),
                      acquisition=harness.digest(args.data / 'acquisition.json'),
                      oracle={name: harness.digest(args.oracle_bin / name) for name in ('ffmpeg', 'ffprobe')})
    require(all(census['oracle'][name] == value for name, value in identities['oracle'].items()), 'census_oracle_identity')
    budgets = dict(max_memory_bytes=args.max_memory_bytes, max_stream_bins=args.max_stream_bins,
                   max_picture_bins=args.max_picture_bins)
    dispositions = [dict(test=case['test'], sha256=stream['sha256'], status=stream['status'])
                    for case in census['archives'] for stream in case['streams']]
    result = dict(complete=False, scope='diagnostic_subset' if args.select_test else 'complete_c1',
                  identities=identities, budgets=budgets, denominator=denominator, results=[], controls=[])
    result['source_dispositions'] = dispositions
    controlled = set()
    try:
        for ordinal, stream in enumerate(selected):
            entry = dict(test=stream['case']['test'], sha256=stream['member']['sha256'], complete=False, stage='identity')
            result['results'].append(entry)
            try:
                source = stream['source']
                require(source.stat().st_size <= harness.BITSTREAM_CAP
                        and harness.digest(source) == entry['sha256'], 'source_identity')
                header_summary, expected, outputs = harness.native_rows(stream['directory'] / 'native.stdout', source.read_bytes())
                frames = harness.read_json(stream['directory'] / 'frames.stdout')['frames']
                references = stream['record']['reference_frames']
                oracle_source = source
                directory = args.out / 'streams' / str(ordinal)
                if stream['case']['test'] == harness.hevc_oracle_input.STREAM:
                    oracle_source, _ = harness.hevc_oracle_input.prepare(stream['case'], source, directory, harness)
                if not references:
                    references = harness.anchored_reference_frames(args.oracle_bin / 'ffmpeg', oracle_source,
                        directory, frames, outputs, stream['record']['reference_packing'], stream['inventory'],
                        stream['case']['test'], entry['sha256'])
                require(len(references) == len(outputs) and stream['record']['output_order_checked_by_packet_positions'] is True,
                        'reference_output_denominator')
                entry['stage'] = 'native'
                code, stdout, stderr = harness.child(command(args, source), directory, 'native')
                if code != 0:
                    entry['refusal'] = harness.read_json(stderr)
                require(code == 0, 'native_complete_decode_failed')
                pictures, summary = records(stdout)
                entry.update(stage='comparison', summary=summary)
                published = [r for r in stream['inventory']['references'] if r['layout'] == 'whole_output_yuv_file']
                require(len(published) == 1 and published[0]['md5'] == stream['record']['reference_packing']['md5'],
                        'published_whole_output_anchor')
                validate(pictures, summary, expected, outputs, references, header_summary, source.stat().st_size,
                         published[0]['md5'], budgets)
                entry.update(complete=True, stage='complete', pictures=pictures,
                             reference_packing=stream['record']['reference_packing'], references=references,
                             reordered=[row['picture_index'] for row in outputs]
                             != sorted(row['picture_index'] for row in outputs))
                depth = pictures[0]['bit_depth']
                if depth not in controlled:
                    result['controls'].append(dict(bit_depth=depth, test=entry['test'], sha256=entry['sha256'],
                                                   controls=controls(args, stream, directory / 'controls', expected)))
                    controlled.add(depth)
                require(harness.digest(source) == entry['sha256'], 'source_identity_changed')
            except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
                entry.update(complete=False, error_type=type(error).__name__)
                if isinstance(error, ValueError):
                    entry['reason'] = str(error)
            harness.write_json(args.out / 'result.json', result)
        require(harness.digest(args.registration) == identities['registration']
                and harness.digest(args.census) == identities['census']
                and harness.digest(args.parser) == identities['parser']
                and harness.digest(args.native) == identities['native']
                and harness.digest(args.data / 'acquisition.json') == identities['acquisition']
                and all(harness.digest(args.oracle_bin / name) == value for name, value in identities['oracle'].items()),
                'run_identity_changed')
        result['passed'] = bool(result['results']) and all(row['complete'] for row in result['results'])
        result['complete'] = result['passed'] and not args.select_test and len(result['results']) == len(denominator)
        result['output_pictures'] = sum(row.get('summary', {}).get('output_pictures', 0) for row in result['results'])
        coverage = {}
        for entry in result['results']:
            if not entry['complete']:
                continue
            for picture in entry['pictures']:
                depth = str(picture['bit_depth'])
                totals = coverage.setdefault(depth, collections.Counter())
                totals.update({key: picture['inter'][key] for key in INTER_COUNTERS})
                totals.update(output_pictures=1, reordered_stream_pictures=int(entry['reordered']),
                              weighted_changed_samples=sum(picture['inter']['weighted_changed_samples']),
                              strength_one_segments=picture['inter']['boundary_strengths'][1],
                              strength_one_changed_samples=sum(picture['inter']['boundary_changed_samples'][1]),
                              tile_restarts=max(0, picture['stats']['tile_restarts'] - 1),
                              wpp_restarts=picture['stats']['wpp_restarts'],
                              dependent_segments=picture['stats']['dependent_segments'])
        result['matched_output_coverage_by_depth'] = {depth: dict(totals) for depth, totals in coverage.items()}
    finally:
        harness.write_json(args.out / 'result.json', result)
    require(result['passed'], 'complete_stream_comparison_failed')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('registration', 'census', 'data', 'out', 'parser', 'native', 'oracle-bin'):
        parser.add_argument('--' + name, required=True, type=pathlib.Path)
    parser.add_argument('--max-memory-bytes', type=int, default=1 << 30)
    parser.add_argument('--max-stream-bins', type=int, default=1 << 40)
    parser.add_argument('--max-picture-bins', type=int, default=1 << 28)
    parser.add_argument('--select-test', action='append', default=[])
    args = parser.parse_args()
    try:
        result = run(args)
        print(json.dumps(dict(complete=result['complete'], scope=result['scope'], streams=len(result['results']))))
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print('hevc_decode.py: ' + type(error).__name__, file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
