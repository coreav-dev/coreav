#!/usr/bin/env python3
"""Generated CLI encoding, unchanged defaults, and exact external fallback."""
import argparse
from fractions import Fraction
import hashlib
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
import native_h264_matroska as matroska
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
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


def native_pair(candidate, baseline, command, *, env, group, label, groups):
    output = Path(command[-1])
    previous = output.with_name(output.stem + '.baseline' + output.suffix)
    for executable, destination in ((candidate, output), (baseline, previous)):
        try:
            run([executable, '--native-h264-encoder', 'ffmpeg', *command[:-1], destination], env=env)
        except ValueError as error:
            raise ValueError(f'actual-base/{group}/{label}: {error}') from error
    same(output.read_bytes(), previous.read_bytes(), f'actual-base/{group}/{label}/whole MP4 bytes')
    groups.setdefault(group, []).append(label)


def byte_groups(groups):
    return {name: dict(count=len(labels), cases=labels) for name, labels in groups.items()}


def deterministic_fallback(ffmpeg, directory):
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / 'ffmpeg'
    path.write_text(f'#!{sys.executable}\nimport os, sys\n'
                    f'os.execv({str(ffmpeg)!r}, [{str(ffmpeg)!r}, *sys.argv[1:-1], '
                    '"-fflags", "+bitexact", sys.argv[-1]])\n')
    path.chmod(0o755)
    return path


def require_fallback_success(status, contents, label):
    if status != 0 or contents == b'preserved destination':
        raise ValueError(f'fallback/{label}/expected successful oracle output replacing destination')


def oracle_rates(stream, rate, common):
    reported = {field: Fraction(stream[field]) for field in ('r_frame_rate', 'avg_frame_rate')}
    interpretation = 'nominal-frame-rate'
    for field, value in reported.items():
        if common:
            same(value, rate, f'oracle common-rate {field}')
        if (not common and field == 'r_frame_rate' and rate == Fraction(29, 7)
                and value == Fraction(58, 7) and reported['avg_frame_rate'] == rate):
            interpretation = 'known-29/7-sps-field-clock'
            continue
        if value <= 0 or abs(1 / value - 1 / rate) > Fraction(1, 1_000_000_000):
            raise ValueError(f'oracle {field} nominal period exceeds one-nanosecond bound')
    return dict(oracle_r_frame_rate=stream['r_frame_rate'],
                oracle_avg_frame_rate=stream['avg_frame_rate'], oracle_rate_interpretation=interpretation)


def unchanged_outputs(args, root, source_path, environment, fallback_environment, *, commands=None,
                      containers=('mp4', 'mkv')):
    if commands is None:
        commands = [('default-copy', ['-c', 'copy']),
                    ('external', ['-an', '-c:v', 'libx264', '-crf', '23'])]
    groups = {}
    for container in containers:
        for label, command in commands:
            for mode, candidate, baseline in (
                ('unselected', args.candidate, args.baseline_external),
                ('default', args.default_candidate, args.baseline_default),
            ):
                previous = root / f'{label}-{mode}-base.{container}'
                current = root / f'{label}-{mode}.{container}'
                selected_environment = fallback_environment if container == 'mkv' else environment
                for binary, output in ((baseline, previous), (candidate, current)):
                    run([binary, 'ffmpeg', '-i', source_path, *command, output], env=selected_environment)
                same(current.read_bytes(), previous.read_bytes(),
                     f'actual-base/{container}/{label}/{mode}/whole bytes')
                groups.setdefault(f'{container}-{mode}', []).append(label)
    return groups


def edited_opus_matroska(args, ffmpeg, ffprobe, root, source_path, command, environment):
    expected, facts = opus_source_pcm(source_path, root / 'opus-source-edit-reference', ffmpeg, ffprobe)
    for mode, candidate in (('unselected', args.candidate), ('default', args.default_candidate)):
        output = root / f'opus-source-edit-{mode}.mkv'
        run([candidate, 'ffmpeg', '-i', source_path, *command, output], env=environment)
        try:
            matroska.compare_opus_pcm(oracle_audio_pcm(ffmpeg, output, output.with_suffix('.opus.f32le')),
                                      expected, facts)
        except ValueError as error:
            raise ValueError(f'ordinary-matroska/opus-source-edit/{mode}: {error}') from error
    return dict(modes=['unselected', 'default'], declared_samples=facts['declared_samples'],
                start_samples=facts['start_samples'], pre_skip=facts['pre_skip'])


def copied_audio_sources(root, ffmpeg, video30, video25):
    sources = []
    for label, container, encoder, rate, offset in [
        ('audio-copy', 'mp4', 'aac', 30, '0'),
        ('audio-mp4-mp3', 'mp4', 'libmp3lame', 30, '0'),
        ('audio-mp4-opus', 'mp4', 'libopus', 30, '0'),
        ('audio-mkv-aac', 'mkv', 'aac', 25, '0'),
        ('audio-mkv-opus', 'mkv', 'libopus', 25, '0'),
        ('audio-offset', 'mp4', 'aac', 30, '0.04'),
        ('video-offset', 'mp4', 'aac', 30, '-0.04'),
    ]:
        path = root / f'{label}-source.{container}'
        run([ffmpeg, '-v', 'error', '-i', video25 if rate == 25 else video30, '-itsoffset', offset,
             '-f', 'lavfi', '-i', 'sine=frequency=997:sample_rate=48000:duration=0.16',
             '-map', '0:v:0', '-map', '1:a:0', '-c:v', 'copy', '-c:a', encoder,
             '-avoid_negative_ts', 'make_non_negative', path])
        sources.append((label, path, Fraction(rate)))
    return sources


def oracle_audio_pcm(ffmpeg, source, output, *, input_options=()):
    run([ffmpeg, '-v', 'error', '-xerror', *input_options, *AUDIO_DECODE_INPUT, '-i', source,
         '-map', '0:a:0', '-vn', '-c:a', 'pcm_f32le', '-f', 'f32le', output])
    return output.read_bytes()


def opus_source_pcm(source, prefix, ffmpeg, ffprobe):
    original = source.read_bytes()
    if source.suffix == '.mp4':
        facts = matroska.opus_mp4_source(source)
        decoder_input = prefix.with_suffix('.oracle-only.mp4')
        decoder_input.write_bytes(facts.pop('clone'))
        input_options = ['-ignore_editlist', '1']
        method = 'original-mp4-edit-over-zero-preskip-unedited-decode'
    else:
        facts = matroska.opus_matroska_source(original)
        decoder_input, input_options = source, []
        method = 'original-matroska-declared-presentation'
        readback = json.loads(run([ffprobe, '-v', 'error', '-select_streams', 'a:0',
                                   '-show_streams', '-show_packets', '-of', 'json', source]).stdout)
        stream, = readback['streams']
        same(len(readback['packets']), len(facts['packet_sha256']), 'source Opus: complete oracle packet readback')
        facts['reported_pts'] = [str(Fraction(packet['pts']) * Fraction(stream['time_base']))
                                 for packet in readback['packets']]
    decoded = prefix.with_suffix('.source.f32le')
    pcm = oracle_audio_pcm(ffmpeg, decoder_input, decoded, input_options=input_options)
    facts['oracle_decoded_samples'] = len(pcm) // (facts['channels'] * 4)
    expected = matroska.opus_presentation_slice(pcm, facts) if source.suffix == '.mp4' else pcm
    matroska.compare_opus_pcm(expected, expected, facts)
    same(source.read_bytes(), original, 'source Opus: original input bytes remain unchanged')
    facts['reference_method'] = method
    facts['expected_pcm_sha256'] = hashlib.sha256(expected).hexdigest()
    return expected, facts


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


def matroska_matrix(args, tools, root, source, native, native_only, source_path,
                    legacy_sources, gop_source):
    cases = []
    for index, (rate, sar) in enumerate([
        ('25', None), ('30', '1/1'), ('24000/1001', '12/11'), ('30000/1001', '16/15'),
        ('25/2', '4/3'), ('1000/33', '40/33'), ('60000/1001', '17/13'),
        ('29/7', '65535/65534'), ('1000', None),
    ]):
        extra = ('-vf', f'setsar={sar}:max=65535') if sar else ()
        path = source(f'mkv-rate-{index}', rate=rate, extra=extra)
        for mode, quality in (('crf', 23), ('qp', 28)):
            cases.append((f'rate-{index}-{mode}', path, Fraction(rate), Fraction(sar or 1),
                          18, 34, 4, ['-an', f'-{mode}', quality], index < 4))
    for mode, quality in [('crf', 10), ('crf', 23), ('crf', 51), ('qp', 10), ('qp', 28), ('qp', 51)]:
        cases.append((f'quality-{mode}-{quality}', source_path, Fraction(30), Fraction(1),
                      18, 34, 4, ['-an', f'-{mode}', quality], True))
    for label, path in legacy_sources:
        cases.append((f'legacy-{label}', path, Fraction(30), Fraction(1), 18, 34,
                      1 if label == 'one-zero-duration' else 4, ['-an'], True))
    for rate, sar in [('30000/1001', '16/15'), ('25', '17/13')]:
        path = source(f'mkv-single-{len(cases)}', rate=rate, frames=1,
                      extra=('-vf', f'setsar={sar}:max=65535'))
        cases.append((f'single-{len(cases)}', path, Fraction(rate), Fraction(sar),
                      18, 34, 1, ['-an'], True))
    cases.append(('gop-251', gop_source, Fraction(30), Fraction(1), 16, 16, 251, ['-an'], True))

    mkv_source = root / 'admitted-input.mkv'
    video25 = source('admitted-input-25', rate='25')
    run([tools.ffmpeg, '-v', 'error', '-i', video25, '-c', 'copy', '-fflags', '+bitexact', mkv_source])
    origin = source('source-origin', extra=('-output_ts_offset', '2'))
    audio_sources = copied_audio_sources(root, tools.ffmpeg, source_path, video25)
    opus_sources = {path: opus_source_pcm(path, root / f'original-{label}', tools.ffmpeg, tools.ffprobe)
                    for label, path, _ in audio_sources if label.endswith('-opus')}
    axes = [
        ('matroska-input', mkv_source, Fraction(25), ['-an']),
        ('source-origin', origin, Fraction(30), ['-an', '-fps_mode', 'passthrough']),
        ('mapped-video', source_path, Fraction(30), ['-map', '0:v:0', '-an']),
        ('remove-readd-map', source_path, Fraction(30), ['-map', '0', '-map', '-0:v', '-map', '0:v:0', '-an']),
        ('explicit-pixel-format', source_path, Fraction(30), ['-an', '-pix_fmt', 'yuv420p']),
    ]
    for label, path, rate in audio_sources:
        axes.append((label, path, rate, ['-c:a', 'copy', '-fps_mode', 'passthrough']))
    axes.append(('mapped-audio-copy', audio_sources[0][1], Fraction(30),
                 ['-map', '0:v:0', '-map', '0:a:0', '-c:a', 'copy', '-fps_mode', 'passthrough']))
    for label, path, rate, extra in axes:
        native(['-i', path, '-c:v:0', 'libx264', *extra, root / f'axis-{label}.mp4'], 'source-audio-selection', label)
        cases.append((f'axis-{label}', path, rate, Fraction(1), 18, 34, 4, extra, True))

    def probe(path, selection='v:0', hashes=False):
        return json.loads(run([tools.ffprobe, '-v', 'error', '-select_streams', selection,
                               '-show_streams', '-show_packets',
                               *(['-show_data_hash', 'sha256'] if hashes else []),
                               '-of', 'json', path]).stdout)

    rows = []
    for label, path, rate, sar, width, height, frames, extra, common in cases:
        identity = None
        opus_reference = opus_sources.get(path)
        for threads in ('1', '2', '8', '0', 'auto'):
            case = f'matroska/{label}/threads-{threads}'
            peer = root / f'mkv-{label}-{threads}.mp4'
            command = ['-i', path, '-c:v', 'libx264', *extra, '-threads', threads]
            native([*command, peer], 'matroska-peer', f'{label}-threads-{threads}')
            output = peer.with_suffix('.mkv')
            try:
                run([args.candidate, '--native-h264-encoder', 'ffmpeg', *command, output], env=native_only)
                contents = output.read_bytes()
                if identity is not None:
                    same(contents, identity, 'complete thread-independent Matroska bytes')
                identity = contents
                info = probe(peer)
                stream, = info['streams']
                times = [Fraction(p['pts']) * Fraction(stream['time_base']) for p in info['packets']]
                same(len(times), frames, 'same-head MP4 frame count')
                same([time - times[0] for time in times], [Fraction(i) / rate for i in range(frames)],
                     'same-head MP4 exact cadence')
                independent = matroska.validate(contents, rate=rate, sar=sar, width=width, height=height,
                                                times=times, sps_reader=sps_metadata, mp4=matroska.mp4_video(peer))
                same([i for i, p in enumerate(independent['packets']) if p['key']],
                     list(range(0, frames, 250)), '250-picture keyframe interval')
                stored = probe(output)
                stored_stream, = stored['streams']
                same(len(stored['packets']), frames, 'oracle Matroska frame count')
                same(Fraction(stored_stream['sample_aspect_ratio'].replace(':', '/')), sar,
                     'oracle Matroska sample aspect')
                same(Fraction(stored_stream['display_aspect_ratio'].replace(':', '/')),
                     Fraction(width, height) * sar, 'oracle Matroska display aspect')
                same([Fraction(p['pts']) * Fraction(stored_stream['time_base']) for p in stored['packets']],
                     [Fraction(p['pts'], 1000) for p in independent['packets']], 'oracle and EBML presentation clock')
                readback = oracle_rates(stored_stream, rate, common)
                if threads == '1':
                    decoded = []
                    for container in (peer, output):
                        destination = container.with_suffix(container.suffix + '.decoded.yuv')
                        run([tools.ffmpeg, '-v', 'error', '-xerror', '-i', container, '-an',
                             '-pix_fmt', 'yuv420p', '-fps_mode', 'passthrough', '-f', 'rawvideo', destination])
                        decoded.append(destination.read_bytes())
                    same(*decoded, 'same-head full pinned-decoder reconstruction')
                    same(len(decoded[0]), frames * width * height * 3 // 2, 'complete decoded picture bytes')
                    own = output.with_suffix('.own.yuv')
                    run([args.candidate, 'ffmpeg', '-i', output, '-an', '-c:v', 'rawvideo',
                         '-pix_fmt', 'yuv420p', '-f', 'rawvideo', own], env=native_only)
                    same(own.read_bytes(), decoded[0], 'complete native Matroska decoder reconstruction')
                    audio = [probe(container, 'a:0', True) for container in (peer, output)]
                    same([p['data_hash'] for p in audio[0]['packets']],
                         [p['data_hash'] for p in audio[1]['packets']], 'copied audio complete packet bytes')
                    if audio[0]['packets']:
                        clocks = [[Fraction(p['pts']) * Fraction(info['streams'][0]['time_base'])
                                   for p in info['packets']] for info in audio]
                        if opus_reference and path.suffix == '.mkv':
                            matroska.compare_opus_matroska_timeline(matroska.opus_matroska_source(contents),
                                                                  clocks[1], opus_reference[1])
                        else:
                            expected_clocks = opus_reference[1]['clocks'] if opus_reference else clocks[0]
                            same(clocks[1], [Fraction(matroska.round_timestamp(t * 1000), 1000) for t in expected_clocks],
                                 'copied audio original-source rounded PTS' if opus_reference else 'copied audio absolute rounded PTS')
                    if opus_reference:
                        expected_pcm, facts = opus_reference
                        same([p['data_hash'].split(':', 1)[1] for p in audio[1]['packets']],
                             facts['packet_sha256'], 'copied Opus complete original-source packet bytes')
                        actual_pcm = output.with_suffix('.opus.f32le')
                        matroska.compare_opus_pcm(oracle_audio_pcm(tools.ffmpeg, output, actual_pcm), expected_pcm, facts)
                opus_proof = {}
                if opus_reference:
                    facts = opus_reference[1]
                    opus_proof['source_opus_pcm'] = {key: value for key, value in facts.items() if key != 'clocks'}
                    opus_proof['source_opus_pcm']['verification'] = 'direct-decode' if threads == '1' else 'complete-thread-byte-identity'
                rows.append(dict(case=label, threads=threads, frames=frames, rate=str(rate), sar=str(sar),
                                 default_duration_ns=independent['duration'], common_rate=common, **readback, **opus_proof))
            except (ValueError, KeyError, StopIteration) as error:
                raise ValueError(f'{case}: {error}') from error
    return rows


def verify(args):
    tools = test_oracle.require(args.ffmpeg, args.ffmpeg.with_name('ffprobe'))
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    environment = {**os.environ, 'COREAV_X264': '/nonexistent/native-cli-x264',
                   'COREAV_FFMPEG': str(tools.ffmpeg), 'COREAV_FALLBACK': str(tools.ffmpeg),
                   'COREAV_STRICT_SAFE': '0'}
    native_only = {**environment, 'COREAV_FFMPEG': '/nonexistent/native-cli-ffmpeg',
                   'COREAV_FALLBACK': '/nonexistent/native-cli-fallback',
                   'PATH': '/nonexistent/native-cli-path'}
    baseline_groups = {}

    def native(command, group, label):
        native_pair(args.candidate, args.baseline_native, command, env=native_only,
                    group=group, label=label, groups=baseline_groups)

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
    fallback_environment = {**environment, 'COREAV_FALLBACK': str(deterministic_fallback(
        tools.ffmpeg, root / 'deterministic-fallback'))}
    unchanged_groups = unchanged_outputs(args, root, source_path, environment, fallback_environment)
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
            native(command, 'quality-and-threads', f'{mode}-{quality}-threads-{threads}')
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
                native(['-i', path, '-an', '-c:v', 'libx264', f'-{mode}', quality,
                        '-threads', threads, output], 'metadata-and-threads', f'{index}-{mode}-threads-{threads}')
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
        native(['-i', path, '-an', '-c:v', 'libx264', root / f'legacy-{label}.mp4'], 'legacy', label)

    flat = root / 'gop.yuv'
    flat.write_bytes(bytes([128]) * (384 * 251))
    gop_source = source('gop', width=16, height=16, frames=251, raw=flat)
    gop_output = root / 'gop-native.mp4'
    native(['-i', gop_source, '-an', '-c:v', 'libx264', gop_output], 'gop', '251-pictures')
    gop_packets = json.loads(run([tools.ffprobe, '-v', 'error', '-show_packets', '-of', 'json', gop_output]).stdout)['packets']
    same(len(gop_packets), 251, 'default-quality GOP picture count')
    same([i for i,p in enumerate(gop_packets) if 'K' in p['flags']], [0, 250], 'native IDR interval')
    gop_decoded = root / 'gop-decoded.yuv'
    run([tools.ffmpeg, '-v', 'error', '-xerror', '-i', gop_output, '-an', '-f', 'rawvideo', gop_decoded])
    same(gop_decoded.stat().st_size, 251 * 384, 'complete GOP decode')

    fallback_cases = []
    fallback_results = []

    def compare(source_path, extra, label, container='mp4'):
        comparison_environment = fallback_environment if container in ('mkv', 'webm') else environment
        if container in ('mkv', 'webm'):
            destination = root / f'excluded-{label}.{container}'
            destination.write_bytes(b'preserved destination')
            before = {path.name for path in root.iterdir()}
            result = run([args.candidate, '--native-h264-encoder', 'ffmpeg', '-y', '-i', source_path,
                          '-an', '-c:v', 'libx264', *extra, destination], env=native_only, success=False)
            if result.returncode == 0:
                raise ValueError(f'excluded/{label}/native-only unexpectedly succeeded')
            same(destination.read_bytes(), b'preserved destination', f'excluded/{label}/destination preservation')
            same({path.name for path in root.iterdir()}, before, f'excluded/{label}/staging cleanup')
        outputs = []
        statuses = []
        for selected in (False, True):
            output = root / f'fallback-{label}-{selected}.{container}'
            output.write_bytes(b'preserved destination')
            command = [args.candidate, *(['--native-h264-encoder'] if selected else []),
                       'ffmpeg', '-y', '-i', source_path, '-an', '-c:v', 'libx264', *extra, output]
            result = run(command, env=comparison_environment, success=False)
            statuses.append(result.returncode)
            outputs.append(output.read_bytes())
        same(statuses[1], statuses[0], f'fallback/{label}/exit status')
        same(outputs[1], outputs[0], f'complete fallback bytes ({label})')
        required_success = label in ('unsupported-matroska-option', 'matroska-high-rate', 'matroska-rounded-collision')
        if required_success:
            for mode, status, contents in zip(('unselected', 'selected'), statuses, outputs):
                require_fallback_success(status, contents, f'{label}/{mode}')
        receipt = dict(case=label, container=container, candidate_unselected=statuses[0],
                       candidate_selected=statuses[1], output_replaced=outputs[0] != b'preserved destination')
        for name, binary, selected in [('external-base', args.baseline_external, False),
                                       ('native-base', args.baseline_native, True)]:
            output = root / f'fallback-{label}-{name}.{container}'
            output.write_bytes(b'preserved destination')
            result = run([binary, *(['--native-h264-encoder'] if selected else []), 'ffmpeg',
                          '-y', '-i', source_path, '-an', '-c:v', 'libx264', *extra, output],
                         env=comparison_environment, success=False)
            same(result.returncode, statuses[0], f'actual-base/fallback/{label}/{name}/status')
            same(output.read_bytes(), outputs[0], f'actual-base/fallback/{label}/{name}/whole bytes')
            if required_success:
                require_fallback_success(result.returncode, output.read_bytes(), f'{label}/{name}')
            receipt[name] = result.returncode
        fallback_cases.append(label)
        fallback_results.append(receipt)

    options = [('-profile:v', 'high'), ('-profile:v', 'baseline'), ('-bf', '2'),
               ('-b:v', '100k'), ('-maxrate', '100k', '-bufsize', '200k'),
               ('-preset', 'fast'), ('-pix_fmt', 'yuv420p10le'), ('-pix_fmt', 'yuv444p'),
               ('-crf', '9'), ('-crf', '23.5'), ('-qp', '9'), ('-qp', '23.5'),
               ('-crf', '23', '-qp', '24'), ('-tune', 'film'), ('-g', '12'),
               ('-x264-params', 'cabac=1'), ('-vf', 'scale=32:32'), ('-s', '32x32')]
    for index, extra in enumerate(options):
        compare(source_path, extra, f'option-{index}')
    compare(source_path, ('-map', '0:v:0?'), 'optional-map')
    compare(source_path, ('-bf', '2'), 'unsupported-matroska-option', 'mkv')
    compare(source_path, (), 'webm', 'webm')
    high_rate = source('high-rate', rate='1001')
    compare(high_rate, (), 'matroska-high-rate', 'mkv')
    collision = source('rounded-collision', rate='2000', frames=4)
    compare(collision, (), 'matroska-rounded-collision', 'mkv')
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
    compare(variable, (), 'matroska-variable-clock', 'mkv')
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

    matroska_rows = matroska_matrix(args, tools, root, source, native, native_only,
                                    source_path, legacy_sources, gop_source)
    opus_edit_source = root / 'audio-mp4-opus-source.mp4'
    opus_edit_command = ['-c:v', 'libx264', '-c:a', 'copy', '-fps_mode', 'passthrough']
    for group, labels in unchanged_outputs(
        args, root, opus_edit_source, environment, fallback_environment,
        commands=[('opus-source-edit', opus_edit_command)], containers=('mp4',),
    ).items():
        unchanged_groups[group].extend(labels)
    ordinary_opus_edit = edited_opus_matroska(args, tools.ffmpeg, tools.ffprobe, root, opus_edit_source,
                                              opus_edit_command, fallback_environment)
    summary = dict(native=rows, metadata=metadata_rows, legacy_native_cases=len(legacy_sources),
                   legacy_native_shapes=[label for label, _ in legacy_sources], gop_frames=251,
                   fallback_cases=fallback_cases, default_bytes_unchanged=True,
                   fallback_results=fallback_results,
                   baseline_revision=args.baseline_revision,
                   actual_base_mp4_whole_bytes=byte_groups(baseline_groups),
                   actual_base_default_unselected_external_whole_bytes=byte_groups(unchanged_groups),
                   actual_base_fallback_whole_bytes=dict(count=len(fallback_cases) * 2, cases=fallback_cases),
                   matroska_fallback_oracle_output_flags=['-fflags', '+bitexact'],
                   matroska_fallback_oracle_contexts=['excluded-controls', 'default-unselected-external-byte-comparisons'],
                   source_opus_pcm=dict(cases=sum('source_opus_pcm' in row for row in matroska_rows),
                                        direct_decodes=sum(row.get('source_opus_pcm', {}).get('verification') == 'direct-decode'
                                                           for row in matroska_rows)),
                   native_matroska=matroska_rows,
                   ordinary_matroska_opus_edit=ordinary_opus_edit)
    (root / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(f'native CLI: {sum(map(len, baseline_groups.values()))} actual-base MP4 byte cases, '
          f'{sum(map(len, unchanged_groups.values()))} default/unselected/external byte cases, '
          f'{len(matroska_rows)} native Matroska cases; named groups in summary.json')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('candidate', 'default-candidate', 'baseline-default', 'baseline-external', 'baseline-native', 'driver', 'ffmpeg', 'output'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--baseline-revision', required=True)
    args = parser.parse_args()
    if not re.fullmatch('[0-9a-f]{40}', args.baseline_revision):
        parser.error('--baseline-revision must be the full SHA of the checkout used to build the baseline binaries')
    for name in ('candidate', 'default_candidate', 'baseline_default', 'baseline_external', 'baseline_native', 'driver', 'ffmpeg'):
        setattr(args, name, getattr(args, name).resolve())
    verify(args)


if __name__ == '__main__':
    main()
