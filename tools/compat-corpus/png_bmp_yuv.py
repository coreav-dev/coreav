#!/usr/bin/env python3
"""Explicit png-bmp-yuv-v1 V2 acquisition; prior coverage predicates are unchanged."""
import argparse
import concurrent.futures
import fractions
import hashlib
import json
import math
import os
import pathlib
import platform
import re
import subprocess
import sys
import types
import uuid

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
import decoded_output as decoded
import encode_breadth as eb

VERSION = 'png-bmp-yuv-v1'
REGISTRATION = ROOT / 'docs/design/png-bmp-yuv-v1.md'
MAX_RGB = 2 << 30
MAX_FRAMES = 100000
MAX_JSON = 8 << 20
YUV = frozenset(('yuv420p', 'yuv422p', 'yuv444p', 'yuvj420p', 'yuvj422p', 'yuvj444p'))
FAILURES = frozenset(('receipt_json', 'receipt_binding', 'receipt_frames', 'capture_roster',
                      'capture_payload', 'output_identity', 'source_identity', 'harness_failed',
                      'harness_timeout', 'output_shape', 'reference_shape', 'candidate_shape',
                      'stream_metadata', 'frame_timing', 'image_decode', 'encoder_pixels',
                      'reference_pixels', 'encoder_and_reference_pixels', 'method_identity', 'source_commit', 'host'))
CAPTURE_FAULTS = frozenset(('harness_timeout', 'harness_failed', 'receipt_json', 'receipt_binding',
                           'receipt_frames', 'capture_roster', 'capture_payload', 'output_identity',
                           'source_identity'))


class Invalid(ValueError):
    def __init__(self, reason):
        if reason not in FAILURES:
            raise ValueError('unknown image evidence failure label')
        self.reason = reason
        super().__init__(reason)


def require(condition, reason):
    if not condition:
        raise Invalid(reason)


def integer(value, minimum=0, maximum=None):
    return type(value) is int and value >= minimum and (maximum is None or value <= maximum)


def unique_pairs(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'receipt_json')
        result[key] = value
    return result


def document(path):
    try:
        require(path.is_file() and not path.is_symlink() and path.stat().st_size <= MAX_JSON, 'receipt_json')
        def constant(_):
            raise Invalid('receipt_json')
        return json.loads(path.read_bytes(), object_pairs_hook=unique_pairs, parse_constant=constant)
    except (OSError, UnicodeError, ValueError, RecursionError) as error:
        raise Invalid('receipt_json') from error


def registration():
    return {'registration': VERSION, 'registration_sha256': eb.sha256(REGISTRATION)}


def method_identity():
    names = ('tools/compat-corpus/png_bmp_yuv.py', 'tools/compat-corpus/encode_breadth.py',
             'tools/compat-corpus/decoded_output.py', 'tools/compat-corpus/r3.py',
             'tools/compat-corpus/copy_check.py', 'tools/compat-corpus/release_round.py',
             'tools/compat-corpus/classify.py', 'scripts/transcode_check.py',
             'scripts/opus_encode_check.py', 'scripts/oracle_audio.py',
             'scripts/oracle-audio-args.txt', 'scripts/oracle_pin.py', 'scripts/oracle-ffmpeg.env',
             'docs/design/png-bmp-yuv-v1.md')
    names += tuple(path.relative_to(ROOT).as_posix() for path in (ROOT / 'scripts').glob('composed_audio*.py'))
    return {**{name: eb.sha256(ROOT / name) for name in names}, **eb.v2_fixture_inputs.provenance()}


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()).hexdigest()


def inventory(directory):
    require(directory.is_dir() and not directory.is_symlink(), 'output_identity')
    result = {}
    for path in directory.rglob('*'):
        require(not path.is_symlink(), 'output_identity')
        if path.is_dir():
            continue
        require(path.is_file(), 'output_identity')
        require(path.stat().st_size <= MAX_RGB, 'output_identity')
        result[path.relative_to(directory).as_posix()] = path
    return result


def same_inventory(left, right):
    mine, theirs = inventory(left), inventory(right)
    require(bool(mine) and set(mine) == set(theirs), 'output_identity')
    require(all(decoded.same_files(mine[name], theirs[name]) for name in mine), 'output_identity')
    return mine


def image_names(name, directory):
    require(pathlib.Path(name).name == name and name not in ('', '.', '..', '-'), 'output_shape')
    files = inventory(directory)
    pattern = eb.NUMBER.search(name)
    if pattern is None:
        require(set(files) == {name}, 'output_shape')
        return [name]
    require(len(eb.NUMBER.findall(name)) == 1, 'output_shape')
    prefix, suffix = name[:pattern.start()], name[pattern.end():]
    found = {}
    width = int(pattern.group()[1:-1] or '0')
    for filename in files:
        match = re.fullmatch(re.escape(prefix) + r'([0-9]+)' + re.escape(suffix), filename)
        require(match is not None, 'output_shape')
        index = int(match[1])
        require(index not in found and filename == prefix + str(index).zfill(width) + suffix, 'output_shape')
        found[index] = filename
    require(bool(found) and sorted(found) == list(range(1, len(found) + 1)), 'output_shape')
    return [found[index] for index in sorted(found)]


def validate_build(opts, source_sha):
    build = document(opts.build_manifest)
    require(isinstance(build, dict) and build.get('schema') == 'coreav.image-build/1', 'receipt_binding')
    require(build.get('source_sha') == source_sha and re.fullmatch(r'[0-9a-f]{40}', source_sha) is not None,
            'receipt_binding')
    require(build.get('cargo_lock_sha256') == eb.sha256(ROOT / 'Cargo.lock'), 'receipt_binding')
    require(all(isinstance(build.get(key), str) and build[key].strip() for key in ('toolchain', 'target')),
            'receipt_binding')
    binaries = build.get('binaries')
    require(isinstance(binaries, dict) and set(binaries) == {'coreav', 'harness', 'image_harness'}, 'receipt_binding')
    for name, record in binaries.items():
        require(isinstance(record, dict) and record.get('sha256') == eb.sha256(getattr(opts, name)), 'receipt_binding')
        features = record.get('features')
        require(isinstance(features, list) and all(isinstance(f, str) and f for f in features)
                and len(set(features)) == len(features), 'receipt_binding')
    normal = set(binaries['coreav']['features'])
    require(normal == {'release-full'}, 'receipt_binding')
    require(set(binaries['harness']['features']) == normal, 'receipt_binding')
    require(set(binaries['image_harness']['features']) == normal | {'input-capture'}, 'receipt_binding')
    return build


def validate_source_commit():
    status = subprocess.run(['git', '-C', str(ROOT), 'status', '--porcelain', '--untracked-files=all'],
                            capture_output=True, text=True, timeout=10)
    require(status.returncode == 0 and not status.stdout.strip(), 'source_commit')
    head = subprocess.run(['git', '-C', str(ROOT), 'rev-parse', 'HEAD'],
                          capture_output=True, text=True, timeout=10)
    require(head.returncode == 0 and re.fullmatch(r'[0-9a-f]{40}', head.stdout.strip()) is not None, 'source_commit')
    merged = subprocess.run(['git', '-C', str(ROOT), 'merge-base', '--is-ancestor', 'HEAD', 'origin/main'],
                            capture_output=True, text=True, timeout=10)
    require(merged.returncode == 0, 'source_commit')
    return head.stdout.strip()


def validate_host():
    require(platform.system() == 'Linux' and platform.machine() == 'x86_64', 'host')


def rational(value):
    require(isinstance(value, dict) and set(value) == {'num', 'den'}
            and integer(value['num'], 0, (1 << 127) - 1)
            and integer(value['den'], 1, (1 << 64) - 1), 'receipt_frames')
    return fractions.Fraction(value['num'], value['den'])


def source_timestamps(listing, stream, source_stream):
    try:
        iso = 'mov' in listing.get('format', {}).get('format_name', '').split(',')
        raw = listing.get('image_raw_timeline')
        require(isinstance(raw, dict) if iso else raw is None, 'source_identity')
        time_base = fractions.Fraction(source_stream['time_base'])
        require(time_base > 0, 'source_identity')
        timelines = []
        for probe in (listing, raw) if iso else (listing,):
            sources = [s for s in probe['streams'] if s.get('index') == stream]
            require(len(sources) == 1 and fractions.Fraction(sources[0]['time_base']) == time_base
                    and all(sources[0].get(key) == source_stream.get(key) for key in
                            ('codec_type', 'codec_name', 'pix_fmt', 'width', 'height',
                             'color_range', 'color_space', 'chroma_location')), 'source_identity')
            require(isinstance(probe.get('frames'), list), 'source_identity')
            times, positions = {}, {}
            for frame in probe['frames']:
                require(isinstance(frame, dict), 'source_identity')
                if frame.get('stream_index') != stream:
                    continue
                pts = frame.get('best_effort_timestamp')
                require(type(pts) is int or isinstance(pts, str) and re.fullmatch(r'-?[0-9]+', pts) is not None,
                        'source_identity')
                pts = int(pts)
                require(pts not in times and integer(frame.get('width'), 1) and integer(frame.get('height'), 1),
                        'source_identity')
                times[pts] = (pts * time_base, frame['width'], frame['height'])
                if iso:
                    position = frame.get('pkt_pos')
                    require(type(position) is int or isinstance(position, str)
                            and re.fullmatch(r'[0-9]+', position) is not None, 'source_identity')
                    position = int(position)
                    require(position >= 0 and position not in positions, 'source_identity')
                    positions[position] = pts
            require(times, 'source_identity')
            timelines.append((times, positions))
        if not iso:
            return timelines[0][0]
        edited_times, edited_positions = timelines[0]
        raw_times, raw_positions = timelines[1]
        mapped = {}
        for position, pts in edited_positions.items():
            require(position in raw_positions, 'source_identity')
            raw_pts = raw_positions[position]
            require(edited_times[pts][1:] == raw_times[raw_pts][1:], 'source_identity')
            mapped[raw_pts] = edited_times[pts]
        return mapped
    except (KeyError, ValueError, TypeError, ZeroDivisionError, AttributeError) as error:
        raise Invalid('source_identity') from error


def validate_receipt(directory, request, source, source_listing, stream, output_dir, harness_sha):
    require(directory.is_dir() and not directory.is_symlink(), 'capture_roster')
    receipt = document(directory / 'receipt.json')
    require(isinstance(receipt, dict), 'receipt_binding')
    expected = {**registration(), 'schema': 'coreav.image-input/1', 'call': directory.name,
                'source_sha256': eb.sha256(source), 'input': str(source), 'harness_sha256': harness_sha,
                'command': request['args'], 'cwd': str(output_dir), 'strict_safe': False, 'complete': True,
                'stream': stream}
    require(re.fullmatch(r'[0-9a-f]{32}', directory.name) is not None, 'receipt_binding')
    require(all(receipt.get(key) == value and type(receipt.get(key)) is type(value)
                for key, value in expected.items()), 'receipt_binding')
    require(request == {'args': receipt['command'], 'capture': str(directory)}, 'receipt_binding')
    sources = [s for s in source_listing['streams'] if s.get('index') == stream]
    require(len(sources) == 1 and sources[0].get('codec_type') == 'video'
            and sources[0].get('pix_fmt') in YUV, 'source_identity')
    source_times = source_timestamps(source_listing, stream, sources[0])
    frames = receipt.get('frames')
    require(isinstance(frames, list) and 0 < len(frames) <= MAX_FRAMES, 'receipt_frames')
    expected_files = {'frames.json', 'receipt.json'}
    total, previous, outputs = 0, None, set()
    for ordinal, frame in enumerate(frames):
        require(isinstance(frame, dict) and type(frame.get('ordinal')) is int and frame['ordinal'] == ordinal,
                'receipt_frames')
        require(integer(frame.get('pts'), -(1 << 63), (1 << 63) - 1), 'receipt_frames')
        time = rational(frame.get('presentation_time'))
        require(previous is None or time > previous, 'receipt_frames')
        previous = time
        require(frame.get('source_format') in YUV and frame['source_format'].replace('yuvj', 'yuv')
                == sources[0]['pix_fmt'].replace('yuvj', 'yuv'), 'source_identity')
        color = frame.get('color')
        require(isinstance(color, dict) and set(color) == {'matrix', 'full_range', 'chroma_location'}, 'receipt_frames')
        require(color['matrix'] is None or integer(color['matrix'], 0, 255), 'receipt_frames')
        require(color['full_range'] is None or type(color['full_range']) is bool, 'receipt_frames')
        require(color['chroma_location'] in (None, 'Left', 'Center', 'TopLeft', 'Top', 'BottomLeft', 'Bottom'),
                'receipt_frames')
        range_flag = {'pc': True, 'tv': False}.get(sources[0].get('color_range'))
        if sources[0]['pix_fmt'].startswith('yuvj'):
            range_flag = True
        require(range_flag is None or color['full_range'] is range_flag, 'source_identity')
        matrix = {'gbr': 0, 'bt709': 1, 'fcc': 4, 'bt470bg': 5, 'smpte170m': 6, 'smpte240m': 7,
                  'ycgco': 8, 'bt2020nc': 9, 'bt2020c': 10}.get(sources[0].get('color_space'))
        require(matrix is None or color['matrix'] == matrix, 'source_identity')
        chroma = {'left': 'Left', 'center': 'Center', 'topleft': 'TopLeft', 'top': 'Top',
                  'bottomleft': 'BottomLeft', 'bottom': 'Bottom'}.get(sources[0].get('chroma_location'))
        require(chroma is None or (color['chroma_location'] or 'Left') == chroma, 'source_identity')
        width, height = frame.get('width'), frame.get('height')
        require(integer(width, 1, (1 << 32) - 1) and integer(height, 1, (1 << 32) - 1)
                and frame.get('pix_fmt') == 'rgb24', 'receipt_frames')
        require(frame['pts'] in source_times and source_times[frame['pts']][0] == time, 'source_identity')
        _, input_width, input_height = source_times[frame['pts']]
        require((width, height) in ((input_width, input_height), (input_height, input_width)), 'source_identity')
        size = width * height * 3
        total += size
        require(total <= MAX_RGB and integer(frame.get('rgb_bytes')) and frame['rgb_bytes'] == size,
                'capture_payload')
        filename = f'{ordinal:06d}.rgb'
        require(frame.get('rgb_file') == filename, 'capture_roster')
        expected_files.add(filename)
        path = directory / filename
        require(path.is_file() and not path.is_symlink() and path.stat().st_size == size
                and frame.get('rgb_sha256') == eb.sha256(path), 'capture_payload')
        output = frame.get('output')
        require(isinstance(output, str) and pathlib.Path(output).name == output and output not in ('', '.', '..')
                and output not in outputs, 'capture_roster')
        outputs.add(output)
        encoded = output_dir / output
        require(encoded.is_file() and not encoded.is_symlink()
                and frame.get('output_sha256') == eb.sha256(encoded), 'output_identity')
    require({path.name for path in directory.iterdir()} == expected_files
            and set(inventory(directory)) == expected_files, 'capture_roster')
    require(document(directory / 'frames.json') == frames, 'receipt_frames')
    require(set(inventory(output_dir)) == outputs, 'capture_roster')
    return frames


def difference_record(maxima, different_rgb, total, squared, pixels):
    different = sum(different_rgb)
    count = total // 3
    def psnr(error, samples):
        return 'infinity' if error == 0 else 10 * math.log10(255 ** 2 * samples / error)
    return {'available': True, 'exact': different == 0, 'max_abs_error': maxima,
            'different_samples': different, 'total_samples': total, 'different_pixels': pixels,
            'total_pixels': count, 'different_samples_rgb': different_rgb, 'squared_error_rgb': squared,
            'different_sample_share': different / total, 'different_pixel_share': pixels / count,
            'psnr_db_rgb': [psnr(error, count) for error in squared], 'psnr_db': psnr(sum(squared), total)}


def statistics(left, right, left_bgr=False, right_bgr=False):
    if left.stat().st_size != right.stat().st_size or not left.stat().st_size or left.stat().st_size % 3:
        return {'available': False, 'reason': 'output_shape', 'exact': False}
    maxima, different, squared, samples, pixels = [0, 0, 0], [0, 0, 0], [0, 0, 0], 0, 0
    with left.open('rb') as mine, right.open('rb') as theirs:
        while block := mine.read(3 * 16384):
            other = theirs.read(len(block))
            require(len(other) == len(block), 'capture_payload')
            for offset in range(0, len(block), 3):
                changed = False
                for channel in range(3):
                    a = block[offset + (2 - channel if left_bgr else channel)]
                    b = other[offset + (2 - channel if right_bgr else channel)]
                    error = abs(a - b)
                    maxima[channel] = max(maxima[channel], error)
                    different[channel] += int(error != 0)
                    squared[channel] += error * error
                    changed |= error != 0
                pixels += int(changed)
            samples += len(block)
        require(not theirs.read(1), 'capture_payload')
    return difference_record(maxima, different, samples, squared, pixels)


def aggregate(records):
    if not records or any(not row['available'] for row in records):
        return {'available': False, 'reason': 'output_shape', 'exact': False}
    return difference_record([max(row['max_abs_error'][c] for row in records) for c in range(3)],
                             [sum(row['different_samples_rgb'][c] for row in records) for c in range(3)],
                             sum(row['total_samples'] for row in records),
                             [sum(row['squared_error_rgb'][c] for row in records) for c in range(3)],
                             sum(row['different_pixels'] for row in records))


def image_description(path, ffprobe, env, codec, reference=False):
    reason = 'reference_shape' if reference else 'candidate_shape'
    document = decoded.probe(path, ffprobe, env)
    require(isinstance(document, dict) and len(document.get('streams', [])) == 1, reason)
    stream = document['streams'][0]
    require(stream.get('index') == 0 and stream.get('codec_type') == 'video' and stream.get('codec_name') == codec
            and stream.get('pix_fmt') == ('bgr24' if codec == 'bmp' else 'rgb24')
            and integer(stream.get('width'), 1) and integer(stream.get('height'), 1), reason)
    frames = document.get('frames')
    require(isinstance(frames, list) and len(frames) == 1 and frames[0].get('stream_index') == 0
            and all(frames[0].get(key) == stream[key] for key in ('width', 'height', 'pix_fmt')), reason)
    return document


def timeline(document):
    stream = document['streams'][0]
    try:
        return [(decoded.time({**frame, 'time_base': stream['time_base']}, 'best_effort_timestamp'),
                 decoded.time({**frame, 'time_base': stream['time_base']}, 'duration')) for frame in document['frames']]
    except (KeyError, ValueError, TypeError, ZeroDivisionError) as error:
        raise Invalid('frame_timing') from error


def decode_image(path, document, cx, destination):
    stream = document['streams'][0]
    argv = decoded.video_command(cx.tc.ffmpeg, path, (), 0,
                                 ['-c:v', 'rawvideo', '-pix_fmt', stream['pix_fmt'],
                                  '-fps_mode', 'passthrough', '-f', 'rawvideo'])
    code, _, stderr = decoded.invoke(argv, cx.opts.env, output=destination)
    require(code == 0 and not stderr and destination.is_file()
            and destination.stat().st_size == stream['width'] * stream['height'] * 3, 'image_decode')
    return destination, stream['pix_fmt'] == 'bgr24'


def image_comparisons(cx, frames, directory):
    try:
        names = image_names(cx.entry['output'], cx.reference.cwd)
    except Invalid as error:
        raise Invalid('reference_shape') from error
    require(image_names(cx.entry['output'], cx.run.cwd) == names and [f['output'] for f in frames] == names,
            'output_shape')
    codec, images, failures = cx.jobs[0].kind, [], []
    for frame, name in zip(frames, names):
        reference = image_description(cx.reference.cwd / name, cx.tc.ffprobe, cx.opts.env, codec, True)
        row = {'output': name}
        unavailable = {'available': False, 'exact': False, 'reason': 'candidate_shape'}
        captured = directory / frame['rgb_file']
        try:
            actual = image_description(cx.run.cwd / name, cx.tc.ffprobe, cx.opts.env, codec)
        except Invalid as error:
            failures.append(error.reason)
            actual = None
        if actual is not None:
            if not decoded.metadata(actual['streams'][0], reference['streams'][0]):
                failures.append('stream_metadata')
                mine, theirs = actual['streams'][0], reference['streams'][0]
                row['stream_metadata_fields'] = [key for key in ('codec_type', 'codec_name', *decoded.VIDEO)
                                                  if decoded.value(mine.get(key)) != decoded.value(theirs.get(key))]
                for key in ('start_pts', 'duration_ts'):
                    try:
                        same = decoded.time(mine, key) == decoded.time(theirs, key)
                    except (KeyError, ValueError, TypeError, ZeroDivisionError):
                        same = False
                    if not same:
                        row['stream_metadata_fields'].append(key)
            try:
                same_times = timeline(actual) == timeline(reference)
            except Invalid:
                same_times = False
            if not same_times:
                failures.append('frame_timing')
        for key, description, path in (('encoder', actual, cx.run.cwd / name),
                                        ('reference', reference, cx.reference.cwd / name)):
            row[key] = dict(unavailable)
            if description is None:
                continue
            stream = description['streams'][0]
            if (stream['width'], stream['height']) != (frame['width'], frame['height']):
                row[key]['reason'] = 'output_shape'
                continue
            try:
                decoded_path, bgr = decode_image(path, description, cx, cx.scratch / f'{frame["ordinal"]}-{key}.rgb')
                row[key] = statistics(captured, decoded_path, right_bgr=bgr)
            except Invalid as error:
                row[key]['reason'] = error.reason
                failures.append(error.reason)
        images.append(row)
    encoder, reference = (aggregate([row[key] for row in images]) for key in ('encoder', 'reference'))
    if not encoder['exact'] or not reference['exact']:
        failures.append('encoder_and_reference_pixels' if not encoder['exact'] and not reference['exact']
                        else 'encoder_pixels' if not encoder['exact'] else 'reference_pixels')
    return {'encoder': encoder, 'reference': reference, 'images': images, 'failures': list(dict.fromkeys(failures))}


def eligibility(cx):
    image_jobs = [j for j in cx.jobs if j.kind in ('png', 'bmp')]
    if not image_jobs:
        return None
    source = eb.source_of(cx, image_jobs[0].file)[1]
    listing = decoded.probe(source, cx.tc.ffprobe, cx.opts.env, inputs=('-show_format',)) if source else None
    if listing is None:
        return 'source_identity'
    selected = [s for s in listing['streams'] if s.get('index') == image_jobs[0].stream]
    if len(selected) != 1:
        return 'source_identity'
    if selected[0].get('pix_fmt') not in YUV:
        return None if selected[0].get('pix_fmt') in ('rgb24', 'bgr24', 'gbrp', 'rgba', 'bgra', 'gbrap') else 'reference_shape'
    args = cx.entry['args']
    output_args = args[max(i for i, a in enumerate(args) if a == '-i') + 2:]
    forms = [b for a, b in zip(output_args, output_args[1:]) if a == '-f']
    if (len(cx.jobs) != 1 or image_jobs[0].file != 0 or image_jobs[0].output != 0 or args.count('-i') != 1
            or '-update' in args or forms not in ([], ['image2']) or cx.entry['output'] == '-'
            or cx.entry['output'].startswith(('/dev/', 'pipe:'))):
        return 'reference_shape'
    return source, listing


def image_oracle(cx, source, source_listing):
    run = eb.prepare(cx.opts, cx.entry, cx.base, 'image-harness', cx.assignment)
    directory = cx.base / uuid.uuid4().hex
    request = {'args': run.args, 'capture': str(directory)}
    request_file = cx.base / 'image-request.json'
    request_file.write_text(json.dumps(request))
    source_path = pathlib.Path(run.args[run.args.index('-i') + 1])
    expected_source = eb.sha256(source)
    reference_outputs = {name: eb.sha256(path) for name, path in inventory(cx.reference.cwd).items()}
    staged_sources = [source_path] + [pathlib.Path(prepared.args[prepared.args.index('-i') + 1])
                                     for prepared in (cx.run, cx.reference)]
    require(all(eb.sha256(path) == expected_source for path in staged_sources), 'source_identity')
    if 'mov' in source_listing.get('format', {}).get('format_name', '').split(','):
        source_listing = {**source_listing, 'image_raw_timeline': decoded.probe(
            source_path, cx.tc.ffprobe, cx.opts.env, inputs=('-ignore_editlist', '1'))}
    (cx.base / 'image-source-probe.json').write_text(json.dumps(source_listing))
    env = {**cx.opts.env, 'COREAV_IMAGE_REQUEST': str(request_file), 'COREAV_FALLBACK': eb.NO_FALLBACK,
           'COREAV_FALLBACK_LOG': str(cx.base / 'image-fallback.jsonl'),
           'COREAV_FFMPEG': eb.NO_FALLBACK, 'COREAV_X264': eb.NO_FALLBACK}
    ran = eb.execute([cx.opts.image_harness, '--ignored', '--exact', 'capture_image_inputs', '--test-threads', '1'],
                     run.cwd, env, eb.COREAV_SECONDS)
    (cx.base / 'image-process.json').write_text(json.dumps(ran._asdict()))
    require(not ran.timed_out, 'harness_timeout')
    require(ran.code == 0, 'harness_failed')
    require(not eb.fallback_lines(pathlib.Path(env['COREAV_FALLBACK_LOG'])), 'harness_failed')
    require(all(eb.sha256(path) == expected_source for path in staged_sources + [source]), 'source_identity')
    frames = validate_receipt(directory, request, source_path, source_listing, cx.jobs[0].stream, run.cwd,
                              cx.opts.tools['image_harness'])
    same_inventory(cx.run.cwd, run.cwd)
    result = image_comparisons(cx, frames, directory)
    require(all(eb.sha256(path) == expected_source for path in staged_sources + [source]), 'source_identity')
    validate_receipt(directory, request, source_path, source_listing, cx.jobs[0].stream, run.cwd,
                     cx.opts.tools['image_harness'])
    same_inventory(cx.run.cwd, run.cwd)
    require(reference_outputs == {name: eb.sha256(path) for name, path in inventory(cx.reference.cwd).items()},
            'output_identity')
    return result


def oracle(cx):
    selection = eligibility(cx)
    if selection is None:
        return eb.oracle(cx)
    if isinstance(selection, str):
        return 'no_oracle', {'check': selection}
    try:
        result = image_oracle(cx, *selection)
    except Invalid as error:
        if error.reason in CAPTURE_FAULTS:
            raise
        return ('no_oracle' if error.reason == 'reference_shape' else 'oracle'), {'check': error.reason}
    return ('oracle', {'check': result['failures'][0], 'image_comparisons': result}) if result['failures'] else (
        None, result)


def judge(opts, index, entry, base):
    record = {**eb.header(index, entry), **registration(), 'run_identity_sha256': opts.run_identity}
    chosen = eb.choose(opts, entry, base)
    if chosen is None:
        return {**record, 'counted': False, 'miss': 'no_input', 'detail': {}}
    assignment, reference = chosen
    mapping = eb.prepare(opts, entry, base, 'info', assignment)
    found = eb.jobs(eb.execute([opts.ffmpeg] + eb.info(mapping.args), mapping.cwd, opts.env,
                               eb.REFERENCE_SECONDS, entry['output'] == '-').stderr)
    c = eb.coreav_run(opts, entry, base, assignment)
    record.update(inputs=list(assignment), jobs=[j.kind for j in found], groups=eb.groups(found), exit=c.ran.code,
                  starts=c.starts, void=any(line.get('ran') is not None for line in c.lines))
    miss, result = eb.failure(opts, entry, c), None
    if miss is None and any(not start['encoder'] for start in c.starts):
        miss = 'ffmpeg_job', {}
    if miss is None:
        cx = types.SimpleNamespace(opts=opts, entry=entry, base=base, assignment=assignment, jobs=found, run=c.run,
                                   reference=reference, starts=c.starts, sources={}, scratch=base / 'scratch', tc=opts.tc)
        cx.scratch.mkdir()
        selected = oracle(cx)
        if isinstance(selected, tuple) and selected[0] is None:
            _, result = selected
        else:
            miss = selected
        difference = eb.listings(cx) if miss is None else None
        miss = ('streams_differ', difference) if difference else miss
    code, detail = miss or (None, {})
    if result is not None:
        detail = {**detail, 'image_comparisons': result}
    return {**record, 'counted': code is None, 'miss': code, 'detail': detail}


def evaluate(opts, index, entry):
    base = opts.work / f'{index:03d}'
    base.mkdir()
    try:
        require(method_identity() == opts.methods, 'method_identity')
        require(all(eb.sha256(getattr(opts, name)) == sha for name, sha in opts.tools.items()), 'receipt_binding')
        record = judge(opts, index, entry, base)
        require(method_identity() == opts.methods, 'method_identity')
        require(all(eb.sha256(getattr(opts, name)) == sha for name, sha in opts.tools.items()), 'receipt_binding')
        return record
    except Exception as error:
        return {**eb.header(index, entry), **registration(), 'run_identity_sha256': opts.run_identity,
                'counted': False, 'miss': None, 'fault': type(error).__name__ + ':' +
                (error.reason if isinstance(error, Invalid) else 'acquisition_failure')}


def summarize(records, manifest):
    identity = digest(manifest)
    require(manifest.get('schema') == 'coreav.image-run/1'
            and all(manifest.get(k) == v for k, v in registration().items())
            and manifest.get('commandset_sha256') == eb.sha256(eb.SET)
            and manifest.get('methods') == method_identity(), 'method_identity')
    inputs = manifest.get('inputs')
    require(isinstance(inputs, dict) and set(inputs) == set(eb.INPUT_FILES)
            and all(isinstance(sha, str) and re.fullmatch(r'[0-9a-f]{64}', sha) is not None
                    for sha in inputs.values()), 'source_identity')
    require(all(isinstance(row, dict) and all(row.get(k) == v for k, v in registration().items())
                and row.get('run_identity_sha256') == identity for row in records), 'receipt_binding')
    expected = dict(eb.commands('v2'))
    require(all(type(row.get('index')) is int and row['index'] in expected and all(row.get(k) == v for k, v in
                eb.header(row['index'], expected[row['index']]).items()) for row in records), 'receipt_binding')
    require(all(type(row.get('counted')) is bool and row.get('miss') in (None, *eb.MISSES)
                and (not row['counted'] or not (row.get('miss') or row.get('fault') or row.get('void')))
                and (row['counted'] or row.get('miss') or row.get('fault') or row.get('void'))
                for row in records), 'receipt_binding')
    summary = eb.summarize(records, 'v2')
    return {**summary, 'schema': 'coreav.png-bmp-yuv/1', **registration(), 'manifest': manifest,
            'run_identity_sha256': identity}


def cmd_run(opts):
    validate_host()
    source_sha = validate_source_commit()
    for name in ('coreav', 'harness', 'image_harness', 'ffmpeg', 'ffprobe', 'x264'):
        setattr(opts, name, str(pathlib.Path(getattr(opts, name)).resolve(strict=True)))
    opts.fixtures, opts.work = opts.fixtures.resolve(strict=True), opts.work.resolve()
    build = validate_build(opts, source_sha)
    programs = eb.oracle_programs(opts)
    opts.tools = {name: eb.sha256(getattr(opts, name)) for name in
                  ('coreav', 'harness', 'image_harness', 'ffmpeg', 'ffprobe', 'x264')}
    fixtures = {name: eb.sha256(opts.fixtures / name) for name in eb.INPUT_FILES}
    opts.methods = method_identity()
    require(opts.methods == LOADED_METHODS, 'method_identity')
    manifest = {'schema': 'coreav.image-run/1', **registration(), 'commandset_sha256': eb.sha256(eb.SET),
                'inputs': fixtures, 'tools': opts.tools, 'build': build, 'build_manifest_sha256': eb.sha256(opts.build_manifest),
                'reference_programs': programs, 'methods': opts.methods}
    opts.run_identity = digest(manifest)
    opts.env = eb.environment()
    eb.transcode_check.ENV = opts.env
    opts.tc = types.SimpleNamespace(ffmpeg=opts.ffmpeg, ffprobe=opts.ffprobe, coreav=opts.coreav, decode_oracle=None)
    opts.check = types.SimpleNamespace(ffmpeg=opts.ffmpeg, ffprobe=opts.ffprobe, mediainfo=opts.mediainfo,
                                       log=str(opts.work / 'copy_check.log'))
    opts.work.mkdir(parents=True, exist_ok=False)
    with concurrent.futures.ThreadPoolExecutor(opts.jobs) as pool:
        records = sorted(pool.map(lambda item: evaluate(opts, *item), eb.commands('v2')), key=lambda row: row['index'])
    require(opts.tools == {name: eb.sha256(getattr(opts, name)) for name in opts.tools}, 'receipt_binding')
    require(fixtures == {name: eb.sha256(opts.fixtures / name) for name in eb.INPUT_FILES}, 'source_identity')
    require(method_identity() == opts.methods and eb.sha256(opts.build_manifest) == manifest['build_manifest_sha256'],
            'method_identity')
    summary = summarize(records, manifest)
    for path in (opts.records, opts.summary, opts.manifest):
        path.parent.mkdir(parents=True, exist_ok=True)
    opts.records.write_text(''.join(json.dumps(row, allow_nan=False) + '\n' for row in records))
    opts.summary.write_text(json.dumps(summary, indent=2, allow_nan=False) + '\n')
    opts.manifest.write_text(json.dumps(manifest, indent=2, allow_nan=False) + '\n')
    return eb.brief(summary)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='mode', required=True)
    run = sub.add_parser('run')
    run.add_argument('--registration', required=True, choices=(VERSION,))
    for name in ('coreav', 'harness', 'image-harness', 'ffmpeg', 'ffprobe', 'x264'):
        run.add_argument('--' + name, required=True)
    for name in ('fixtures', 'work', 'records', 'summary', 'manifest', 'build-manifest'):
        run.add_argument('--' + name, type=pathlib.Path, required=True)
    run.add_argument('--mediainfo')
    run.add_argument('--jobs', type=int, default=4)
    report = sub.add_parser('summarize')
    report.add_argument('--registration', required=True, choices=(VERSION,))
    report.add_argument('--records', type=pathlib.Path, required=True)
    report.add_argument('--manifest', type=pathlib.Path, required=True)
    report.add_argument('--summary', type=pathlib.Path, required=True)
    opts = parser.parse_args()
    try:
        if opts.mode == 'run':
            require(opts.jobs > 0, 'receipt_binding')
            env = eb.environment()
            if dict(os.environ) != env:
                os.execve(sys.executable, [sys.executable, '-I', '-B', str(pathlib.Path(__file__).resolve()),
                                          *sys.argv[1:]], env)
            return cmd_run(opts)
        records = [json.loads(line, object_pairs_hook=unique_pairs) for line in opts.records.read_text().splitlines()]
        summary = summarize(records, document(opts.manifest))
        opts.summary.write_text(json.dumps(summary, indent=2, allow_nan=False) + '\n')
        return eb.brief(summary)
    except (Invalid, OSError, ValueError, subprocess.SubprocessError) as error:
        print('VOID: ' + str(error), file=sys.stderr)
        return 2


LOADED_METHODS = method_identity()


if __name__ == '__main__':
    sys.exit(main())
