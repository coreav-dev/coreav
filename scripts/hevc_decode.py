#!/usr/bin/env python3
"""Compare complete scalar HEVC output with the existing source-bound census."""

import argparse
import collections
import json
import pathlib
import re
import subprocess
import sys
from fractions import Fraction

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_conformance as harness
import hevc_intra as intra
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT

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


def hash_dispositions(rows, pictures):
    skipped = []
    for picture in pictures:
        require(type(picture.get('reconstruction_required')) is bool, 'picture_decode_disposition')
        if not picture['reconstruction_required']:
            require(picture['nal_type'] in (8, 9) and picture.get('no_rasl_output') is True,
                    'unexpected_extracted_picture')
            skipped.append(dict(picture_index=picture['picture_index'], poc=picture['poc'],
                                nal_type=picture['nal_type'], reason='suppressed_rasl'))
    indices = {picture['picture_index'] for picture in skipped}
    defined = reserved = 0
    for row in rows:
        if row.get('kind') != 'nal' or row.get('nal_type') != 40:
            continue
        if row.get('picture_hash') or row.get('reserved_hashes', 0):
            require(harness.integer(row.get('picture_index')) and row['picture_index'] < len(pictures),
                    'hash_coded_picture_identity')
        if row.get('picture_index') in indices:
            require(type(row.get('picture_hash')) is bool and harness.integer(row.get('reserved_hashes')),
                    'skipped_hash_disposition')
            defined += int(row['picture_hash'])
            reserved += row['reserved_hashes']
    return dict(pictures=skipped, skipped_picture_hashes=defined, skipped_reserved_hashes=reserved)


def validate(pictures, summary, expected, outputs, references, header_summary, source_size, published_md5, budgets, dispositions):
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
            and summary['skipped_pictures'] == len(dispositions['pictures'])
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
    for skipped, checked, total in [('skipped_picture_hashes', 'picture_hashes', 'picture_hashes'),
                                    ('skipped_reserved_hashes', 'reserved_hashes', 'reserved_hashes')]:
        require(harness.integer(summary.get(skipped)) and summary[skipped] == dispositions[skipped]
                and summary['stats'][checked] + summary[skipped] == header_summary[total],
                'native_complete_coded_hashes')
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
    require(not list(harness.test_oracle.verify_bin(args.oracle_bin)), 'pinned_oracle_required')
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
                header_rows = [json.loads(line, object_pairs_hook=harness.unique_object)
                               for line in (stream['directory'] / 'native.stdout').read_text().splitlines()]
                hash_accounting = hash_dispositions(header_rows, expected)
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
                         published[0]['md5'], budgets, hash_accounting)
                entry.update(complete=True, stage='complete', pictures=pictures,
                             hash_dispositions=hash_accounting,
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


def packet_records(path, oracle_packets, oracle_stream, limit):
    rows = [json.loads(line, object_pairs_hook=harness.unique_object) for line in path.read_text().splitlines()]
    require(rows and all(isinstance(row, dict) for row in rows)
            and rows[0].get('type') == 'stream' and rows[-1].get('type') == 'summary'
            and rows[-1].get('complete') is True, 'packet_complete_records')
    stream, summary = rows[0], rows[-1]
    require(stream.get('container') in ('mp4', 'matroska')
            and harness.integer(stream.get('length_size'), 1) and stream['length_size'] <= 4,
            'packet_stream_shape')
    require(all(harness.integer(stream.get(key)) for key in ('stream_index', 'packet_count', 'sent_packets'))
            and stream.get('max_packet_bytes') == 64 << 20 and stream.get('max_packets') == 1_000_000
            and isinstance(stream.get('time_base'), dict)
            and isinstance(stream['time_base'].get('num'), str)
            and re.fullmatch('[1-9][0-9]{0,19}', stream['time_base']['num'])
            and harness.integer(stream['time_base'].get('den'), 1), 'packet_stream_bounds')
    require(all(row.get('type') in ('packet', 'payload', 'picture', 'nal_padding') for row in rows[1:-1]),
            'packet_record_types')
    count = len(oracle_packets)
    sent = count if limit == 0 else min(count, limit)
    require(count > 0 and stream.get('packet_count') == count and stream.get('sent_packets') == sent
            and stream.get('stream_index') == oracle_stream['index']
            and Fraction(int(stream['time_base']['num']), stream['time_base']['den'])
            == Fraction(oracle_stream['time_base'])
            and stream.get('clock') == 'demux_presentation_ticks_nearest_away', 'packet_clock_identity')
    packets = rows[1:1 + count]
    require(len(packets) == count and all(row.get('type') == 'packet' for row in packets),
            'packet_metadata_order')
    for index, (packet, oracle) in enumerate(zip(packets, oracle_packets)):
        require(type(packet.get('index')) is int and packet['index'] == index
                and type(packet.get('sent')) is bool and packet['sent'] == (index < sent)
                and type(packet.get('pts')) is int and packet['pts'] == int(oracle['pts'])
                and type(packet.get('media_pts')) is int and type(packet.get('presented')) is bool,
                'packet_presentation_clock')
        if stream['container'] == 'mp4':
            require(packet.get('offset') == int(oracle['pos']) and packet.get('size') == int(oracle['size']),
                    'packet_stored_identity')
    cutoff = min((p['pts'] for p in packets[sent:]), default=None)
    require(stream.get('cutoff') == cutoff, 'packet_unsent_cutoff')
    payloads = [row for row in rows[1 + count:-1] if row['type'] == 'payload']
    require(len(payloads) == sent, 'packet_payload_count')
    for index, (payload, oracle) in enumerate(zip(payloads, oracle_packets)):
        key = 'stored_sha256' if stream['container'] == 'mp4' else 'sha256'
        require(type(payload.get('index')) is int and payload['index'] == index
                and isinstance(payload.get(key), str) and re.fullmatch('[0-9a-f]{64}', payload[key])
                and 'SHA256:' + payload[key] == oracle.get('data_hash')
                and harness.integer(payload.get('bytes'), 1), 'packet_payload_identity')
    padding = [row for row in rows[1 + count:-1] if row['type'] == 'nal_padding']
    require(all(harness.integer(row.get('index')) and row['index'] < sent
                and harness.integer(row.get('nal_index')) and harness.integer(row.get('coded_bytes'), 2)
                and harness.integer(row.get('trailing_zero_bytes'), 1)
                and row['coded_bytes'] + row['trailing_zero_bytes'] <= intra.NAL_CAP for row in padding),
            'packet_padding_warning')
    pictures = [row for row in rows[1 + count:-1] if row['type'] == 'picture']
    require(isinstance(summary.get('headers'), dict)
            and all(harness.integer(summary['headers'].get(key)) for key in HEADERS)
            and all(harness.integer(summary.get(key)) for key in ('decoded_pictures', 'skipped_pictures')),
            'packet_summary_counts')
    require(harness.integer(summary.get('output_pictures'))
            and len(pictures) == summary['output_pictures'] == summary['headers']['output_pictures']
            and summary['headers']['pictures'] <= sent
            and summary['decoded_pictures'] + summary['skipped_pictures'] == summary['headers']['pictures']
            and len({p['picture_index'] for p in pictures}) == len(pictures)
            and summary.get('output_bytes') == sum(p['output_bytes'] for p in pictures), 'packet_output_count')
    states = collections.defaultdict(set)
    for packet in packets[:sent]:
        states[packet['pts']].add(packet['presented'])
    require(all(len(values) == 1 for values in states.values()), 'packet_ambiguous_presentation')
    require(all(type(p.get('timestamp')) is int and p['timestamp'] in states for p in pictures),
            'packet_timestamp_preservation')
    selected = [p for p in pictures if True in states[p['timestamp']]
                and (cutoff is None or p['timestamp'] < cutoff)]
    return stream, summary, pictures, selected


def presentation_timestamps(text, time_base):
    matches = re.findall(r'^#tb 0: (\d+/\d+)$', text, re.MULTILINE)
    require(len(matches) == 1 and Fraction(matches[0]) == Fraction(time_base), 'oracle_presentation_time_base')
    times = []
    for line in text.splitlines():
        if not line.strip() or line.startswith('#'):
            continue
        columns = [column.strip() for column in line.split(',')]
        require(len(columns) == 6 and columns[0] == '0'
                and all(re.fullmatch('-?[0-9]+', value) for value in columns[1:4])
                and int(columns[3]) > 0 and columns[4].isdecimal() and md5(columns[5]), 'oracle_presentation_row')
        times.append(int(columns[2]))
    require(times, 'oracle_presentation_empty')
    return times


def presentation_hashes(text, frames, time_base, cutoff=None):
    times = presentation_timestamps(text, time_base)
    if cutoff is not None:
        frames = [frame for frame in frames if int(frame['pts']) < cutoff]
        lines = []
        index = 0
        for line in text.splitlines():
            if not line.strip() or line.startswith('#'):
                lines.append(line)
            else:
                if times[index] < cutoff:
                    lines.append(line)
                index += 1
        text = '\n'.join(lines)
        times = [pts for pts in times if pts < cutoff]
    result = harness.frame_hashes(text, frames)
    require(len(times) == len(result) and all(int(frame['pts']) == pts for frame, pts in zip(frames, times)),
            'oracle_presentation_timestamps')
    return [dict(row, pts=pts, width=frame['width'], height=frame['height'],
                 bit_depth=10 if frame['pix_fmt'] == 'yuv420p10le' else 8)
            for row, pts, frame in zip(result, times, frames)]


def geometry_end(packets, time_base):
    clocks = [int(packet[key]) for packet in packets[:184] for key in ('pts', 'dts')
              if packet.get(key) not in (None, 'N/A')]
    require(clocks, 'presentation_geometry_clock')
    end = max(clocks) * Fraction(time_base) + 1
    scaled = end * 1_000_000
    micros = -(-scaled.numerator // scaled.denominator)
    sign = '-' if micros < 0 else ''
    whole, fractional = divmod(abs(micros), 1_000_000)
    return f'%{sign}{whole}.{fractional:06d}'


def compare_packet_pictures(pictures, oracle, cutoff):
    if cutoff is not None:
        require(all(a['pts'] <= b['pts'] for a, b in zip(oracle, oracle[1:])),
                'oracle_cutoff_requires_ordered_clock')
    expected = [row for row in oracle if cutoff is None or row['pts'] < cutoff]
    require(len(pictures) == len(expected) > 0, 'packet_picture_denominator')
    for native, reference in zip(pictures, expected):
        require(native['timestamp'] == reference['pts']
                and all(native[key] == reference[key] for key in ('width', 'height', 'bit_depth'))
                and native['output_bytes'] == reference['bytes']
                and native['output_md5'] == reference['md5'], 'packet_picture_mismatch')
    return len(expected)


def corpus_decode(item, source, directory, oracle_bin, *, native, parser, max_memory_bytes,
                  max_stream_bins, max_picture_bins, scope_refusal=False):
    require(source.stat().st_size == item['size'] and harness.digest(source) == item['sha256'],
            'packet_source_identity')
    report = dict(complete=False, source_sha256=item['sha256'], source_bytes=item['size'],
                  native_sha256=harness.digest(native), parser_sha256=harness.digest(parser),
                  oracle={name: harness.digest(oracle_bin / name) for name in ('ffmpeg', 'ffprobe')},
                  stage='configuration', results=[])
    directory.mkdir(parents=True, exist_ok=True)
    config = directory / 'configuration-prefix.hevc'
    budgets = dict(max_memory_bytes=max_memory_bytes, max_stream_bins=max_stream_bins,
                   max_picture_bins=max_picture_bins)
    try:
        code, _, _ = harness.child([oracle_bin / 'ffmpeg', '-nostdin', '-v', 'error', *AUDIO_DECODE_INPUT, '-i', source,
            '-map', '0:v:0', '-c:v', 'copy', '-bsf:v', 'hevc_mp4toannexb', '-frames:v', '1',
            '-f', 'hevc', '-y', config], directory, 'packet-configuration')
        require(code == 0 and 0 < config.stat().st_size <= harness.BITSTREAM_CAP, 'packet_configuration')
        report.update(configuration_sha256=harness.digest(config), configuration_bytes=config.stat().st_size)
        code, out, err = harness.child([parser, config], directory, 'packet-headers')
        if code != 0:
            rows = [json.loads(line, object_pairs_hook=harness.unique_object) for line in out.read_text().splitlines()]
            refusal = rows[-1] if rows else {}
            require(scope_refusal and refusal.get('kind') == 'refusal' and refusal.get('category') == 'unsupported'
                    and refusal.get('reason') in ('HEVC profile space', 'HEVC profile', 'HEVC chroma format',
                        'HEVC bit depth', 'unequal component depths', 'HEVC field sequence',
                        'HEVC SPS range extension', 'HEVC PPS range extension', 'HEVC SCC extension'),
                    'packet_unexpected_header_refusal')
            require(harness.digest(source) == item['sha256'] and harness.digest(config) == report['configuration_sha256']
                    and harness.digest(parser) == report['parser_sha256']
                    and all(harness.digest(oracle_bin / name) == sha for name, sha in report['oracle'].items()),
                    'packet_refusal_identity_changed')
            report.update(complete=True, stage='profile_refusal', expected_refusal=refusal)
            return report
        require(code == 0, 'packet_header_admission')
        report['stage'] = 'oracle_packets'
        code, out, _ = harness.child([oracle_bin / 'ffprobe', '-v', 'error', '-select_streams', 'v:0',
            '-show_packets', '-show_streams', '-show_data_hash', 'sha256',
            '-show_entries', 'packet=pts,dts,pos,size,data_hash,flags,duration:stream=index,time_base,codec_name,pix_fmt',
            '-of', 'json=compact=1', source], directory, 'presentation-packets')
        require(code == 0, 'presentation_packet_oracle')
        metadata = harness.read_json(out)
        require(len(metadata.get('streams', [])) == 1 and metadata['streams'][0]['codec_name'] == 'hevc',
                'presentation_selected_stream')
        stream, packets = metadata['streams'][0], metadata['packets']
        require(0 < len(packets) <= 1_000_000, 'presentation_packet_count')
        for limit in ([120, 0] if 'b5' in item['sets'] else [120]):
            arm = dict(packet_limit=limit, complete=False)
            report['results'].append(arm)
            report['stage'] = 'native_packets'
            code, out, err = harness.child([native, source, config, str(stream['index']), str(limit),
                str(max_memory_bytes), str(max_stream_bins), str(max_picture_bins)], directory, f'packet-native-{limit}',
                timeout=2400 if limit == 0 else 600)
            arm['native_exit_code'] = code
            if code != 0 and err.stat().st_size < harness.TEXT_CAP:
                arm['native_refusal'] = harness.read_json(err)
                refusal = arm['native_refusal']
                if (refusal.get('kind') == 'refusal' and refusal.get('category') in ('invalid_data', 'unsupported')
                        and refusal.get('reason') in harness.PACKET_TOOLING_REFUSALS and out.stat().st_size == 0):
                    report.update(stage='follow_up', follow_up=refusal['reason'])
                    break
            require(code == 0, 'packet_native_decode')
            packet_stream, summary, pictures, selected = packet_records(out, packets, stream, limit)
            arm.update(stream=packet_stream, summary=summary, pictures=pictures)
            require(all(summary.get(key) == value for key, value in budgets.items())
                    and harness.integer(summary.get('memory_bytes'), 1)
                    and summary['memory_bytes'] <= max_memory_bytes
                    and all(picture['stats']['memory_bytes'] <= summary['memory_bytes'] for picture in pictures)
                    and summary['input_bytes'] == config.stat().st_size + sum(row['bytes'] for row in (
                        json.loads(line) for line in out.read_text().splitlines()) if row.get('type') == 'payload'),
                    'packet_budget_identity')
            counters(summary['stats'], summary['inter'], max_memory_bytes, max_stream_bins)
            require(summary.get('max_nal_bytes') == intra.NAL_CAP
                    and summary.get('input_byte_limit') == intra.INPUT_CAP
                    and harness.integer(summary.get('input_capacity'), 1)
                    and summary['input_capacity'] <= max(intra.NAL_CAP + 65536 + 8, 64 << 20),
                    'packet_input_capacity')
            for skipped, checked in [('skipped_picture_hashes', 'picture_hashes'),
                                     ('skipped_reserved_hashes', 'reserved_hashes')]:
                require(harness.integer(summary.get(skipped))
                        and summary[skipped] + summary['stats'][checked] == summary['headers'][checked],
                        'packet_hash_accounting')
            for picture in pictures:
                counters(picture['stats'], picture['inter'], max_memory_bytes, max_picture_bins)
            bound = len(packets) + 1 if limit == 0 or len(packets) <= limit else 152
            cutoff = packet_stream['cutoff']
            report['stage'] = 'oracle_frame_geometry'
            code, out, _ = harness.child([oracle_bin / 'ffprobe', '-v', 'error', '-flags', 'unaligned',
                *harness.HEVC_DECODE_INPUT, '-select_streams', 'v:0', '-show_frames',
                *(['-read_intervals', geometry_end(packets, stream['time_base'])] if cutoff is not None else []),
                '-show_entries', 'frame=width,height,pix_fmt,pts', '-of', 'json=compact=1', source],
                directory, f'presentation-geometry-{limit}')
            require(code == 0, 'presentation_frame_geometry')
            frames = harness.read_json(out)['frames']
            compared_frames = [frame for frame in frames if cutoff is None or int(frame['pts']) < cutoff]
            require(0 < len(compared_frames) <= len(packets)
                    and len({frame['pix_fmt'] for frame in compared_frames}) == 1
                    and compared_frames[0]['pix_fmt'] in ('yuv420p', 'yuvj420p', 'yuv420p10le'), 'presentation_frame_packing')
            report['stage'] = 'oracle_samples'
            command = [oracle_bin / 'ffmpeg', '-nostdin', '-v', 'repeat+error', '-copyts',
                '-noautorotate', '-flags', 'unaligned', '-c:v', 'hevc', *harness.HEVC_DECODE_INPUT,
                *AUDIO_DECODE_INPUT, '-i', source, '-map', '0:v:0', '-fps_mode', 'passthrough',
                '-enc_time_base', 'demux', '-noautoscale', '-c:v', 'rawvideo',
                '-pix_fmt', '+' + compared_frames[0]['pix_fmt'], '-frames:v', str(bound),
                '-progress', 'pipe:2', '-f', 'framemd5', '-']
            code, out, err = harness.child(command, directory, f'packet-oracle-{limit}')
            require(code == 0, 'packet_oracle_decode')
            collected = [harness.stderr_line(raw, item['sha256'], 'packet-frames', exit_code=code)
                         for raw in harness.stderr_lines(err)]
            for entry in collected:
                if harness.progress_line(entry):
                    entry['kind'] = 'progress'
            progress = harness.reference_progress(collected)
            times = presentation_timestamps(out.read_text(), stream['time_base'])
            require(len(times) <= bound and int(progress[-1]['frame']) == len(times), 'packet_oracle_completion')
            require(len(times) < bound or cutoff is not None and times[-1] >= cutoff,
                    'packet_oracle_bound_before_cutoff')
            oracle = presentation_hashes(out.read_text(), frames, stream['time_base'], cutoff)
            arm.update(matched_pictures=compare_packet_pictures(selected, oracle, cutoff),
                       oracle_bound=bound, oracle_frames=len(times), geometry_interval=geometry_end(packets, stream['time_base'])
                       if cutoff is not None else None, native_timeout=2400 if limit == 0 else 600, complete=True)
        require(harness.digest(source) == item['sha256'] and harness.digest(config) == report['configuration_sha256']
                and harness.digest(native) == report['native_sha256'] and harness.digest(parser) == report['parser_sha256']
                and all(harness.digest(oracle_bin / name) == sha for name, sha in report['oracle'].items()),
                'packet_run_identity_changed')
        if 'follow_up' not in report:
            report.update(complete=True, stage='complete')
        return report
    except ValueError as error:
        reason = str(error)
        report['failure'] = reason if re.fullmatch('[a-z0-9_]{1,80}', reason) else 'value_error'
        raise
    finally:
        harness.write_json(directory / 'decode.json', report)
        config.unlink(missing_ok=True)


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
