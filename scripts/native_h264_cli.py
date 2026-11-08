#!/usr/bin/env python3
"""Generated CLI encoding, unchanged defaults, and exact external fallback."""
import argparse
from fractions import Fraction
import json
import os
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import h264_crf_conformance as crf
import h264_p_conformance as syntax
import test_oracle


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
                             ('rate24', dict(rate='24')), ('rate2997', dict(rate='30000/1001')),
                             ('sar', dict(extra=('-vf', 'setsar=4/3'))),
                             ('depth10', dict(extra=('-pix_fmt', 'yuv420p10le'))),
                             ('chroma444', dict(extra=('-pix_fmt', 'yuv444p'))),
                             ('hdr', dict(extra=('-x264-params', 'colorprim=bt709:transfer=smpte2084:colormatrix=bt709'))),
                             ('interlace', dict(width=32, height=32, extra=('-flags', '+ildct+ilme', '-x264-params', 'tff=1')))]:
        compare(source(name, **parameters), (), name)

    variable = source('variable-rate', extra=('-vf', 'setpts=if(gte(N\\,2)\\,N+1\\,N)/30/TB',
                                              '-fps_mode', 'passthrough'))
    compare(variable, ('-fps_mode', 'passthrough'), 'variable-rate')

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

    (root / 'summary.json').write_text(json.dumps(dict(native=rows, gop_frames=251, fallback_cases=fallback_cases,
                                                     default_bytes_unchanged=True), indent=2) + '\n')
    print(f'native CLI: {len(rows)} conformance cases, thread identity, fallback and default byte checks passed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('candidate', 'default-candidate', 'baseline-default', 'baseline-external', 'driver', 'ffmpeg', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args()
    for name in ('candidate', 'default_candidate', 'baseline_default', 'baseline_external', 'driver', 'ffmpeg'):
        setattr(args, name, getattr(args, name).resolve())
    verify(args)


if __name__ == '__main__':
    main()
