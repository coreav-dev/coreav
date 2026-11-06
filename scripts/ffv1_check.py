#!/usr/bin/env python3
"""Require exact FFV1 pixels and probe fields using the shared pinned oracle."""

import argparse
from dataclasses import dataclass
from fractions import Fraction
import json
import os
from pathlib import Path
import resource
import shlex
import signal
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_pin
import test_oracle

MAX_BYTES = 64 * 1024 * 1024
MAX_CASES = 128
TIMEOUT = 90
COLOR_FIELDS = ('color_range', 'color_space', 'color_transfer', 'color_primaries')
FORMATS = {'yuv420p': (2, 2), 'yuv422p': (2, 1), 'yuv444p': (1, 1)}
SCALE_NS = 1_000_000


class GateError(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise GateError(message)


def child_limit():
    resource.setrlimit(resource.RLIMIT_FSIZE, (MAX_BYTES, MAX_BYTES))


def run(argv, *, env=None, allow_failure=False, output_limit=MAX_BYTES):
    argv = [str(arg) for arg in argv]
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        child = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                 env=env, start_new_session=True, preexec_fn=child_limit)
        try:
            child.wait(timeout=TIMEOUT)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
            raise GateError(f'command timed out: {argv}') from None
        stdout.seek(0)
        stderr.seek(0)
        out, err = stdout.read(output_limit + 1), stderr.read(65537)
    require(len(out) <= output_limit and len(err) <= 65536, f'command output limit: {argv}')
    result = subprocess.CompletedProcess(argv, child.returncode, out, err)
    if not allow_failure:
        require(result.returncode == 0 and not err.strip(),
                f'command failed or emitted diagnostics ({result.returncode}): {argv}\n'
                f'{err.decode(errors="replace")}')
    return result


def verify_oracles(ffmpeg, ffprobe):
    ffmpeg, ffprobe = Path(ffmpeg).resolve(), Path(ffprobe).resolve()
    require(ffmpeg.name == 'ffmpeg' and ffprobe.name == 'ffprobe' and ffmpeg.parent == ffprobe.parent,
            'supply ffmpeg and ffprobe from the same pinned installation')
    try:
        oracle = test_oracle.require(ffmpeg, ffprobe)
    except test_oracle.QualificationError as error:
        raise GateError(str(error)) from error
    ffmpeg, ffprobe = oracle.ffmpeg, oracle.ffprobe
    for executable in (ffmpeg, ffprobe):
        run([executable, '-version'], output_limit=65536)
    return ffmpeg, ffprobe


def bounded_bytes(path, limit=MAX_BYTES):
    require(path.is_file() and path.stat().st_size <= limit, f'file absent or too large: {path}')
    with path.open('rb') as source:
        data = source.read(limit + 1)
    require(len(data) <= limit, f'file grew beyond limit: {path}')
    return data


def relative_file(directory, name):
    require(isinstance(name, str) and name and not Path(name).is_absolute(), 'fixture path must be relative')
    path = (directory / name).resolve()
    require(path.is_relative_to(directory.resolve()), 'fixture path escapes its directory')
    return path


@dataclass(frozen=True)
class Expected:
    name: str
    width: int
    height: int
    pix_fmt: str
    frames: int
    pts_ns: tuple
    duration_ns: tuple
    sample_aspect_ratio: str
    color: dict

    @property
    def frame_bytes(self):
        horizontal, vertical = FORMATS[self.pix_fmt]
        return self.width * self.height + 2 * ((self.width + horizontal - 1) // horizontal) * (
            (self.height + vertical - 1) // vertical)

    @classmethod
    def parse(cls, row):
        require(isinstance(row, dict), 'fixture must be an object')
        fields = cls.__dataclass_fields__
        require(set(fields) <= row.keys(), f'missing fixture fields: {set(fields) - row.keys()}')
        for field, maximum in (('width', 16384), ('height', 16384), ('frames', 256)):
            require(type(row[field]) is int and 0 < row[field] <= maximum, f'invalid {field}')
        require(isinstance(row['name'], str) and row['name'], 'missing fixture name')
        require(row['pix_fmt'] in FORMATS, 'unsupported fixture pixel format')
        require(row['sample_aspect_ratio'] in ('1:1', '4:3'), 'unsupported fixture aspect')
        require(isinstance(row['color'], dict) and set(row['color']) == set(COLOR_FIELDS),
                'colour expectations must explicitly name every field, including unknowns')
        require(all(value is None or isinstance(value, str) for value in row['color'].values()),
                'colour expectations must be probe strings or null')
        for key in ('pts_ns', 'duration_ns'):
            values = row[key]
            require(isinstance(values, list) and len(values) == row['frames'], f'invalid {key} count')
            require(all((value is None and key == 'duration_ns') or
                        (type(value) is int and value % SCALE_NS == 0 and
                         (value >= 0 if key == 'pts_ns' else value > 0)) for value in values),
                    f'{key} must describe exact stored ticks or explicit unknown durations')
        require(all(a < b for a, b in zip(row['pts_ns'], row['pts_ns'][1:])), 'non-increasing fixture timestamps')
        values = {key: row[key] for key in fields}
        values.update(pts_ns=tuple(row['pts_ns']), duration_ns=tuple(row['duration_ns']))
        expected = cls(**values)
        require(expected.frame_bytes * expected.frames <= MAX_BYTES, 'fixture raw size limit')
        return expected


def manifest(directory):
    document = json.loads(bounded_bytes(directory / 'manifest.json', 1024 * 1024))
    rows = document.get('cases') if isinstance(document, dict) else None
    require(isinstance(rows, list) and 0 < len(rows) <= MAX_CASES, 'invalid fixture case count')
    cases, names, paths = [], set(), set()
    total = 0
    for row in rows:
        expected = Expected.parse(row)
        require(expected.name not in names, 'duplicate fixture name')
        names.add(expected.name)
        media, raw = (relative_file(directory, row[key]) for key in ('file', 'raw'))
        require(media.suffix == '.mkv' and media != raw, 'fixture must have separate Matroska and raw files')
        require(media not in paths and raw not in paths, 'duplicate fixture file')
        paths.update((media, raw))
        for path in (media, raw):
            require(path.is_file() and path.stat().st_size <= MAX_BYTES, f'file absent or too large: {path}')
            total += path.stat().st_size
        require(total <= 1024 * 1024 * 1024, 'fixture set size limit')
        cases.append((expected, media, raw))
    return cases


def probe(ffprobe, path):
    result = run([ffprobe, '-v', 'error', '-show_streams', '-show_frames', '-of', 'json', path],
                 output_limit=4 * 1024 * 1024)
    return json.loads(result.stdout)


def compare_probe(expected, document):
    streams, frames = document.get('streams'), document.get('frames')
    require(isinstance(streams, list) and len(streams) == 1, f'{expected.name}: expected exactly one stream')
    stream = streams[0]
    aspect = Fraction(expected.sample_aspect_ratio.replace(':', '/'))
    display = aspect * Fraction(expected.width, expected.height)
    fields = {'codec_name': 'ffv1', 'codec_type': 'video', 'width': expected.width, 'height': expected.height,
              'pix_fmt': expected.pix_fmt, 'sample_aspect_ratio': expected.sample_aspect_ratio,
              'display_aspect_ratio': f'{display.numerator}:{display.denominator}',
              'field_order': 'progressive', 'time_base': '1/1000', **expected.color}
    for key, value in fields.items():
        require(stream.get(key) == value, f'{expected.name}: stream {key}: {stream.get(key)!r} != {value!r}')
    for key in ('bits_per_raw_sample', 'bits_per_sample'):
        value = stream.get(key)
        require(value in (None, 'N/A', 0, '0', 8, '8'), f'{expected.name}: wrong {key}: {value!r}')
    require(isinstance(frames, list) and len(frames) == expected.frames, f'{expected.name}: decoded frame count')
    for index, frame in enumerate(frames):
        fields = {'media_type': 'video', 'width': expected.width, 'height': expected.height,
                  'pix_fmt': expected.pix_fmt, 'key_frame': 1, 'interlaced_frame': 0}
        for key, value in fields.items():
            require(frame.get(key) == value, f'{expected.name}: frame {index} {key}')
        for key, nanoseconds in (('pts', expected.pts_ns[index]), ('duration', expected.duration_ns[index])):
            actual = frame.get(key)
            if nanoseconds is None:
                require(actual is None, f'{expected.name}: frame {index} unknown {key} was invented')
            else:
                require(type(actual) is int and actual * SCALE_NS == nanoseconds,
                        f'{expected.name}: frame {index} {key}: {actual!r} != {nanoseconds} ns')


def compare_raw(expected, original, decoded):
    size = expected.frame_bytes * expected.frames
    require(len(original) == size and len(decoded) == size,
            f'{expected.name}: complete raw byte count: {len(original)}, {len(decoded)} != {size}')
    for index in range(expected.frames):
        start, stop = index * expected.frame_bytes, (index + 1) * expected.frame_bytes
        require(original[start:stop] == decoded[start:stop], f'{expected.name}: pixels differ in frame {index}')


def decode(ffmpeg, path, expected):
    return run([ffmpeg, '-nostdin', '-v', 'error', '-xerror', '-err_detect', 'explode', '-threads', '1',
                '-i', path, '-map', '0:v:0', '-an', '-sn', '-dn', '-c:v', 'rawvideo',
                '-pix_fmt', expected.pix_fmt, '-threads', '1', '-fps_mode', 'passthrough',
                '-f', 'rawvideo', '-'], output_limit=expected.frame_bytes * expected.frames).stdout


def stored_ns(ticks, time_base):
    require(type(ticks) is int, 'source lacks an integer timestamp or duration')
    value = ticks * Fraction(time_base) * 1_000_000_000 / SCALE_NS
    return ((2 * value.numerator + value.denominator) // (2 * value.denominator)) * SCALE_NS


def source_expectation(name, document):
    [stream] = document['streams']
    frames = document['frames']
    require(stream['codec_name'] == 'h264' and stream['pix_fmt'] == 'yuv420p' and len(frames) == 3,
            'generated source is not the intended tiny lossy H.264 fixture')
    require(stream['width'] == 32 and stream['height'] == 24 and stream['sample_aspect_ratio'] == '4:3' and
            {key: stream.get(key) for key in COLOR_FIELDS} == {
                'color_range': 'tv', 'color_space': 'bt709', 'color_transfer': 'bt709', 'color_primaries': 'bt709'},
            'generated source lost its geometry, non-square aspect or colour declarations')
    require(stream.get('field_order') == 'progressive', 'generated source is not progressive')
    return Expected.parse({'name': name, 'width': stream['width'], 'height': stream['height'],
                           'pix_fmt': stream['pix_fmt'], 'frames': len(frames),
                           'pts_ns': [stored_ns(frame['pts'], stream['time_base']) for frame in frames],
                           'duration_ns': [stored_ns(frame['duration'], stream['time_base']) for frame in frames],
                           'sample_aspect_ratio': stream['sample_aspect_ratio'],
                           'color': {key: stream.get(key) for key in COLOR_FIELDS}})


def cli_cases(coreav, ffmpeg, ffprobe, work, records):
    marker = work / 'encoder-invoked'
    encoder = work / 'renamed-failing-encoder'
    encoder.write_text(f'#!/bin/sh\nprintf invoked > {shlex.quote(str(marker))}\nexit 99\n')
    encoder.chmod(0o755)
    fallback_log = work / 'fallback.jsonl'
    env = {**os.environ, 'COREAV_STRICT_SAFE': '0', 'COREAV_FFMPEG': str(encoder),
           'COREAV_X264': str(encoder), 'COREAV_FALLBACK': str(work / 'absent-fallback'),
           'COREAV_FALLBACK_LOG': str(fallback_log)}
    env.pop('COREAV_FALLBACK_ACTIVE', None)
    sources = []
    for container in ('mp4', 'mkv'):
        source = work / f'source.{container}'
        run([ffmpeg, '-nostdin', '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=32x24:rate=25',
             '-frames:v', '3', '-vf', 'setsar=4/3,setparams=range=limited:color_primaries=bt709:color_trc=bt709:colorspace=bt709',
             '-c:v', 'libx264', '-crf', '23', '-bf', '0',
             '-threads', '1', '-pix_fmt', 'yuv420p', '-color_range', 'tv', '-colorspace', 'bt709',
             '-color_trc', 'bt709', '-color_primaries', 'bt709', source])
        expected = source_expectation(container, probe(ffprobe, source))
        original = decode(ffmpeg, source, expected)
        sources.append(source)
        exclusions = ['-an', '-sn', '-dn']
        options = (exclusions, [*exclusions, '-map', '0:v:0', '-pix_fmt', expected.pix_fmt, '-f', 'matroska'],
                   [*exclusions, '-c:v', 'libx264', '-c:v:0', 'ffv1'],
                   [*exclusions, '-c:v:1', 'libx264', '-c:v:0', 'ffv1'], [])
        for index, extra in enumerate(options):
            output = work / f'cli-{container}-{index}.mkv'
            before = fallback_log.stat().st_size if fallback_log.exists() else 0
            case_env = {**env, 'COREAV_STRICT_SAFE': str(index % 2)}
            run([coreav, 'ffmpeg', '-i', source, '-c:v', 'ffv1', *extra, output], env=case_env)
            require((fallback_log.stat().st_size if fallback_log.exists() else 0) == before,
                    'native success went through fallback')
            compare_probe(expected, probe(ffprobe, output))
            compare_raw(expected, original, decode(ffmpeg, output, expected))
            records.append({'case': f'cli-{container}-{index}', 'result': 'exact'})

    def refuse(name, source, options=(), suffix='mkv', stdout=False, exclusions=('-an', '-sn', '-dn'),
               input_options=(), diagnostic=None):
        destination = work / f'refused-{name}.{suffix}'
        destination.write_bytes(b'preserved destination')
        result = run([coreav, 'ffmpeg', '-y', *input_options, '-i', source, *exclusions,
                      '-c:v', 'ffv1', *options, '-' if stdout else destination], env=env, allow_failure=True,
                     output_limit=65536)
        require(result.returncode == 4 and not result.stdout and result.stderr,
                f'{name}: expected unsupported refusal, got {result.returncode}: {result.stderr!r}')
        if diagnostic is not None:
            require(diagnostic in result.stderr, f'{name}: wrong refusal cause: {result.stderr!r}')
        require(destination.read_bytes() == b'preserved destination', f'{name}: destination replaced')
        records.append({'case': f'refused-{name}', 'result': 'refused, destination preserved'})

    source = sources[0]
    for suffix in ('mp4', 'webm', 'avi', 'mka', 'arbitrary'):
        refuse(f'container-{suffix}', source, ['-map', '0:v:0'], suffix=suffix)
    refuse('stdout', source, stdout=True)
    refuse('explicit-stdout', source, ['-f', 'matroska'], stdout=True)
    refuse('explicit-arbitrary', source, ['-f', 'matroska'], suffix='arbitrary')
    for name, options in (
        ('muxer', ['-f', 'mp4']), ('raw', ['-f', 'rawvideo']), ('image', ['-f', 'image2']),
        ('pixel-format', ['-pix_fmt', 'yuv444p']), ('filter', ['-vf', 'null']),
        ('resize', ['-s', '16x16']), ('rate', ['-r', '30']), ('seek', ['-ss', '0.04']),
        ('trim', ['-t', '0.04']), ('bitrate', ['-b:v', '1M']), ('quality', ['-crf', '12']),
        ('preset', ['-preset', 'fast']), ('tune', ['-tune', 'film']), ('qscale', ['-q:v', '2']),
    ):
        refuse(name, source, options)
    refuse('input-seek', source, input_options=['-ss', '0.04'])
    centered = work / 'centered.mkv'
    run([ffmpeg, '-nostdin', '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=32x24:rate=25',
         '-frames:v', '3', '-c:v', 'libx264', '-bf', '0', '-threads', '1', '-pix_fmt', 'yuv420p',
         '-chroma_sample_location', 'center', centered])
    observed = probe(ffprobe, centered)['frames']
    require(len(observed) == 3 and all(frame.get('chroma_location') == 'center' for frame in observed),
            'chroma-siting fixture did not retain centered samples')
    refuse('chroma-location', centered, diagnostic=b'unsupported chroma sample location')
    for pix_fmt in ('yuv420p10le', 'gbrp'):
        unsupported = work / f'source-{pix_fmt}.mkv'
        codec = 'libx264rgb' if pix_fmt == 'gbrp' else 'libx264'
        run([ffmpeg, '-nostdin', '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=32x24:rate=25',
             '-frames:v', '3', '-pix_fmt', 'rgb24' if pix_fmt == 'gbrp' else pix_fmt,
             '-c:v', codec, '-crf', '23', '-bf', '0',
             '-threads', '1', unsupported])
        [stream] = probe(ffprobe, unsupported)['streams']
        require(stream['pix_fmt'] == pix_fmt, 'unsupported-format fixture was converted')
        refuse(pix_fmt, unsupported)
    subtitle = work / 'subtitle.srt'
    subtitle.write_text('1\n00:00:00,000 --> 00:00:00,080\nFFV1 test\n')
    extras = work / 'extras.mkv'
    run([ffmpeg, '-nostdin', '-v', 'error', '-i', source, '-f', 'lavfi', '-i', 'sine=duration=0.12',
         '-i', subtitle, '-map', '0:v:0', '-map', '0:v:0', '-map', '1:a:0', '-map', '2:s:0',
         '-c:v', 'copy', '-c:a', 'pcm_s16le', '-c:s', 'srt', extras])
    for kind, options, exclusions in (
        ('audio', ['-map', '0:v:0', '-map', '0:a:0'], ['-sn', '-dn']),
        ('subtitle', ['-map', '0:v:0', '-map', '0:s:0'], ['-an', '-dn']),
        ('video', ['-map', '0:v'], ['-an', '-sn', '-dn']),
    ):
        refuse(f'extra-{kind}', extras, options, exclusions=exclusions)

    parts = []
    for width in (32, 64):
        part = work / f'changing-{width}.h264'
        run([ffmpeg, '-nostdin', '-v', 'error', '-f', 'lavfi', '-i', f'testsrc2=size={width}x24:rate=25',
             '-frames:v', '1', '-c:v', 'libx264', '-bf', '0', '-threads', '1', '-pix_fmt', 'yuv420p', part])
        parts.append(bounded_bytes(part))
    elementary = work / 'changing.h264'
    elementary.write_bytes(b''.join(parts))
    changing = work / 'changing.mkv'
    run([ffmpeg, '-nostdin', '-v', 'error', '-fflags', '+genpts', '-r', '25', '-i', elementary,
         '-map', '0:v:0', '-c:v', 'copy', changing])
    observed = probe(ffprobe, changing)['frames']
    require([(frame['width'], frame['height']) for frame in observed] == [(32, 24), (64, 24)],
            'mid-stream fixture did not change decoded geometry')
    destination = work / 'terminal-frame-change.mkv'
    destination.write_bytes(b'preserved destination')
    before = fallback_log.stat().st_size if fallback_log.exists() else 0
    result = run([coreav, 'ffmpeg', '-y', '-i', changing, '-an', '-sn', '-dn', '-c:v', 'ffv1', destination],
                 env=env, allow_failure=True, output_limit=65536)
    require(result.returncode == 1 and not result.stdout and
            b'video geometry, format or colour changes' in result.stderr,
            f'mid-stream frame change must be terminal: {result.returncode}: {result.stderr!r}')
    require(destination.read_bytes() == b'preserved destination', 'mid-stream failure replaced the destination')
    require((fallback_log.stat().st_size if fallback_log.exists() else 0) == before,
            'mid-stream failure attempted fallback')
    records.append({'case': 'terminal-frame-change', 'result': 'failed, destination preserved, no fallback'})

    parts = []
    for matrix in ('bt709', 'smpte170m'):
        part = work / f'colour-{matrix}.h264'
        run([ffmpeg, '-nostdin', '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=32x24:rate=25',
             '-frames:v', '3', '-colorspace', matrix, '-c:v', 'libx264', '-bf', '0', '-threads', '1',
             '-pix_fmt', 'yuv420p', part])
        parts.append(bounded_bytes(part))
    elementary = work / 'colour-changing.h264'
    elementary.write_bytes(b''.join(parts))
    changing = work / 'colour-changing.mkv'
    run([ffmpeg, '-nostdin', '-v', 'error', '-fflags', '+genpts', '-r', '25', '-i', elementary,
         '-map', '0:v:0', '-c:v', 'copy', changing])
    observed = probe(ffprobe, changing)['frames']
    require([frame.get('color_space') for frame in observed] ==
            ['bt709'] * 3 + ['smpte170m'] * 3, 'mid-stream fixture did not change decoded colour')
    require(all((frame.get('width'), frame.get('height'), frame.get('pix_fmt')) ==
                (32, 24, 'yuv420p') for frame in observed), 'colour-change fixture changed geometry or format')
    for strict in (False, True):
        destination = work / f'terminal-colour-change-{int(strict)}.mkv'
        destination.write_bytes(b'preserved destination')
        before = fallback_log.stat().st_size if fallback_log.exists() else 0
        result = run([coreav, *(['--strict-safe'] if strict else []), 'ffmpeg', '-y', '-i', changing,
                      '-an', '-sn', '-dn', '-c:v', 'ffv1', destination], env=env, allow_failure=True,
                     output_limit=65536)
        require(result.returncode == 1 and not result.stdout and
                (b'conflicting stream and frame colour' in result.stderr or
                 b'video geometry, format or colour changes' in result.stderr),
                f'mid-stream colour change must be terminal: {result.returncode}: {result.stderr!r}')
        require(destination.read_bytes() == b'preserved destination', 'mid-stream colour change replaced the destination')
        require((fallback_log.stat().st_size if fallback_log.exists() else 0) == before,
                'mid-stream colour change attempted fallback')
        records.append({'case': f'terminal-colour-change-{int(strict)}',
                        'result': 'failed, destination preserved, no fallback'})
    require(not marker.exists(), 'an external encoder was invoked')
    if fallback_log.exists():
        for line in bounded_bytes(fallback_log, 1024 * 1024).splitlines():
            record = json.loads(line)
            require(record.get('ran') is None and record.get('not_run'), 'fallback executed')
    require(not list(work.rglob('*.part')), 'unfinished output publication')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('ffmpeg', 'ffprobe', 'coreav', 'fixtures', 'work'):
        parser.add_argument('--' + name, type=Path, required=True)
    args = parser.parse_args(argv)
    args.work = args.work.resolve()
    args.work.mkdir(parents=True, exist_ok=False)
    records = []
    report = {'result': 'failed', 'cases': records}
    try:
        ffmpeg, ffprobe = verify_oracles(args.ffmpeg, args.ffprobe)
        require(args.coreav.is_file() and os.access(args.coreav, os.X_OK), 'CoreAV executable is absent')
        for expected, media, raw in manifest(args.fixtures.resolve()):
            compare_probe(expected, probe(ffprobe, media))
            compare_raw(expected, bounded_bytes(raw), decode(ffmpeg, media, expected))
            records.append({'case': expected.name, 'result': 'exact'})
        cli_cases(args.coreav.resolve(), ffmpeg, ffprobe, args.work, records)
        report['result'] = 'passed'
    except (GateError, OSError, ValueError, KeyError, TypeError) as error:
        report['error'] = str(error)
        print(f'ffv1_check: {error}', file=sys.stderr)
        return 1
    finally:
        (args.work / 'results.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f'ffv1_check: {len(records)} exact or refusal cases passed')
    return 0


if __name__ == '__main__':
    sys.exit(main())
