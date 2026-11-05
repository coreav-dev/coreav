#!/usr/bin/env python3
"""Prepare or verify tiny synthetic HEVC references, never a CoreAV gate."""

import argparse
import hashlib
import json
import os
import pathlib
import re
import resource
import selectors
import signal
import struct
import subprocess
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import oracle_pin

ROOT = pathlib.Path(__file__).resolve().parents[1]
REGISTRATION = ROOT / 'bench/hevc-conformance/synthetic.json'
FILE_CAP = 2 << 20
CAPTURE_CAP = 1 << 20
TIMEOUT = 30
FRAME_BYTES = 64 * 64 * 3
ENVIRONMENT = {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C', 'HOME': '/nonexistent',
               'OMP_NUM_THREADS': '1'}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def file_sha256(path):
    value = hashlib.sha256()
    with pathlib.Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            value.update(block)
    return value.hexdigest()


def child_limits():
    for kind, limit in ((resource.RLIMIT_AS, 2 << 30), (resource.RLIMIT_CPU, 20),
                        (resource.RLIMIT_FSIZE, FILE_CAP), (resource.RLIMIT_NOFILE, 64),
                        (resource.RLIMIT_CORE, 0)):
        resource.setrlimit(kind, (limit, limit))


def capture(command, allow_unknown=False):
    require(sys.platform == 'linux', 'fixture subprocesses require Linux limits')
    process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, env=ENVIRONMENT,
                               start_new_session=True, preexec_fn=child_limits)
    buffers = [bytearray(), bytearray()]
    deadline = time.monotonic() + TIMEOUT
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ, 0)
            selector.register(process.stderr, selectors.EVENT_READ, 1)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                require(remaining > 0, 'fixture subprocess deadline reached')
                for key, _ in selector.select(remaining):
                    block = os.read(key.fileobj.fileno(), 65536)
                    if not block:
                        selector.unregister(key.fileobj)
                        continue
                    target = buffers[key.data]
                    require(len(target) + len(block) < CAPTURE_CAP, 'fixture capture limit reached')
                    target.extend(block)
            process.wait(timeout=max(0.001, deadline - time.monotonic()))
        require(process.returncode == 0, 'fixture subprocess failed')
        text = buffers[1].decode('utf-8', errors='replace')
        require(allow_unknown or not re.search(r'Unknown option|Error setting|Error parsing|Invalid value', text, re.I),
                'unsupported fixture parameter')
        return bytes(buffers[0]), text
    finally:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
        process.stdout.close()
        process.stderr.close()


def recipes():
    common = ('asm=0:pools=none:frame-threads=1:wpp=0:keyint=16:min-keyint=1:'
              'bframes=0:rc-lookahead=0:scenecut=0:open-gop=0:repeat-headers=1:'
              'hash=1:sao=0:no-deblock=1:min-cu-size=8:tu-intra-depth=3:tskip=1')
    result = []
    for name, ctu, tu, scaling, bypass, delta in (
            ('main10-ctu16-tu4', 16, 4, False, False, 0),
            ('main10-ctu32-tu16', 32, 16, True, False, 0),
            ('main10-bypass', 64, 32, True, True, 0),
            ('main10-bypass-changed', 64, 32, True, True, 1)):
        source = ('nullsrc=s=64x64:r=1,format=yuv420p10le,'
                  f'geq=lum=mod(X*17+Y*29+N*37+{delta}\\,1024):'
                  f'cb=mod(X*11+Y*23+N*13+{delta}\\,1024):'
                  f'cr=mod(X*31+Y*7+N*19+{delta}\\,1024)')
        params = (f'{common}:ctu={ctu}:max-tu-size={tu}:scaling-list='
                  f'{"default" if scaling else "off"}:'
                  f'{"lossless=1" if bypass else "qp=18"}')
        command = ['{ffmpeg}', '-nostdin', '-hide_banner', '-cpuflags', '0',
                   '-filter_threads', '1', '-filter_complex_threads', '1', '-threads', '1',
                   '-f', 'lavfi', '-i', source, '-map', '0:v:0', '-frames:v', '2', '-an',
                   '-c:v', 'libx265', '-profile:v', 'main10', '-preset', 'medium',
                   '-threads', '1', '-forced-idr', '1', '-force_key_frames',
                   'expr:gte(t,n_forced)', '-x265-params', params,
                   '-f', 'hevc', '-y', '{bitstream}']
        result.append(dict(id=name, ctu=ctu, max_tu=tu, scaling_list=scaling,
                           transquant_bypass=bypass, content_delta=delta, command=command))
    return result


def decode_command():
    return ['{ffmpeg}', '-nostdin', '-v', 'error', '-cpuflags', '0', '-threads', '1',
            '-noautorotate', '-flags', 'unaligned', '-c:v', 'hevc', '-i', '{bitstream}',
            '-map', '0:v:0', '-fps_mode', 'passthrough', '-noautoscale', '-threads', '1',
            '-pix_fmt', 'yuv420p10le', '-f', 'rawvideo', '-y', '{raw}']


def expand(command, **paths):
    return [paths.get(token[1:-1], token) if token.startswith('{') else token for token in command]


def bounded_bytes(path):
    path = pathlib.Path(path)
    require(0 < path.stat().st_size < FILE_CAP, 'fixture file size outside bounds')
    return path.read_bytes()


def nal_units(data):
    require(0 < len(data) < FILE_CAP, 'fixture bitstream size outside bounds')
    starts = list(re.finditer(b'\x00\x00\x01', data))
    require(starts and not any(data[:starts[0].start()]), 'missing Annex B start code')
    require(len(starts) < 100, 'too many fixture NAL units')
    for index, marker in enumerate(starts):
        end = starts[index + 1].start() if index + 1 < len(starts) else len(data)
        nal = data[marker.end():end].rstrip(b'\x00')
        require(len(nal) >= 3 and not nal[0] & 0x81 and nal[1] == 1,
                'fixture requires single-layer temporal-id-zero NAL units')
        yield (nal[0] >> 1) & 63, nal[2:]


def rbsp(data):
    result = bytearray()
    zeros = 0
    for index, value in enumerate(data):
        if zeros == 2 and value == 3:
            require(index + 1 < len(data) and data[index + 1] <= 3,
                    'invalid emulation prevention byte')
            zeros = 0
            continue
        result.append(value)
        zeros = zeros + 1 if value == 0 else 0
    return bytes(result)


def picture_hashes(data):
    pictures = []
    for kind, payload in nal_units(data):
        if kind < 32:
            require(kind in (19, 20) and payload[0] & 0x80,
                    'fixture requires one independent IDR slice per picture')
            require(not pictures or pictures[-1] is not None, 'picture hash SEI absent')
            pictures.append(None)
        if kind != 40:
            continue
        value = rbsp(payload)
        offset = 0
        while offset < len(value) and value[offset:] != b'\x80':
            fields = []
            for _ in range(2):
                total = 0
                while True:
                    require(offset < len(value), 'truncated SEI header')
                    byte = value[offset]
                    offset += 1
                    total += byte
                    if byte != 255:
                        break
                fields.append(total)
            kind_sei, size = fields
            require(offset + size <= len(value), 'truncated SEI payload')
            message = value[offset:offset + size]
            offset += size
            if kind_sei == 132:
                require(pictures and pictures[-1] is None, 'unattached or duplicate picture hash')
                require(len(message) == 49 and message[0] == 0,
                        'fixture requires three-component decoded-picture MD5 SEI')
                pictures[-1] = [message[i:i + 16].hex() for i in (1, 17, 33)]
        require(value[offset:] == b'\x80', 'invalid SEI trailing bits')
    require(len(pictures) == 2 and all(p is not None for p in pictures),
            'fixture requires two pictures with hash SEI')
    return pictures


def raw_metadata(raw):
    require(len(raw) == 2 * FRAME_BYTES, 'raw picture count or packing differs')
    pictures = []
    for offset in range(0, len(raw), FRAME_BYTES):
        frame = raw[offset:offset + FRAME_BYTES]
        planes = [frame[:8192], frame[8192:10240], frame[10240:]]
        proof = []
        for plane in planes:
            samples = list(struct.unpack('<' + 'H' * (len(plane) // 2), plane))
            require(max(samples) <= 1023, 'samples exceed ten-bit range')
            require(max(samples) > 255 and any(s & 3 for s in samples),
                    'missing effective ten-bit sample evidence')
            proof.append(dict(min=min(samples), max=max(samples),
                              above_255=sum(s > 255 for s in samples),
                              nonzero_low_two_bits=sum(bool(s & 3) for s in samples)))
        pictures.append(dict(visible_yuv_md5=hashlib.md5(frame).hexdigest(),
                             coded_component_md5=[hashlib.md5(p).hexdigest() for p in planes],
                             sample_proof=proof))
    return dict(bytes=len(raw), sha256=sha256(raw), whole_visible_yuv_md5=hashlib.md5(raw).hexdigest(),
                pictures=pictures)


def verify_hash_sei(bitstream, raw):
    expected = picture_hashes(bitstream)
    actual = [p['coded_component_md5'] for p in raw_metadata(raw)['pictures']]
    require(actual == expected, 'coded-picture component MD5 disagrees with SEI')
    return expected


def reject_crossed_hashes(bitstream, raw):
    try:
        verify_hash_sei(bitstream, raw)
    except ValueError as error:
        require(str(error) == 'coded-picture component MD5 disagrees with SEI',
                'negative control failed for a reason other than hash disagreement')
        return
    raise ValueError('negative control unexpectedly matched')


def trace_fields(text, recipe):
    names = ['general_profile_idc', 'general_profile_compatibility_flag[2]',
             'pic_width_in_luma_samples', 'pic_height_in_luma_samples',
             'conformance_window_flag', 'bit_depth_luma_minus8', 'bit_depth_chroma_minus8',
             'log2_min_luma_coding_block_size_minus3', 'log2_diff_max_min_luma_coding_block_size',
             'log2_min_luma_transform_block_size_minus2', 'log2_diff_max_min_luma_transform_block_size',
             'sample_adaptive_offset_enabled_flag', 'pcm_enabled_flag', 'scaling_list_enabled_flag',
             'sps_scaling_list_data_present_flag', 'pps_scaling_list_data_present_flag',
             'transform_skip_enabled_flag', 'transquant_bypass_enabled_flag',
             'pps_deblocking_filter_disabled_flag', 'slice_type']
    fields = {}
    for name in names:
        matches = re.findall(r'\s' + re.escape(name) + r'\s+[01]+\s+=\s+(\d+)', text)
        if name == 'sps_scaling_list_data_present_flag' and not recipe['scaling_list']:
            require(not matches, 'unexpected scaling-list syntax')
            fields[name] = None
            continue
        require(matches and len(set(matches)) == 1, 'missing or changing trace field: ' + name)
        fields[name] = int(matches[0])
    expected = [2, 1, 64, 64, 0, 2, 2, 0, recipe['ctu'].bit_length() - 4,
                0, recipe['max_tu'].bit_length() - 3, 0, 0, int(recipe['scaling_list']),
                0 if recipe['scaling_list'] else None, 0, 1, int(recipe['transquant_bypass']), 1, 2]
    require(list(fields.values()) == expected, 'fixture syntax disagrees with recipe')
    return fields


def binary_identity(ffmpeg, ffprobe):
    result = {}
    for name, path in [('ffmpeg', ffmpeg), ('ffprobe', ffprobe)]:
        output, _ = capture([str(path), '-version'])
        version = output.decode().splitlines()[0]
        build = oracle_pin.load()['BUILD']
        require(version.split()[:2] == [name, 'version']
                and (version.split()[2] == build or version.split()[2].startswith(build + '-')),
                'fixture requires pinned oracle build')
        result[name] = dict(sha256=file_sha256(path),
                            version_pin='scripts/oracle-ffmpeg.env:BUILD',
                            version_suffix=version[len(name + ' version ' + build):])
    return result


def pcm_constraint(ffmpeg, directory):
    attempts = []
    for parameter in ('pcm=1', 'ipcm=1'):
        command = recipes()[0]['command'].copy()
        index = command.index('-x265-params') + 1
        command[index] += ':' + parameter
        path = directory / 'unsupported-pcm.hevc'
        _, log = capture(expand(command, ffmpeg=str(ffmpeg), bitstream=str(path)), allow_unknown=True)
        diagnostic = 'Unknown option: ' + parameter.split('=')[0] + '.'
        require(diagnostic in log, 'PCM parameter behavior changed; investigate before registration')
        path.unlink(missing_ok=True)
        attempts.append(dict(parameter=parameter, diagnostic=diagnostic, command=command))
    return dict(status='no_PCM_fixture_from_tested_parameters', attempts=attempts)


def prepare(ffmpeg, ffprobe, directory):
    directory = pathlib.Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    oracle = binary_identity(ffmpeg, ffprobe)
    records = []
    retained = {}
    for recipe in recipes():
        observations = []
        for attempt in (1, 2):
            work = directory / f'run-{attempt}'
            work.mkdir(exist_ok=True)
            bitstream = work / (recipe['id'] + '.hevc')
            raw = work / (recipe['id'] + '.yuv')
            paths = dict(ffmpeg=str(ffmpeg), bitstream=str(bitstream), raw=str(raw))
            _, log = capture(expand(recipe['command'], **paths))
            version = re.search(r'x265 \[info\]: HEVC encoder version ([^\n]+)', log)
            require(version is not None, 'missing encoder version from binary log')
            capture(expand(decode_command(), **paths))
            data, pixels = bounded_bytes(bitstream), bounded_bytes(raw)
            sei = verify_hash_sei(data, pixels)
            output, _ = capture([str(ffprobe), '-v', 'error', '-threads', '1', '-c:v', 'hevc',
                                 '-show_streams', '-show_frames', '-of', 'json', str(bitstream)])
            probe = json.loads(output)
            require(len(probe['streams']) == 1 and len(probe['frames']) == 2,
                    'oracle stream or picture count differs')
            stream = probe['streams'][0]
            require(all(stream[k] == v for k, v in dict(codec_name='hevc', profile='Main 10',
                    width=64, height=64, coded_width=64, coded_height=64,
                    pix_fmt='yuv420p10le', has_b_frames=0).items()), 'oracle format differs')
            require(all(f['pict_type'] == 'I' and f['pix_fmt'] == 'yuv420p10le'
                        and f['width'] == f['height'] == 64
                        and all(f.get('crop_' + side, 0) == 0 for side in ('left', 'right', 'top', 'bottom'))
                        for f in probe['frames']), 'oracle pictures are not uncropped Main10 intra')
            _, trace = capture([str(ffmpeg), '-nostdin', '-hide_banner', '-loglevel', 'info',
                                '-i', str(bitstream), '-map', '0:v:0', '-c:v', 'copy',
                                '-bsf:v', 'trace_headers', '-f', 'null', '-'])
            observation = dict(bitstream_bytes=len(data), bitstream_sha256=sha256(data),
                               encoder_version=version.group(1),
                               syntax=trace_fields(trace, recipe), raw=raw_metadata(pixels),
                               decoded_picture_hash_sei_md5=sei)
            if recipe['transquant_bypass']:
                expected = bytearray()
                for n in range(2):
                    for size, a, b, c in ((64, 17, 29, 37), (32, 11, 23, 13), (32, 31, 7, 19)):
                        for y in range(size):
                            for x in range(size):
                                expected.extend(struct.pack('<H', (x*a + y*b + n*c + recipe['content_delta']) % 1024))
                require(pixels == expected, 'bypass reconstruction differs from synthetic samples')
            observations.append(observation)
            retained[recipe['id']] = (data, pixels)
        require(observations[0] == observations[1], 'fixture regeneration is not byte-identical')
        records.append(dict(id=recipe['id'], **observations[0]))
    original, changed = (retained[name] for name in ('main10-bypass', 'main10-bypass-changed'))
    require(original[1] != changed[1], 'changed ten-bit content produced identical decoded bytes')
    require(all(a != b for pa, pb in zip(picture_hashes(original[0]), picture_hashes(changed[0]))
                for a, b in zip(pa, pb)), 'changed content failed to change every component hash')
    reject_crossed_hashes(original[0], changed[1])
    reject_crossed_hashes(changed[0], original[1])
    damaged = bytearray(original[1])
    damaged[0] ^= 1
    reject_crossed_hashes(original[0], damaged)
    damaged_hash = bytearray(original[0])
    first_hash = bytes.fromhex(picture_hashes(original[0])[0][0])
    offset = original[0].find(first_hash)
    require(offset >= 0 and original[0].count(first_hash) == 1,
            'negative-control hash byte location is ambiguous')
    damaged_hash[offset] ^= 1
    reject_crossed_hashes(damaged_hash, original[1])
    pcm = pcm_constraint(ffmpeg, directory)
    require(binary_identity(ffmpeg, ffprobe) == oracle, 'oracle binary changed during preparation')
    return dict(version=1, purpose='reference_preparation_only_not_a_CoreAV_G2_verdict',
                oracle=oracle, geometry=dict(width=64, height=64, depth=10, chroma='4:2:0',
                                           pictures=2, crop=[0, 0, 0, 0], pixel_format='yuv420p10le'),
                recipes=recipes(), decode_command=decode_command(), fixtures=records, pcm=pcm,
                controls=dict(two_generations_identical=True, crossed_component_hashes_rejected=True,
                              changed_10bit_content_changes_decoded_bytes=True,
                              altered_raw_rejected=True, altered_SEI_hash_rejected=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('prepare', 'verify'))
    parser.add_argument('--ffmpeg', required=True, type=pathlib.Path)
    parser.add_argument('--ffprobe', required=True, type=pathlib.Path)
    parser.add_argument('--output', required=True, type=pathlib.Path)
    parser.add_argument('--registration', type=pathlib.Path, default=REGISTRATION)
    args = parser.parse_args()
    expected = None
    if args.mode == 'verify':
        expected = json.loads(args.registration.read_text())
        require(expected['recipes'] == recipes() and expected['decode_command'] == decode_command(),
                'registered commands differ')
        require(binary_identity(args.ffmpeg.resolve(), args.ffprobe.resolve()) == expected['oracle'],
                'registered oracle identity differs')
    observed = prepare(args.ffmpeg.resolve(), args.ffprobe.resolve(), args.output.resolve())
    if expected is not None:
        require(observed == expected, 'registered fixture content or expected hashes differ')
    (args.output / 'reference.json').write_text(json.dumps(observed, indent=2) + '\n')
    print(f'{args.mode}: four synthetic references verified; no CoreAV G2 verdict')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
        print('hevc_fixture: incomplete reference preparation or verification', file=sys.stderr)
        sys.exit(1)
