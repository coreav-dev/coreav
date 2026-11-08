#!/usr/bin/env python3
"""Generated CLI encoding, unchanged defaults, and exact external fallback."""
import argparse
from fractions import Fraction
import json
import os
from pathlib import Path
import re
import struct
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import h264_crf_conformance as crf
import h264_p_conformance as syntax
import test_oracle
import track_boxes


def run(argv, *, env=None, success=True):
    result = subprocess.run(list(map(str, argv)), capture_output=True, timeout=120, env=env)
    if success and result.returncode:
        raise ValueError(f'command failed: {argv[0]}: {result.stderr.decode(errors="replace")}')
    return result


def same(actual, expected, label):
    if actual != expected:
        raise ValueError(f'{label} differs')


def annexb_units(data):
    parts = re.split(rb'\x00\x00\x00?\x01', data)
    if parts.pop(0) or not parts or any(not part for part in parts):
        raise ValueError('invalid Annex B delimiters')
    return [part.rstrip(b'\0') for part in parts]


def entry_aspect(data):
    parent = (0, len(data))
    for kind in (b'moov', b'trak', b'mdia', b'minf', b'stbl', b'stsd', b'avc1'):
        parent = track_boxes.child(data, parent, kind)
        if parent is None:
            raise ValueError('missing generated video sample entry')
        if kind == b'stsd':
            parent = (parent[0] + 8, parent[1])
        elif kind == b'avc1':
            parent = (parent[0] + 78, parent[1])
    return track_boxes.child(data, parent, b'pasp')


def terminal_duration(data, ticks):
    parents = []
    parent = (0, len(data))
    for kind in (b'moov', b'trak', b'mdia', b'minf', b'stbl'):
        parent = track_boxes.child(data, parent, kind)
        if parent is None:
            raise ValueError('missing generated sample table parent')
        start, end = parent
        same(struct.unpack_from('>I', data, start - 8)[0], end - start + 8, 'generated 32-bit parent')
        parents.append(parent)
    same(parents[0][1], len(data), 'generated movie follows media')
    table = track_boxes.child(data, parent, b'stts')
    if table is None or table[1] - table[0] != 16:
        raise ValueError('expected one generated duration run')
    start, end = table
    same(struct.unpack_from('>I', data, start - 8)[0], end - start + 8, 'generated 32-bit duration box')
    version, entries, count, delta = struct.unpack_from('>IIII', data, start)
    same((version, entries), (0, 1), 'generated duration header')
    if not count or not delta:
        raise ValueError('empty generated duration run')
    changed = bytearray(data)
    if count == 1:
        struct.pack_into('>I', changed, start + 12, ticks)
    else:
        changed[start:end] = struct.pack('>IIIIII', 0, 2, count - 1, delta, 1, ticks)
        struct.pack_into('>I', changed, start - 8, end - start + 16)
        for parent_start, parent_end in parents:
            struct.pack_into('>I', changed, parent_start - 8, parent_end - parent_start + 16)
    return bytes(changed)


def sps_metadata(nal):
    r = syntax.rbsp(nal)
    same(r.u(24), 0x42c033, 'native SPS profile constraints and level')
    same([r.ue() for _ in range(4)], [0, 0, 2, 1], 'native SPS fixed fields')
    same(r.u(1), 0, 'frame number gaps')
    r.ue()
    r.ue()
    same((r.u(1), r.u(1)), (1, 1), 'progressive SPS')
    if r.u(1):
        for _ in range(4):
            r.ue()
    same(r.u(1), 1, 'VUI presence')
    aspect = None
    if r.u(1):
        code = r.u(8)
        ratios = ((1, 1), (12, 11), (10, 11), (16, 11), (40, 33), (24, 11),
                  (20, 11), (32, 11), (80, 33), (18, 11), (15, 11), (64, 33),
                  (160, 99), (4, 3), (3, 2), (2, 1))
        if code == 255:
            width, height = r.u(16), r.u(16)
            if not width or not height:
                raise ValueError('zero SPS sample aspect')
            aspect = Fraction(width, height)
        elif 1 <= code <= len(ratios):
            aspect = Fraction(*ratios[code - 1])
        else:
            raise ValueError('unsupported SPS aspect code')
    same((r.u(1), r.u(1), r.u(3), r.u(1), r.u(1)), (0, 1, 5, 0, 1), 'video signal')
    same([r.u(8) for _ in range(3)], [1, 1, 1], 'BT.709 colour')
    same((r.u(1), r.ue(), r.ue()), (1, 0, 0), 'chroma location')
    same(r.u(1), 1, 'VUI timing presence')
    tick, scale = r.u(32), r.u(32)
    same(r.u(1), 1, 'fixed frame rate')
    same([r.u(1) for _ in range(4)], [0] * 4, 'HRD and restrictions')
    r.finish()
    if not tick or not scale:
        raise ValueError('zero VUI clock')
    return Fraction(scale, 2 * tick), aspect


def verify(args):
    tools = test_oracle.require(args.ffmpeg, args.ffmpeg.with_name('ffprobe'))
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    environment = {**os.environ, 'COREAV_X264': '/nonexistent/native-cli-x264',
                   'COREAV_FFMPEG': str(tools.ffmpeg), 'COREAV_FALLBACK': str(tools.ffmpeg),
                   'COREAV_STRICT_SAFE': '0'}
    native_only = {**environment, 'COREAV_FFMPEG': '/nonexistent/native-cli-ffmpeg',
                   'COREAV_FALLBACK': '/nonexistent/native-cli-fallback'}

    def source(name, width=18, height=34, rate='30', extra=(), frames=4, raw=None):
        path = root / f'{name}.mp4'
        if raw is None:
            raw = root / f'{name}.yuv'
            raw.write_bytes(crf.pictures(width, height, frames))
        run([tools.ffmpeg, '-v', 'error', '-f', 'rawvideo', '-pix_fmt', 'yuv420p',
             '-s', f'{width}x{height}', '-r', rate, '-i', raw, '-frames:v', frames,
             '-an', '-c:v', 'libx264', '-preset', 'ultrafast', '-qp', 18, '-bf', 0,
             '-color_primaries', 'bt709', '-color_trc', 'bt709', '-colorspace', 'bt709',
             '-x264-params', 'colorprim=bt709:transfer=bt709:colormatrix=bt709',
             *extra, path])
        return path

    source_path = source('source')
    source_info = json.loads(run([tools.ffprobe, '-v', 'error', '-show_streams', '-of', 'json', source_path]).stdout)['streams'][0]
    same([source_info.get(key) for key in ('color_space', 'color_transfer', 'color_primaries')],
         ['bt709'] * 3, 'generated BT.709 source metadata')
    raw = root / 'decoded-source.yuv'
    run([tools.ffmpeg, '-v', 'error', '-i', source_path, '-an', '-pix_fmt', 'yuv420p', '-f', 'rawvideo', raw])
    rows = []
    for mode, quality in [('crf', 10), ('crf', 23), ('crf', 51), ('qp', 10), ('qp', 28), ('qp', 51)]:
        reference = root / f'reference-{mode}-{quality}'
        reference.mkdir()
        expected = reference / 'reconstruction.yuv'
        run([args.driver, 18, 34, f'crf:{quality}' if mode == 'crf' else quality, 4,
             raw, reference / 'stream.h264', expected, reference / 'own.yuv', 'true', 250])
        container = 'mp4'
        identity = None
        for threads in ('1', '2', '8', '0', 'auto'):
            output = root / f'native-{mode}-{quality}-{threads}.{container}'
            command = ['-i', source_path, '-an', '-c:v', 'libx264', f'-{mode}', quality,
                       '-threads', threads, output]
            run([args.candidate, '--native-h264-encoder', 'ffmpeg', *command], env=native_only)
            if identity is None:
                identity = output.read_bytes()
            else:
                same(output.read_bytes(), identity, 'thread-independent complete output')
            if threads != '1':
                continue
            info = json.loads(run([tools.ffprobe, '-v', 'error', '-show_streams',
                                   '-show_packets', '-of', 'json', output]).stdout)
            stream, = info['streams']
            same((stream['profile'], stream['width'], stream['height'], stream['pix_fmt']),
                 ('Constrained Baseline', 18, 34, 'yuv420p'), 'native output properties')
            same(stream['r_frame_rate'], '30/1', 'native output rate')
            same(len(info['packets']), 4, 'stored picture count')
            times = [Fraction(p['pts']) * Fraction(stream['time_base']) for p in info['packets']]
            for index, time in enumerate(times):
                if abs(time - Fraction(index, 30)) > Fraction(1, 1000):
                    raise ValueError('stored presentation time differs')
            decoded = root / f'decoded-{mode}-{quality}.{container}.yuv'
            run([tools.ffmpeg, '-v', 'error', '-xerror', '-i', output, '-an', '-f', 'rawvideo', decoded])
            same(decoded.read_bytes(), expected.read_bytes(), 'pinned decoder reconstruction')
            own = root / f'own-{mode}-{quality}.{container}.yuv'
            run([args.candidate, 'ffmpeg', '-i', output, '-an', '-c:v', 'rawvideo',
                 '-pix_fmt', 'yuv420p', '-f', 'rawvideo', own], env=native_only)
            same(own.read_bytes(), expected.read_bytes(), 'CoreAV decoder reconstruction')
            elementary = root / f'syntax-{mode}-{quality}.{container}.h264'
            run([tools.ffmpeg, '-v', 'error', '-i', output, '-c:v', 'copy', '-an',
                 '-bsf:v', 'h264_mp4toannexb', '-f', 'h264', elementary])
            units = annexb_units(elementary.read_bytes())
            parameters = [n for n in units if n[0] & 31 in (7, 8)][:2]
            pictures = [n for n in units if n[0] & 31 in (1, 5)]
            repeated = b''.join(b'\0\0\0\1' + n for picture in pictures for n in [*parameters, picture])
            qps = crf.expected_qps(raw.read_bytes(), 18, 34, 4, quality, 250) if mode == 'crf' else None
            syntax.inspect(repeated, 18, 34, quality, 4, True, 120, picture_qps=qps)
            rows.append(dict(mode=mode, quality=quality, container=container, frames=4))

    metadata_rows = []
    for index, (rate, sar, frames) in enumerate([
        ('24/1', '1/1', 4), ('25/1', '4/3', 4), ('24000/1001', '12/11', 4),
        ('30000/1001', '16/15', 4), ('60000/1001', '17/13', 4),
        ('25/2', '65535/65534', 4), ('1000/33', '40/33', 4), ('30/1', '4/3', 4),
        ('30000/1001', '16/15', 1),
        ('25/1', '17/13', 1),
    ]):
        rate, sar = Fraction(rate), Fraction(sar)
        path = source(f'metadata-{index}', rate=str(rate), frames=frames, extra=('-vf', f'setsar={sar}:max=65535'))
        decoded_source = root / f'metadata-source-{index}.yuv'
        run([tools.ffmpeg, '-v', 'error', '-i', path, '-an', '-f', 'rawvideo', decoded_source])
        for mode, quality in [('crf', 23), ('qp', 28)]:
            prefix = root / f'metadata-{index}-{mode}'
            reference = prefix.with_suffix('.reference.h264')
            reconstruction = prefix.with_suffix('.reference.yuv')
            run([args.driver, 18, 34, f'crf:{quality}' if mode == 'crf' else quality, frames,
                 decoded_source, reference, reconstruction, prefix.with_suffix('.reference-own.yuv'),
                 'true', 250, rate.numerator, rate.denominator, 0 if sar == 1 else sar.numerator, sar.denominator])
            identity = None
            for threads in ('1', '2', '8', '0', 'auto'):
                output = root / f'metadata-{index}-{mode}-{threads}.mp4'
                run([args.candidate, '--native-h264-encoder', 'ffmpeg', '-i', path, '-an',
                     '-c:v', 'libx264', f'-{mode}', quality, '-threads', threads, output], env=native_only)
                if identity is not None:
                    same(output.read_bytes(), identity, 'metadata thread-independent complete output')
                    continue
                identity = output.read_bytes()
                info = json.loads(run([tools.ffprobe, '-v', 'error', '-show_streams', '-show_packets',
                                       '-of', 'json', output]).stdout)
                stream, = info['streams']
                for field in ('r_frame_rate', 'avg_frame_rate'):
                    same(Fraction(stream[field]), rate, field)
                same(Fraction(stream['sample_aspect_ratio'].replace(':', '/')), sar, 'declared SAR')
                same(Fraction(stream['display_aspect_ratio'].replace(':', '/')), sar * Fraction(18, 34), 'DAR')
                same([Fraction(p['pts']) * Fraction(stream['time_base']) for p in info['packets']],
                     [Fraction(i) / rate for i in range(frames)], 'exact presentation clock')
                same(Fraction(info['packets'][-1]['duration']) * Fraction(stream['time_base']), 1 / rate, 'last duration')
                pasp = entry_aspect(identity)
                if pasp is None or pasp[1] - pasp[0] != 8:
                    raise ValueError('missing or malformed declared pasp')
                same(Fraction(*struct.unpack_from('>II', identity, pasp[0])), sar, 'independent MP4 pasp')
                elementary = output.with_suffix('.h264')
                run([tools.ffmpeg, '-v', 'error', '-i', output, '-c:v', 'copy', '-an',
                     '-bsf:v', 'h264_mp4toannexb', '-f', 'h264', elementary])
                units = annexb_units(elementary.read_bytes())
                same(sps_metadata(next(n for n in units if n[0] & 31 == 7)),
                     (rate, None if sar == 1 else sar), 'independent SPS metadata')
                reference_units = annexb_units(reference.read_bytes())
                same([n for n in units if n[0] & 31 in (7, 8)],
                     [n for n in reference_units if n[0] & 31 in (7, 8)][:2], 'fresh same-head parameter sets')
                same([n for n in units if n[0] & 31 in (1, 5)],
                     [n for n in reference_units if n[0] & 31 in (1, 5)], 'fresh same-head coded pictures')
                for executable, name in ((tools.ffmpeg, 'oracle'), (args.candidate, 'coreav')):
                    decoded = output.with_suffix(f'.{name}.yuv')
                    command = [executable, '-v', 'error', '-xerror', '-i', output, '-an', '-f', 'rawvideo', decoded] if name == 'oracle' else [executable, 'ffmpeg', '-i', output, '-an', '-c:v', 'rawvideo', '-pix_fmt', 'yuv420p', '-f', 'rawvideo', decoded]
                    run(command, env=native_only)
                    same(decoded.read_bytes(), reconstruction.read_bytes(), f'{name} metadata reconstruction')
            metadata_rows.append(dict(rate=str(rate), sar=str(sar), mode=mode, frames=frames))

    square = source('square-declaration', extra=('-vf', 'setsar=1'))
    square_bytes = square.read_bytes()
    pasp = entry_aspect(square_bytes)
    if pasp is None:
        raise ValueError('missing square source declaration')
    legacy_sources = [('absent', source_path), ('square', square)]
    unspecified = root / 'unspecified-aspect.mp4'
    data = bytearray(square_bytes)
    struct.pack_into('>II', data, pasp[0], 0, 1)
    unspecified.write_bytes(data)
    legacy_sources.append(('unspecified', unspecified))
    one_clock = source('legacy-one-clock', frames=1, extra=('-video_track_timescale', '30'))
    one_zero = root / 'legacy-one-zero.mp4'
    one_zero.write_bytes(terminal_duration(one_clock.read_bytes(), 0))
    final_zero = root / 'legacy-final-zero.mp4'
    final_zero.write_bytes(terminal_duration(source_path.read_bytes(), 0))
    legacy_sources.extend([('one-zero-duration', one_zero), ('final-zero-duration', final_zero)])
    for label, path in legacy_sources:
        previous = []
        for index, executable in enumerate((args.baseline_native, args.candidate)):
            output = root / f'legacy-{label}-{index}.mp4'
            run([executable, '--native-h264-encoder', 'ffmpeg', '-i', path, '-an', '-c:v', 'libx264', output], env=native_only)
            previous.append(output.read_bytes())
        same(*previous, f'unchanged previously eligible {label} bytes')
    print('native base comparisons: ' + ', '.join(label for label, _ in legacy_sources))

    flat = root / 'gop.yuv'
    flat.write_bytes(bytes([128]) * (384 * 251))
    gop_source = source('gop', width=16, height=16, frames=251, raw=flat)
    gop_output = root / 'gop-native.mp4'
    run([args.candidate, '--native-h264-encoder', 'ffmpeg', '-i', gop_source,
         '-an', '-c:v', 'libx264', gop_output], env=native_only)
    gop_packets = json.loads(run([tools.ffprobe, '-v', 'error', '-show_packets', '-of', 'json', gop_output]).stdout)['packets']
    same(len(gop_packets), 251, 'default-quality GOP picture count')
    same([i for i,p in enumerate(gop_packets) if 'K' in p['flags']], [0, 250], 'native IDR interval')
    gop_decoded = root / 'gop-decoded.yuv'
    run([tools.ffmpeg, '-v', 'error', '-xerror', '-i', gop_output, '-an', '-f', 'rawvideo', gop_decoded])
    same(gop_decoded.stat().st_size, 251 * 384, 'complete GOP decode')

    fallback_cases = []

    def compare(source_path, extra, label, container='mp4'):
        outputs = []
        statuses = []
        for selected in (False, True):
            output = root / f'fallback-{label}-{selected}.{container}'
            output.write_bytes(b'preserved destination')
            command = [args.candidate, *(['--native-h264-encoder'] if selected else []),
                       'ffmpeg', '-y', '-i', source_path, '-an', '-c:v', 'libx264', *extra, output]
            result = run(command, env=environment, success=False)
            statuses.append(result.returncode)
            outputs.append(output.read_bytes())
        same(statuses[1], statuses[0], 'fallback exit status')
        same(outputs[1], outputs[0], f'complete fallback bytes ({label})')
        fallback_cases.append(label)

    options = [('-profile:v', 'high'), ('-profile:v', 'baseline'), ('-bf', '2'),
               ('-b:v', '100k'), ('-maxrate', '100k', '-bufsize', '200k'),
               ('-preset', 'fast'), ('-pix_fmt', 'yuv420p10le'), ('-pix_fmt', 'yuv444p'),
               ('-crf', '9'), ('-crf', '23.5'), ('-qp', '9'), ('-qp', '23.5'),
               ('-crf', '23', '-qp', '24'), ('-tune', 'film'), ('-g', '12'),
               ('-x264-params', 'cabac=1'), ('-vf', 'scale=32:32'), ('-s', '32x32')]
    for index, extra in enumerate(options):
        compare(source_path, extra, f'option-{index}')
    compare(source_path, (), 'matroska', 'mkv')
    for name, parameters in [('small', dict(width=14, height=16)),
                             ('wide', dict(width=1922, height=16, frames=1)),
                             ('tall', dict(width=16, height=1082, frames=1)),
                             ('4k', dict(width=3840, height=2160, frames=1)),
                             ('depth10', dict(extra=('-pix_fmt', 'yuv420p10le'))),
                             ('chroma444', dict(extra=('-pix_fmt', 'yuv444p'))),
                             ('hdr', dict(extra=('-x264-params', 'colorprim=bt709:transfer=smpte2084:colormatrix=bt709'))),
                             ('interlace', dict(width=32, height=32, extra=('-flags', '+ildct+ilme', '-x264-params', 'tff=1')))]:
        compare(source(name, **parameters), (), name)

    variable = source('variable-rate', extra=('-vf', 'setpts=if(gte(N\\,2)\\,N+1\\,N)/30/TB',
                                              '-fps_mode', 'passthrough'))
    compare(variable, ('-fps_mode', 'passthrough'), 'variable-rate')
    compare(variable, (), 'variable-native-clock')
    rounded = source('rounded-clock', rate='30000/1001', extra=('-video_track_timescale', '1000'))
    compare(rounded, (), 'rounded-clock')
    for label, parameters in [
        ('zero-duration-new-rate', dict(rate='25', frames=1, extra=('-video_track_timescale', '25'))),
        ('zero-duration-new-aspect', dict(frames=1, extra=('-video_track_timescale', '30', '-vf', 'setsar=4/3'))),
    ]:
        original = source(label + '-source', **parameters)
        path = root / f'{label}.mp4'
        path.write_bytes(terminal_duration(original.read_bytes(), 0))
        output = root / f'{label}-native-only.mp4'
        rejected = run([args.candidate, '--native-h264-encoder', 'ffmpeg', '-i', path,
                        '-an', '-c:v', 'libx264', output], env=native_only, success=False)
        same(rejected.returncode, 4, 'new metadata cannot inherit the legacy zero-duration default')
        if output.exists() or b'no encoder program' not in rejected.stderr:
            raise ValueError('zero-duration control failed outside the existing encoder fallback')
        compare(path, (), label)
    last_unequal = root / 'unequal-final-duration.mp4'
    last_unequal.write_bytes(terminal_duration(source_path.read_bytes(), 1024))
    compare(last_unequal, (), 'unequal-final-duration')
    for label, sar in [('unsupported-aspect', (65536, 65535)), ('display-overflow', (65535, 1))]:
        data = bytearray(square_bytes)
        struct.pack_into('>II', data, pasp[0], *sar)
        path = root / f'{label}.mp4'
        path.write_bytes(data)
        compare(path, (), label)

    state = 17
    dense = bytearray()
    for _ in range(384):
        state ^= state << 13 & 0xffffffff
        state ^= state >> 17
        state ^= state << 5 & 0xffffffff
        dense.append(state & 255)
    mixed = root / 'late-limit.yuv'
    mixed.write_bytes(bytes([128]) * (384 * 250) + dense)
    late = source('late-limit', width=16, height=16, frames=251, raw=mixed, extra=('-qp', '1'))
    decoded_late = root / 'late-decoded.yuv'
    run([tools.ffmpeg, '-v', 'error', '-i', late, '-f', 'rawvideo', decoded_late])
    late_reference = root / 'late-reconstruction.yuv'
    rejected = run([args.driver, 16, 16, 10, 251, decoded_late, root / 'late.h264', late_reference,
                    root / 'late-own.yuv', 'true', 250], success=False)
    if rejected.returncode == 0 or late_reference.stat().st_size != 384 * 250 or b'MacroblockLimit' not in rejected.stderr:
        raise ValueError('late fixture did not hit the native coding limit after 250 pictures')
    first_only = root / 'first-only.mp4'
    run([tools.ffmpeg, '-v', 'error', '-i', late, '-frames:v', 1, '-c:v', 'copy', first_only])
    run([args.candidate, '--native-h264-encoder', 'ffmpeg', '-i', first_only,
         '-an', '-c:v', 'libx264', '-qp', 10, root / 'first-native.mp4'], env=native_only)
    destination = root / 'late-preserved.mp4'
    destination.write_bytes(b'preserved destination')
    before = {path.name for path in root.iterdir()}
    rejected = run([args.candidate, '--native-h264-encoder', 'ffmpeg', '-y', '-i', late,
                    '-an', '-c:v', 'libx264', '-qp', 10, destination], env=native_only, success=False)
    if rejected.returncode == 0:
        raise ValueError('native-only late coding limit unexpectedly succeeded')
    same(destination.read_bytes(), b'preserved destination', 'late-refusal destination preservation')
    same({path.name for path in root.iterdir()}, before, 'late-refusal staging cleanup')
    compare(late, ('-qp', '10'), 'late-coding-limit')

    for label, binary, command in [
        ('default-copy', args.baseline_default, ['-c', 'copy']),
        ('external', args.baseline_external, ['-an', '-c:v', 'libx264', '-crf', '23']),
    ]:
        outputs = []
        for index, executable in enumerate((binary, args.candidate)):
            output = root / f'{label}-{index}.mp4'
            run([executable, 'ffmpeg', '-i', source_path, *command, output], env=environment)
            outputs.append(output.read_bytes())
        same(*outputs, f'unchanged {label} bytes')
        if label == 'default-copy':
            output = root / 'default-current.mp4'
            run([args.default_candidate, 'ffmpeg', '-i', source_path, *command, output], env=environment)
            same(output.read_bytes(), outputs[0], 'default-build bytes against base')

    (root / 'summary.json').write_text(json.dumps(dict(native=rows, metadata=metadata_rows, legacy_native_cases=len(legacy_sources), legacy_native_shapes=[label for label, _ in legacy_sources], gop_frames=251, fallback_cases=fallback_cases,
                                                     default_bytes_unchanged=True), indent=2) + '\n')
    print(f'native CLI: {len(rows)} conformance cases, {len(metadata_rows)} exact rate/SAR cases, thread identity, fallback and default byte checks passed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('candidate', 'default-candidate', 'baseline-default', 'baseline-external', 'baseline-native', 'driver', 'ffmpeg', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    for name in ('candidate', 'default_candidate', 'baseline_default', 'baseline_external', 'baseline_native', 'driver', 'ffmpeg'):
        setattr(args, name, getattr(args, name).resolve())
    verify(args)


if __name__ == '__main__':
    main()
