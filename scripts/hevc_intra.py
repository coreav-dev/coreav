#!/usr/bin/env python3
"""Run the existing G1 census and compare every source-bound G2 candidate.

Linux Actions only; acquisition and synthetic preparation are separate existing
commands. The native example takes INPUT TARGET_INDEX OUTPUT_U16LE MEMORY BINS.
Its output is coded planar Y/Cb/Cr, little-endian u16 at both supported depths.
"""

import argparse
import collections
import hashlib
import io
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
            'picture_hashes', 'reserved_hashes', 'memory_bytes', 'sao_merges')
ARRAY_COUNTERS = {'luma_modes': 35, 'chroma_modes': 35, 'dct_blocks': 4,
                  'deblock_edges': 3, 'deblock_changed_samples': 3,
                  'sao_blocks': 3, 'sao_changed_samples': 3}
SERIAL_COUNTERS = {'tiles': 'tile_restarts', 'wpp': 'wpp_restarts',
                   'dependent_segments': 'dependent_segments'}
INPUT_CAP = 4 << 30
NAL_CAP = 16 << 20


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


def stream_targets(source, record, native, directory, test, source_kind, filters=False):
    require(0 < source.stat().st_size <= harness.BITSTREAM_CAP
            and harness.digest(source) == record['sha256'], 'candidate_source_identity')
    summary, pictures, outputs = harness.native_rows(native, source.read_bytes())
    eligible_coded = {index for index, picture in enumerate(pictures)
                      if picture['all_intra'] and (filters or picture['filters_disabled'])}
    stored = [row for row in record.get('intra_candidates', []) if filters or row['gate'] == 'G2']
    if not eligible_coded:
        require(not stored, 'unexpected_g2_candidate')
        return None
    nonoutput = eligible_coded - {output['picture_index'] for output in outputs}
    require(not nonoutput or filters and test == 'OPFLAG_B' and nonoutput == {39, 73},
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
    expected = [row for row in harness.coverage(pictures, outputs, references) if filters or row['gate'] == 'G2']
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
    for index in sorted(nonoutput):
        picture = pictures[index]
        hashes = selected_hashes(rows, index)
        require(hashes[0] == 1, 'nonoutput_sei_reference_missing')
        current, reference = None, None
        for row in rows:
            if row.get('kind') != 'nal':
                continue
            if row['nal_type'] < 32 and row.get('first_slice_segment_in_pic_flag'):
                current = row['picture_index']
            if row['nal_type'] == 40 and current == index and row.get('picture_hash'):
                require(reference is None, 'nonoutput_sei_reference_duplicate')
                reference = row.get('picture_hash_reference')
        require(isinstance(reference, dict) and reference.get('algorithm') == 'md5'
                and isinstance(reference.get('components'), list) and len(reference['components']) == 3
                and all(isinstance(c, list) and len(c) == 16
                        and all(harness.integer(v) and v < 256 for v in c)
                        for c in reference['components']), 'nonoutput_sei_reference_invalid')
        candidate = dict(test=test, stream_sha256=record['sha256'], source_kind=source_kind,
                         picture_index=index, bit_depth=picture['bit_depth'], gate='G3',
                         tiles=picture['tiles'], wpp=picture['wpp'],
                         dependent_segments=picture['dependent_segments'],
                         reference=dict(source_scope='coded-picture reference (SEI), not output',
                                        algorithm='md5', coded_component_md5=[bytes(c).hex() for c in reference['components']]))
        targets.append(dict(candidate=candidate, picture=picture, hashes=hashes))
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
                                  case['test'], 'published_conformance', getattr(args, 'filters', False))
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
                              directory / 'native.stdout', directory, record['test'], 'synthetic', getattr(args, 'filters', False))
        require(item is not None, 'synthetic_g2_absent')
        streams.append(item)
    expected = [row for row in census['intra_candidates'] if getattr(args, 'filters', False) or row['gate'] == 'G2']
    actual = [target['candidate'] for stream in streams for target in stream['targets']]
    output_actual = [row for row in actual if row['reference'].get('source_scope') != 'coded-picture reference (SEI), not output']
    require(actual and output_actual == expected, 'complete_g2_denominator')
    if getattr(args, 'filters', False):
        require([(row['test'], row['picture_index']) for row in actual
                 if row['reference'].get('source_scope') == 'coded-picture reference (SEI), not output']
                == [('OPFLAG_B', 39), ('OPFLAG_B', 73)], 'nonoutput_denominator')
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
            and metadata.get('max_nal_bytes') == NAL_CAP
            and harness.integer(metadata.get('caller_input_capacity_bytes'))
            and 0 < metadata['caller_input_capacity_bytes'] <= NAL_CAP + 65536 + 8,
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
    require(stats['sao_merges'] <= ctus, 'native_sao_merge_count')
    for component in range(3):
        scale = 1 if component == 0 else 2
        require(stats['sao_blocks'][component] <= ctus
                and stats['sao_changed_samples'][component] <= (width // scale) * (height // scale)
                and stats['deblock_changed_samples'][component] <= stats['deblock_edges'][component] * (24 if component == 0 else 8),
                'native_filter_counters')
    if expected['filters_disabled']:
        require(not stats['sao_merges'] and not any(stats['deblock_edges']) and not any(stats['sao_blocks']),
                'unfiltered_picture_filter_work')
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
    size = raw.stat().st_size if isinstance(raw, pathlib.Path) else len(raw)
    require(harness.integer(metadata['output_bytes'])
            and size == metadata['output_bytes'] == sum(w * h * 2 for w, h in geometry),
            'native_raw_size')
    crop = metadata['conformance_window']
    require(len(crop) == 4 and all(harness.integer(value) for value in crop)
            and width - 2 * (crop[0] + crop[1]) == metadata['width'] > 0
            and height - 2 * (crop[2] + crop[3]) == metadata['height'] > 0, 'native_raw_crop')
    whole = hashlib.md5()
    coded_hashes, visible_hashes, proof = [], [], []
    visible_bytes = 0
    with raw.open('rb') if isinstance(raw, pathlib.Path) else io.BytesIO(raw) as source:
        for component, (w, h) in enumerate(geometry):
            minimum, maximum, above_255, low_bits = 1 << depth, 0, 0, 0
            sample_bytes = 1 if depth == 8 else 2
            scale = 2 if component == 0 else 1
            left, top = crop[0] * scale, crop[2] * scale
            visible_width = metadata['width'] if component == 0 else (metadata['width'] + 1) // 2
            visible_height = metadata['height'] if component == 0 else (metadata['height'] + 1) // 2
            visible, coded = hashlib.md5(), hashlib.md5()
            for y in range(h):
                row = source.read(w * 2)
                require(len(row) == w * 2, 'native_raw_size')
                for (sample,) in struct.iter_unpack('<H', row):
                    require(sample < 1 << depth, 'native_sample_range')
                    minimum, maximum = min(minimum, sample), max(maximum, sample)
                    above_255 += sample > 255
                    low_bits += bool(sample & 3)
                packed = row[::2] if depth == 8 else row
                coded.update(packed)
                if top <= y < top + visible_height:
                    selected = packed[left * sample_bytes:(left + visible_width) * sample_bytes]
                    require(len(selected) == visible_width * sample_bytes, 'native_crop_row')
                    visible.update(selected)
                    whole.update(selected)
                    visible_bytes += len(selected)
            coded_hashes.append(coded.hexdigest())
            visible_hashes.append(visible.hexdigest())
            proof.append(dict(min=minimum, max=maximum, above_255=above_255, nonzero_low_two_bits=low_bits))
        require(not source.read(1), 'native_raw_size')
    return dict(bytes=visible_bytes, md5=whole.hexdigest(), coded_component_md5=coded_hashes,
                visible_component_md5=visible_hashes, sample_proof=proof)


def compare_picture(raw, metadata, candidate, oracle):
    result = packed_picture(raw, metadata)
    reference = candidate['reference']
    if reference.get('source_scope') == 'coded-picture reference (SEI), not output':
        require('output_index' not in reference and oracle is None, 'nonoutput_mapping_invented')
        require(result['coded_component_md5'] == reference['coded_component_md5'], 'coded_component_mismatch')
        return result
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


def diagnose_picture(binary, source, directory, stream, candidate, native, metadata):
    references = stream['record']['reference_frames']
    total = sum(row['bytes'] for row in references)
    if 'output_index' not in candidate['reference'] or total >= harness.CAPTURE_CAP:
        return dict(available=False, reason='no_bounded_output_reference')
    visible_bytes = sum(((metadata['width'] + scale - 1) // scale)
                        * ((metadata['height'] + scale - 1) // scale) for scale in (1, 2, 2))
    visible_bytes *= 2 if metadata['bit_depth'] == 10 else 1
    if visible_bytes != candidate['reference']['bytes']:
        return dict(available=False, reason='different_output_geometry')
    formats = {frame['pix_fmt'] for frame in stream['frames']}
    require(len(formats) == 1, 'diagnostic_packing')
    command = [binary, '-v', 'repeat+error', '-noautorotate', '-flags', 'unaligned', '-f', 'hevc',
               '-c:v', 'hevc', *harness.HEVC_DECODE_INPUT, '-i', source, '-map', '0:v:0',
               '-fps_mode', 'passthrough', '-noautoscale', '-c:v', 'rawvideo',
               '-pix_fmt', '+' + next(iter(formats)), '-progress', 'pipe:2', '-f', 'rawvideo', '-']
    code, raw, stderr = harness.child(command, directory, 'diagnostic-raw')
    try:
        require(code == 0 and raw.stat().st_size == total, 'diagnostic_oracle_size')
        progress = []
        for line in harness.stderr_lines(stderr):
            item = harness.stderr_line(line, stream['test'], 'diagnostic', exit_code=code)
            if harness.progress_line(item):
                item['kind'] = 'progress'
            progress.append(item)
        require(int(harness.reference_progress(progress)[-1]['frame']) == len(references), 'diagnostic_oracle_count')
        whole, selected = hashlib.md5(), None
        with raw.open('rb') as frames:
            for index, reference in enumerate(references):
                frame = frames.read(reference['bytes'])
                require(hashlib.md5(frame).hexdigest() == reference['md5'], 'diagnostic_frame_reference')
                whole.update(frame)
                if index == candidate['reference']['output_index']:
                    selected = frame
            require(not frames.read(1), 'diagnostic_oracle_size')
        require(whole.hexdigest() == stream['record']['reference_packing']['md5'] and selected is not None,
                'diagnostic_whole_reference')
        changed, first, ctus = [0, 0, 0], [], collections.Counter()
        oracle_offset = 0
        sample_bytes = 1 if metadata['bit_depth'] == 8 else 2
        with native.open('rb') as image:
            for component in range(3):
                scale = 1 if component == 0 else 2
                width, height = metadata['coded_width'] // scale, metadata['coded_height'] // scale
                visible_width = (metadata['width'] + scale - 1) // scale
                visible_height = (metadata['height'] + scale - 1) // scale
                left, top = metadata['conformance_window'][0] * 2 // scale, metadata['conformance_window'][2] * 2 // scale
                for y in range(height):
                    row = image.read(width * 2)
                    require(len(row) == width * 2, 'diagnostic_native_size')
                    if not top <= y < top + visible_height:
                        continue
                    begin = oracle_offset + (y - top) * visible_width * sample_bytes
                    expected = selected[begin:begin + visible_width * sample_bytes]
                    values = ((v,) for v in expected) if sample_bytes == 1 else struct.iter_unpack('<H', expected)
                    for x, ((actual,), (wanted,)) in enumerate(zip(struct.iter_unpack('<H', row[left * 2:(left + visible_width) * 2]), values)):
                        if actual != wanted:
                            changed[component] += 1
                            lx, ly = (x + left) * scale, y * scale
                            ctus[lx >> metadata['ctb_log2'], ly >> metadata['ctb_log2']] += 1
                            if len(first) < 16:
                                first.append(dict(component=component, x=lx, y=ly, delta=actual - wanted))
                oracle_offset += visible_width * visible_height * sample_bytes
            require(not image.read(1), 'diagnostic_native_size')
        return dict(available=True, oracle_complete=True, oracle_whole_md5=whole.hexdigest(),
                    changed_samples=changed, first_differences=first,
                    ctu_differences=[dict(x=x, y=y, samples=n) for (x,y),n in sorted(ctus.items())])
    finally:
        raw.unlink(missing_ok=True)


def run(args):
    require(sys.platform == 'linux', 'linux_actions_required')
    require(0 < args.max_memory_bytes <= 4 << 30 and 0 < args.max_bins < 1 << 64, 'driver_budgets')
    args.out.mkdir(parents=True, exist_ok=False)
    identities = dict(registration=harness.digest(args.registration), parser=harness.digest(args.parser),
                      native=harness.digest(args.native), synthetic_registration=harness.digest(fixture.REGISTRATION),
                      acquisition=harness.digest(args.data / 'acquisition.json'))
    oracle_identity = {name: harness.digest(args.oracle_bin / name) for name in ('ffmpeg', 'ffprobe')}
    result = dict(complete=False, gate='G3' if args.filters else 'G2', stage='g1', identities=identities, results=[])
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
            diagnosed = False
            for target in stream['targets']:
                candidate = target['candidate']
                selected = candidate['picture_index']
                output = directory / (str(selected) + '.u16le')
                entry = dict(complete=False, candidate=candidate, stage='native')
                try:
                    code, stdout, _ = harness.child(native_command(args.native, source, selected, output,
                        args.max_memory_bytes, args.max_bins), directory, 'native-' + str(selected),
                        file_limit=max(harness.CAPTURE_CAP, args.max_memory_bytes))
                    require(code == 0 and stdout.stat().st_size < harness.TEXT_CAP, 'native_failed')
                    lines = stdout.read_text().splitlines()
                    require(len(lines) == 1, 'native_metadata_count')
                    metadata = json.loads(lines[0], object_pairs_hook=harness.unique_object)
                    entry.update(stage='metadata', metadata=metadata)
                    validate_metadata(metadata, target, stream, source, args.max_memory_bytes, args.max_bins)
                    require(output.stat().st_size < args.max_memory_bytes, 'native_raw_limit')
                    reference = candidate['reference']
                    entry['stage'] = 'comparison'
                    entry['comparison'] = compare_picture(output, metadata, candidate,
                        oracle[reference['output_index']] if 'output_index' in reference else None)
                    entry.update(complete=True, stage='complete')
                except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as error:
                    entry['error_type'] = type(error).__name__
                    if entry['stage'] == 'comparison' and not diagnosed:
                        diagnosed = True
                        try:
                            entry['diagnostic'] = diagnose_picture(args.oracle_bin / 'ffmpeg', oracle_source,
                                directory, stream, candidate, output, metadata)
                        except (ValueError, OSError, KeyError, TypeError, subprocess.SubprocessError) as diagnostic_error:
                            entry['diagnostic'] = dict(available=False, error_type=type(diagnostic_error).__name__)
                finally:
                    output.unlink(missing_ok=True)
                    result['results'].append(entry)
            stream['controls'] = negative_controls(args, stream, directory / 'controls')
            require(harness.digest(source) == stream['record']['sha256'], 'candidate_source_changed')
        require(harness.digest(args.registration) == identities['registration']
                and harness.digest(args.parser) == identities['parser']
                and harness.digest(args.native) == identities['native']
                and harness.digest(fixture.REGISTRATION) == identities['synthetic_registration']
                and harness.digest(args.data / 'acquisition.json') == identities['acquisition']
                and all(harness.digest(args.oracle_bin / name) == value for name, value in oracle_identity.items()),
                'run_identity_changed')
        result['stage'] = 'aggregate'
        result['negative_controls'] = [dict(test=stream['test'], source_sha256=stream['record']['sha256'],
                                           controls=stream['controls']) for stream in streams]
        result.update(aggregate(denominator, result['results']), stage='complete')
        if args.filters:
            result['gate'] = 'G3'
            result['coverage_gaps'].append('unproved at 10-bit: tiles/dependent segments, no official candidate')
            result['serial_effective_coverage_complete'] = False
            g2 = [row for row in denominator if row['gate'] == 'G2']
            result['g2_regression'] = aggregate(g2, [row for row in result['results'] if row['candidate']['gate'] == 'G2'])
    finally:
        harness.write_json(args.out / 'result.json', result)
    return result


def development_picture(item, source, directory, oracle_bin, *, native, parser, max_memory_bytes, max_bins):
    require(item['partition'] == 'development' and source.stat().st_size == item['size']
            and harness.digest(source) == item['sha256'], 'development_source_identity')
    require(parser and 0 < max_memory_bytes <= 4 << 30 and 0 < max_bins < 1 << 64, 'development_options')
    directory.mkdir(parents=True, exist_ok=True)
    prefix, output = directory / 'first-irap.hevc', directory / 'first-irap.u16le'
    report = dict(complete=False, stage='extract_first_picture', source_sha256=item['sha256'],
                  source_bytes=item['size'], native_sha256=harness.digest(native), parser_sha256=harness.digest(parser),
                  oracle={name: harness.digest(oracle_bin / name) for name in ('ffmpeg', 'ffprobe')})
    try:
        code, _, _ = harness.child([oracle_bin / 'ffmpeg', '-nostdin', '-v', 'error', '-i', source,
            '-map', '0:v:0', '-c:v', 'copy', '-bsf:v', 'hevc_mp4toannexb', '-frames:v', '1',
            '-f', 'hevc', '-y', prefix], directory, 'extract-first-picture')
        require(code == 0 and 0 < prefix.stat().st_size <= harness.BITSTREAM_CAP, 'development_prefix_failed')
        report.update(stage='headers', prefix_sha256=harness.digest(prefix), prefix_bytes=prefix.stat().st_size)
        code, stdout, _ = harness.child([parser, prefix], directory, 'headers')
        require(code == 0 and stdout.stat().st_size < harness.TEXT_CAP, 'development_headers_failed')
        summary, pictures, outputs = harness.native_rows(stdout, prefix.read_bytes())
        require(len(pictures) == len(outputs) == 1 and outputs[0]['picture_index'] == 0
                and pictures[0]['all_intra'] and 16 <= pictures[0]['nal_type'] <= 23, 'development_first_independent_irap')
        picture = pictures[0]
        rows = [json.loads(line, object_pairs_hook=harness.unique_object) for line in stdout.read_text().splitlines()]
        hashes = selected_hashes(rows, 0)
        report['stage'] = 'oracle'
        frames = harness.probe(oracle_bin / 'ffprobe', prefix, directory, frames=True)['frames']
        require(len(frames) == 1 and frames[0]['pict_type'] == 'I' and frames[0]['key_frame'] == 1,
                'development_oracle_irap')
        oracle = harness.oracle_frames(oracle_bin / 'ffmpeg', prefix, directory, frames, item['sha256'])
        candidate = dict(picture_index=0, gate='G3', source_kind='development_first_independent_irap',
                         stream_sha256=report['prefix_sha256'], source_sha256=item['sha256'],
                         bit_depth=picture['bit_depth'], tiles=picture['tiles'], wpp=picture['wpp'],
                         dependent_segments=picture['dependent_segments'],
                         reference=dict(output_index=0, source_scope='development_first_irap_oracle', **oracle[0]))
        target = dict(candidate=candidate, picture=picture, hashes=hashes)
        stream = dict(source=prefix, summary=summary, rows=rows, targets=[target])
        report.update(stage='reconstruction', candidate=candidate)
        code, stdout, stderr = harness.child(native_command(native, prefix, 0, output, max_memory_bytes, max_bins),
            directory, 'native', file_limit=max(harness.CAPTURE_CAP, max_memory_bytes))
        if code != 0 and stderr.stat().st_size < harness.TEXT_CAP:
            report['native_refusal'] = harness.read_json(stderr)
        require(code == 0 and stdout.stat().st_size < harness.TEXT_CAP, 'development_native_failed')
        metadata = harness.read_json(stdout)
        validate_metadata(metadata, target, stream, prefix, max_memory_bytes, max_bins)
        report.update(stage='comparison', metadata=metadata)
        try:
            report['comparison'] = compare_picture(output, metadata, candidate, oracle[0])
        except ValueError:
            diagnostic_stream = dict(test='development_first_irap', frames=frames,
                                     record=dict(reference_frames=oracle, reference_packing=oracle[0]))
            report['diagnostic'] = diagnose_picture(oracle_bin / 'ffmpeg', prefix, directory,
                                                     diagnostic_stream, candidate, output, metadata)
            report['native_packing'] = packed_picture(output, metadata)
            report['source_first_frame'] = harness.oracle_frames(
                oracle_bin / 'ffmpeg', source, directory,
                [dict(frames[0], width=metadata['width'], height=metadata['height'])],
                item['sha256'], first_picture=True)[0]
            raise
        require(harness.digest(prefix) == report['prefix_sha256'] and harness.digest(source) == item['sha256']
                and harness.digest(native) == report['native_sha256'] and harness.digest(parser) == report['parser_sha256']
                and all(harness.digest(oracle_bin / name) == sha for name, sha in report['oracle'].items()),
                'development_identity_changed')
        report.update(complete=True, stage='complete')
        return report
    finally:
        harness.write_json(directory / 'intra.json', report)
        prefix.unlink(missing_ok=True)
        output.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--registration', required=True, type=pathlib.Path)
    for name in ('data', 'synthetic-data', 'out', 'parser', 'native', 'oracle-bin'):
        parser.add_argument('--' + name, required=True, type=pathlib.Path)
    parser.add_argument('--max-memory-bytes', required=True, type=int)
    parser.add_argument('--max-bins', required=True, type=int)
    parser.add_argument('--filters', action='store_true', help='Include original filtered intra and non-output SEI references')
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
