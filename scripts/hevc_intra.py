#!/usr/bin/env python3
"""Run the existing G1 census and compare every source-bound G2 candidate.

Linux Actions only; acquisition and synthetic preparation are separate existing
commands. The native example takes INPUT TARGET_INDEX OUTPUT_U16LE MEMORY BINS.
Its output is coded planar Y/Cb/Cr, little-endian u16 at both supported depths.
"""

import argparse
import collections
import hashlib
import json
import pathlib
import struct
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_conformance as harness
import hevc_fixture as fixture

COUNTERS = ('ctus', 'coding_units', 'transform_units', 'nonzero_transform_units',
            'transform_skips', 'transquant_bypass_units', 'pcm_units', 'dependent_segments',
            'tile_restarts', 'wpp_restarts', 'qp_deltas', 'nonzero_qp_deltas',
            'scaling_list_blocks', 'sign_hidden_groups', 'dst_blocks', 'bins',
            'picture_hashes', 'reserved_hashes', 'memory_bytes')
ARRAY_COUNTERS = {'luma_modes': 35, 'chroma_modes': 35, 'dct_blocks': 4}
SERIAL_COUNTERS = {'tiles': 'tile_restarts', 'wpp': 'wpp_restarts',
                   'dependent_segments': 'dependent_segments'}
INPUT_CAP = 256 << 20


def require(condition, reason):
    harness.require(condition, reason)


def selected_hashes(rows, target):
    current, defined, reserved = None, 0, 0
    for row in rows:
        if row.get('kind') != 'nal':
            continue
        if row['nal_type'] < 32 and row.get('first_slice_segment_in_pic_flag'):
            current = row['picture_index']
        if row['nal_type'] == 40 and current == target:
            require(type(row.get('picture_hash')) is bool
                    and harness.integer(row.get('reserved_hashes')), 'suffix_hash_metadata')
            defined += row['picture_hash']
            reserved += row['reserved_hashes']
    return defined, reserved


def stream_targets(source, record, native, directory, test, source_kind):
    require(0 < source.stat().st_size <= harness.BITSTREAM_CAP
            and harness.digest(source) == record['sha256'], 'candidate_source_identity')
    summary, pictures, outputs = harness.native_rows(native, source.read_bytes())
    eligible_coded = {index for index, picture in enumerate(pictures)
                      if picture['all_intra'] and picture['filters_disabled']}
    stored = [row for row in record.get('intra_candidates', []) if row['gate'] == 'G2']
    if not eligible_coded:
        require(not stored, 'unexpected_g2_candidate')
        return None
    require(eligible_coded <= {output['picture_index'] for output in outputs},
            'eligible_coded_output_absent')
    eligible = [(index, output, pictures[output['picture_index']])
                for index, output in enumerate(outputs) if output['picture_index'] in eligible_coded]
    references = record['reference_frames']
    require(len(references) == len(outputs)
            and [r['output_index'] for r in references] == list(range(len(outputs))),
            'candidate_reference_denominator')
    require(record['reference_count']['value'] == len(outputs)
            and record.get('output_order_checked_by_packet_positions') is True,
            'candidate_output_mapping')
    expected = [row for row in harness.coverage(pictures, outputs, references) if row['gate'] == 'G2']
    require(stored == expected and len(stored) == len(eligible), 'candidate_census_disagrees')
    rows = [json.loads(line, object_pairs_hook=harness.unique_object)
            for line in native.read_text().splitlines()]
    frames = harness.read_json(directory / 'frames.stdout')['frames']
    require(len(frames) == len(outputs), 'candidate_oracle_count')
    targets = []
    for candidate, (output_index, output, picture) in zip(stored, eligible):
        require(candidate['picture_index'] == output['picture_index']
                and candidate['reference']['output_index'] == output_index,
                'candidate_coded_output_identity')
        targets.append(dict(candidate=dict(test=test, stream_sha256=record['sha256'],
                                          source_kind=source_kind, **candidate),
                            picture=picture, hashes=selected_hashes(rows, candidate['picture_index'])))
    return dict(source=source, record=record, summary=summary, frames=frames,
                targets=targets, directory=directory, test=test, rows=rows)


def candidates(args, census):
    require(census.get('complete') is True, 'incomplete_g1_census')
    registered = harness.registration(args.registration)
    require(len(census['archives']) == len(registered['archives']), 'archive_denominator')
    streams = []
    for case, result in zip(registered['archives'], census['archives']):
        require(result['test'] == case['test'] and result['archive'] == case['archive']
                and result['status'] == 'accounted', 'archive_identity')
        inventory = harness.read_json(args.data / case['test'] / 'inventory.json')
        members = [member for member in inventory['members'] if member['role'] == 'bitstream']
        require(len(members) == len(result['streams']), 'stream_denominator')
        for member, record in zip(members, result['streams']):
            require(record['name'] == member['name'] and record['sha256'] == member['sha256'],
                    'stream_identity')
            if record['status'] in ('excluded', 'refused'):
                require(not record.get('intra_candidates'), 'refused_candidate')
                continue
            require(record['status'] == 'headers_checked', 'stream_incomplete')
            directory = args.out / 'g1' / case['test'] / member['sha256']
            item = stream_targets(args.data / case['test'] / member['path'], record,
                                  directory / 'native.stdout', directory,
                                  case['test'], 'published_conformance')
            if item:
                streams.append(item)
    synthetic = census['synthetic_census']
    require(synthetic.get('complete') is True, 'synthetic_census_incomplete')
    recipes = fixture.recipes()
    require([row['test'] for row in synthetic['streams']]
            == ['SYNTHETIC_' + row['id'] for row in recipes], 'synthetic_denominator')
    for recipe, record in zip(recipes, synthetic['streams']):
        require(record['status'] == 'headers_checked', 'synthetic_stream_incomplete')
        identity = recipe['id']
        directory = args.out / 'g1' / 'synthetic' / identity
        item = stream_targets(args.synthetic_data / 'run-1' / (identity + '.hevc'), record,
                              directory / 'native.stdout', directory, record['test'], 'synthetic')
        require(item is not None, 'synthetic_g2_absent')
        streams.append(item)
    expected = [row for row in census['intra_candidates'] if row['gate'] == 'G2']
    actual = [target['candidate'] for stream in streams for target in stream['targets']]
    require(actual and actual == expected, 'complete_g2_denominator')
    require({row['bit_depth'] for row in actual} == {8, 10}, 'g2_depth_denominator')
    return streams


def oracle_frames(binary, source, directory, stream):
    return harness.oracle_frames(binary, source, directory, stream['frames'], stream['test'],
                                 stream['record']['reference_frames'])


def validate_metadata(metadata, target, stream, source, memory, bins):
    expected = target['picture']
    require(metadata.get('kind') == 'intra_picture' and metadata.get('complete') is True
            and harness.integer(metadata.get('picture_index'))
            and metadata.get('picture_index') == target['candidate']['picture_index']
            and metadata.get('packing') == 'planar_y_cb_cr_u16le', 'native_picture_identity')
    require(all(type(metadata.get(key)) is int and metadata[key] == expected[key]
                for key in ('coded_width', 'coded_height', 'width', 'height', 'bit_depth', 'profile_idc', 'ctb_log2'))
            and metadata['ctb_log2'] in (4, 5, 6),
            'native_picture_format')
    require(all(harness.integer(metadata.get(key)) for key in
                ('input_bytes', 'nals', 'pictures', 'input_byte_limit', 'max_memory_bytes', 'max_bins'))
            and metadata.get('input_bytes') == source.stat().st_size
            and metadata.get('nals') == stream['summary']['nals']
            and metadata.get('pictures') == stream['summary']['pictures']
            and metadata.get('input_byte_limit') == INPUT_CAP
            and harness.integer(metadata.get('caller_input_capacity_bytes'))
            and source.stat().st_size <= metadata['caller_input_capacity_bytes'] <= INPUT_CAP,
            'native_input_completion')
    require(metadata.get('max_memory_bytes') == memory and metadata.get('max_bins') == bins,
            'native_budget_identity')
    stats = metadata['stats']
    ctb = 1 << metadata['ctb_log2']
    width, height = metadata['coded_width'], metadata['coded_height']
    ctus = ((width + ctb - 1) // ctb) * ((height + ctb - 1) // ctb)
    coded_bytes = (width * height + 2 * ((width + 1) // 2) * ((height + 1) // 2)) * 2
    require(all(harness.integer(stats.get(key)) for key in COUNTERS)
            and stats['ctus'] == ctus <= stats['coding_units']
            and coded_bytes <= stats['memory_bytes'] <= memory and ctus <= stats['bins'] <= bins
            and stats['nonzero_transform_units'] <= stats['transform_units']
            and stats['transform_skips'] <= stats['nonzero_transform_units']
            and stats['transquant_bypass_units'] <= stats['coding_units']
            and stats['pcm_units'] <= stats['coding_units']
            and stats['nonzero_qp_deltas'] <= stats['qp_deltas']
            and stats['scaling_list_blocks'] <= stats['nonzero_transform_units']
            and stats['dst_blocks'] <= stats['nonzero_transform_units'],
            'native_effective_counters')
    require((stats['picture_hashes'], stats['reserved_hashes']) == tuple(target['hashes']),
            'native_suffix_hash_coverage')
    for key, length in ARRAY_COUNTERS.items():
        require(isinstance(stats.get(key), list) and len(stats[key]) == length
                and all(harness.integer(value) for value in stats[key]), 'native_effective_arrays')
    require(sum(stats['dct_blocks']) + stats['dst_blocks'] <= stats['nonzero_transform_units'],
            'native_transform_counters')
    crop = metadata['conformance_window']
    require(isinstance(crop, list) and len(crop) == 4 and all(harness.integer(value) for value in crop)
            and crop == expected['conformance_window']
            and metadata['coded_width'] - 2 * (crop[0] + crop[1]) == metadata['width']
            and metadata['coded_height'] - 2 * (crop[2] + crop[3]) == metadata['height'], 'native_crop')


def packed_picture(raw, metadata):
    width, height, depth = metadata['coded_width'], metadata['coded_height'], metadata['bit_depth']
    require(harness.integer(width, 1) and harness.integer(height, 1) and depth in (8, 10),
            'native_raw_format')
    geometry = [(width, height)] + [((width + 1) // 2, (height + 1) // 2)] * 2
    require(metadata['planes'] == [dict(width=w, height=h) for w, h in geometry], 'native_raw_planes')
    require(harness.integer(metadata['output_bytes'])
            and len(raw) == metadata['output_bytes'] == sum(w * h * 2 for w, h in geometry),
            'native_raw_size')
    crop = metadata['conformance_window']
    require(len(crop) == 4 and all(harness.integer(value) for value in crop)
            and width - 2 * (crop[0] + crop[1]) == metadata['width'] > 0
            and height - 2 * (crop[2] + crop[3]) == metadata['height'] > 0, 'native_raw_crop')
    whole = hashlib.md5()
    coded_hashes, visible_hashes, proof = [], [], []
    offset, visible_bytes = 0, 0
    for component, (w, h) in enumerate(geometry):
        plane = memoryview(raw)[offset:offset + w * h * 2]
        offset += len(plane)
        minimum, maximum, above_255, low_bits = 1 << depth, 0, 0, 0
        for (sample,) in struct.iter_unpack('<H', plane):
            require(sample < 1 << depth, 'native_sample_range')
            minimum, maximum = min(minimum, sample), max(maximum, sample)
            above_255 += sample > 255
            low_bits += bool(sample & 3)
        packed = bytes(plane[::2]) if depth == 8 else plane
        coded_hashes.append(hashlib.md5(packed).hexdigest())
        sample_bytes = 1 if depth == 8 else 2
        scale = 2 if component == 0 else 1
        left, top = crop[0] * scale, crop[2] * scale
        visible_width = metadata['width'] if component == 0 else (metadata['width'] + 1) // 2
        visible_height = metadata['height'] if component == 0 else (metadata['height'] + 1) // 2
        visible = hashlib.md5()
        for y in range(top, top + visible_height):
            row = packed[(y * w + left) * sample_bytes:(y * w + left + visible_width) * sample_bytes]
            require(len(row) == visible_width * sample_bytes, 'native_crop_row')
            visible.update(row)
            whole.update(row)
            visible_bytes += len(row)
        visible_hashes.append(visible.hexdigest())
        proof.append(dict(min=minimum, max=maximum, above_255=above_255, nonzero_low_two_bits=low_bits))
    return dict(bytes=visible_bytes, md5=whole.hexdigest(), coded_component_md5=coded_hashes,
                visible_component_md5=visible_hashes, sample_proof=proof)


def compare_picture(raw, metadata, candidate, oracle):
    result = packed_picture(raw, metadata)
    reference = candidate['reference']
    require(result['bytes'] == reference['bytes'] == oracle['bytes']
            and result['md5'] == reference['md5'] == oracle['md5'], 'native_reference_mismatch')
    if 'coded_component_md5' in reference:
        require(result['coded_component_md5'] == reference['coded_component_md5'], 'coded_component_mismatch')
    return result


def native_command(binary, source, target, destination, memory, bins):
    return [binary, source, str(target), destination, str(memory), str(bins)]


def entropy_truncation_offset(data, picture):
    offset, size, header = picture['offset'], picture['size'], picture['rbsp_header_bytes']
    require(harness.integer(offset) and harness.integer(size, 3) and harness.integer(header, 1)
            and offset + size <= len(data), 'truncation_control_boundary')
    payload = memoryview(data)[offset + 2:offset + size]
    zeros, rbsp_index, first_entropy = 0, 0, None
    for position, byte in enumerate(payload):
        if zeros == 2 and byte == 3:
            require(position + 1 < len(payload) and payload[position + 1] <= 3,
                    'truncation_control_epb')
            zeros = 0
            continue
        require(zeros != 2 or byte >= 3, 'truncation_control_epb')
        if rbsp_index == header:
            first_entropy = offset + 2 + position
        elif rbsp_index == header + 1:
            require(first_entropy is not None, 'truncation_control_entropy')
            return first_entropy + 1
        rbsp_index += 1
        zeros = zeros + 1 if byte == 0 else 0
    raise ValueError('truncation_control_entropy')


def negative_controls(args, stream, directory):
    target = stream['targets'][0]
    index = target['candidate']['picture_index']
    data = stream['source'].read_bytes()
    cut = entropy_truncation_offset(data, target['picture'])
    truncated = directory / 'truncated.hevc'
    truncated.parent.mkdir(parents=True, exist_ok=True)
    truncated.write_bytes(data[:cut])
    controls = [('absent_target', stream['source'], stream['summary']['pictures'], args.max_memory_bytes, args.max_bins),
                ('truncated_entropy', truncated, index, args.max_memory_bytes, args.max_bins),
                ('memory_budget', stream['source'], index, 0, args.max_bins),
                ('bin_budget', stream['source'], index, args.max_memory_bytes, 0)]
    results = []
    try:
        for label, source, selected, memory, bins in controls:
            output = directory / (label + '.u16le')
            code, stdout, stderr = harness.child(native_command(args.native, source, selected, output, memory, bins),
                                               directory, label)
            require(code != 0 and stdout.stat().st_size == 0 and not output.exists(), 'negative_control_accepted')
            refusal = harness.read_json(stderr)
            require(refusal.get('kind') == 'refusal'
                    and refusal.get('category') in ('invalid_data', 'unsupported', 'limit')
                    and isinstance(refusal.get('reason'), str) and refusal['reason'], 'negative_control_refusal')
            if label in ('memory_budget', 'bin_budget'):
                require(refusal['category'] == 'limit', 'budget_control_not_limit')
            results.append(dict(control=label, rejected=True, category=refusal['category']))
        sentinel = directory / 'existing.u16le'
        sentinel.write_bytes(b'owned-by-caller')
        code, stdout, _ = harness.child(native_command(args.native, stream['source'], index, sentinel,
                                       args.max_memory_bytes, args.max_bins), directory, 'no-clobber')
        require(code != 0 and stdout.stat().st_size == 0 and sentinel.read_bytes() == b'owned-by-caller',
                'no_clobber_control')
        results.append(dict(control='no_clobber', rejected=True))
        return results
    finally:
        truncated.unlink(missing_ok=True)


def aggregate(candidates, results):
    require(candidates and [row['candidate'] for row in results] == candidates, 'result_denominator')
    totals = collections.Counter()
    arrays = {key: [0] * length for key, length in ARRAY_COUNTERS.items()}
    for result in results:
        require(result.get('complete') is True, 'incomplete_candidate')
        stats = result['metadata']['stats']
        totals.update({key: stats[key] for key in COUNTERS if key != 'memory_bytes'})
        for key, values in arrays.items():
            arrays[key] = [a + b for a, b in zip(values, stats[key])]
    serial = {}
    for feature, counter in SERIAL_COUNTERS.items():
        signalled = [result for result in results if result['candidate'][feature]]
        serial[feature] = dict(candidates=len(signalled), effective=sum(
            max(0, result['metadata']['stats'][counter] - (1 if feature == 'tiles' else 0))
            for result in signalled))
    gaps = [feature for feature, value in serial.items() if not value['candidates'] or not value['effective']]
    return dict(complete=True, gate='G2', candidate_count=len(candidates), results=results,
                effective_counters=dict(totals), effective_arrays=arrays, serial_coverage=serial,
                coverage_gaps=gaps, serial_effective_coverage_complete=not gaps)


def run(args):
    require(sys.platform == 'linux', 'linux_actions_required')
    require(0 < args.max_memory_bytes <= 4 << 30 and 0 < args.max_bins < 1 << 64, 'driver_budgets')
    args.out.mkdir(parents=True, exist_ok=False)
    identities = dict(registration=harness.digest(args.registration), parser=harness.digest(args.parser),
                      native=harness.digest(args.native), synthetic_registration=harness.digest(fixture.REGISTRATION),
                      acquisition=harness.digest(args.data / 'acquisition.json'))
    oracle_identity = {name: harness.digest(args.oracle_bin / name) for name in ('ffmpeg', 'ffprobe')}
    result = dict(complete=False, gate='G2', stage='g1', identities=identities, results=[])
    try:
        options = argparse.Namespace(**vars(args))
        options.out = args.out / 'g1'
        harness.run(options)
        census = harness.read_json(options.out / 'census.json')
        require(census['registration_sha256'] == identities['registration']
                and census['parser_sha256'] == identities['parser']
                and all(census['oracle'][name] == value for name, value in oracle_identity.items()),
                'fresh_census_identity')
        result['stage'] = 'candidates'
        streams = candidates(args, census)
        denominator = [target['candidate'] for stream in streams for target in stream['targets']]
        result['candidates'] = denominator
        result['census_sha256'] = harness.digest(options.out / 'census.json')
        for ordinal, stream in enumerate(streams):
            result['stage'] = 'reconstruction'
            source = stream['source']
            require(harness.digest(source) == stream['record']['sha256'], 'candidate_source_changed')
            directory = args.out / 'pictures' / str(ordinal)
            oracle_source = source
            if stream['test'] == harness.hevc_oracle_input.STREAM:
                case = next(case for case in harness.registration(args.registration)['archives']
                            if case['test'] == stream['test'])
                directory.mkdir(parents=True, exist_ok=True)
                oracle_source, _ = harness.hevc_oracle_input.prepare(case, source, directory, harness)
            oracle = oracle_frames(args.oracle_bin / 'ffmpeg', oracle_source, directory, stream)
            for target in stream['targets']:
                candidate = target['candidate']
                selected = candidate['picture_index']
                output = directory / (str(selected) + '.u16le')
                code, stdout, _ = harness.child(native_command(args.native, source, selected, output,
                    args.max_memory_bytes, args.max_bins), directory, 'native-' + str(selected))
                require(code == 0 and stdout.stat().st_size < harness.TEXT_CAP, 'native_failed')
                lines = stdout.read_text().splitlines()
                require(len(lines) == 1, 'native_metadata_count')
                metadata = json.loads(lines[0], object_pairs_hook=harness.unique_object)
                validate_metadata(metadata, target, stream, source, args.max_memory_bytes, args.max_bins)
                require(output.stat().st_size < harness.CAPTURE_CAP, 'native_raw_limit')
                comparison = compare_picture(output.read_bytes(), metadata, candidate,
                                             oracle[candidate['reference']['output_index']])
                result['results'].append(dict(complete=True, candidate=candidate, metadata=metadata,
                                              comparison=comparison))
                output.unlink()
            stream['controls'] = negative_controls(args, stream, directory / 'controls')
            require(harness.digest(source) == stream['record']['sha256'], 'candidate_source_changed')
        require(harness.digest(args.registration) == identities['registration']
                and harness.digest(args.parser) == identities['parser']
                and harness.digest(args.native) == identities['native']
                and harness.digest(fixture.REGISTRATION) == identities['synthetic_registration']
                and harness.digest(args.data / 'acquisition.json') == identities['acquisition']
                and all(harness.digest(args.oracle_bin / name) == value for name, value in oracle_identity.items()),
                'run_identity_changed')
        result.update(aggregate(denominator, result['results']), stage='complete',
                      negative_controls=[dict(test=stream['test'], source_sha256=stream['record']['sha256'],
                                              controls=stream['controls']) for stream in streams])
    finally:
        harness.write_json(args.out / 'result.json', result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--registration', required=True, type=pathlib.Path)
    for name in ('data', 'synthetic-data', 'out', 'parser', 'native', 'oracle-bin'):
        parser.add_argument('--' + name, required=True, type=pathlib.Path)
    parser.add_argument('--max-memory-bytes', required=True, type=int)
    parser.add_argument('--max-bins', required=True, type=int)
    args = parser.parse_args()
    try:
        result = run(args)
    except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError):
        print('hevc_intra: incomplete', file=sys.stderr)
        return 1
    print(json.dumps({key: result[key] for key in ('complete', 'candidate_count', 'coverage_gaps')}))
    return 0


if __name__ == '__main__':
    sys.exit(main())
