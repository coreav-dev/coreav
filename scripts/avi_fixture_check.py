#!/usr/bin/env python3
"""Compare generated AVI fixtures with the shared pinned FFmpeg, without timing runs."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import struct
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'tools/compat-corpus'))
import decoded_output
import oracle_pin
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT


def run(argv, env):
    result = subprocess.run([str(a) for a in argv], env=env, stdin=subprocess.DEVNULL,
                            capture_output=True, timeout=150, check=False)
    if result.returncode:
        raise RuntimeError(result.stderr.decode(errors='replace')[-2000:])
    return result.stdout


def riff_layout(path):
    data = path.read_bytes()
    result = []
    def walk(start, end, parent):
        at = start
        while at + 8 <= end:
            kind = data[at:at + 4].decode('ascii')
            size, = struct.unpack_from('<I', data, at + 4)
            stop = at + 8 + size
            assert stop <= end, (kind, size)
            name = parent + '/' + kind
            if kind == 'LIST':
                name += ':' + data[at + 8:at + 12].decode('ascii')
                walk(at + 12, stop, name)
            elif kind in ('avih', 'strh', 'idx1'):
                result.append({'name': name, 'offset': at, 'size': size,
                               'words': list(struct.unpack_from('<' + 'I' * (size // 4), data, at + 8))})
            elif kind == 'strf':
                result.append({'name': name, 'offset': at, 'size': size, 'hex': data[at + 8:stop].hex()})
            elif '/LIST:movi/' in name:
                result.append({'name': name, 'offset': at, 'size': size})
            at = stop + size % 2
    assert data[:4] == b'RIFF' and data[8:12] == b'AVI '
    walk(12, len(data), 'AVI')
    return result


def video_equal(candidate, reference, ffmpeg, ffprobe, env, scratch, record):
    documents = [decoded_output.probe(p, ffprobe, env) for p in (candidate, reference)]
    if any(d is None for d in documents):
        raise AssertionError('AVI video probe failed')
    left, right = documents
    record.update(candidate_probe=left, reference_probe=right,
                  candidate_layout=riff_layout(candidate), reference_layout=riff_layout(reference))
    print(json.dumps(record), flush=True)
    assert len(left['streams']) == len(right['streams']) == 1
    assert decoded_output.metadata(left['streams'][0], right['streams'][0]), (left, right)
    timelines = []
    for document in documents:
        stream = document['streams'][0]
        timelines.append([(decoded_output.time({**f, 'time_base': stream['time_base']}, 'best_effort_timestamp'),
                           decoded_output.time({**f, 'time_base': stream['time_base']}, 'duration'))
                          for f in document['frames']])
    assert timelines[0] == timelines[1], timelines
    for side, source in enumerate((candidate, reference)):
        run([ffmpeg, '-v', 'error', *AUDIO_DECODE_INPUT, '-i', source, '-map', '0:v',
             '-c:v', 'rawvideo', '-fps_mode', 'passthrough', '-f', 'rawvideo', scratch / str(side)], env)
    assert decoded_output.same_files(scratch / '0', scratch / '1'), 'decoded pixels'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('coreav', 'ffmpeg', 'ffprobe', 'work'):
        parser.add_argument('--' + name, type=Path, required=True)
    opts = parser.parse_args()
    opts.coreav, opts.ffmpeg, opts.ffprobe = (p.resolve() for p in (opts.coreav, opts.ffmpeg, opts.ffprobe))
    problems = list(oracle_pin.verify_bin(opts.ffmpeg.parent))
    if problems:
        raise RuntimeError('\n'.join(problems))
    opts.work.mkdir(parents=True, exist_ok=True)
    env = {**os.environ, 'COREAV_FFMPEG': str(opts.ffmpeg), 'COREAV_FALLBACK': '/nonexistent/coreav-fallback',
           'COREAV_STRICT_SAFE': '0', 'COREAV_FALLBACK_LOG': str(opts.work / 'fallback.jsonl')}
    records = []
    try:
        for rate, channels in ((44100, 1), (48000, 2), (22050, 1)):
            directory = opts.work / f'{rate}-{channels}'
            directory.mkdir()
            source = directory / 'source.mp4'
            run([opts.ffmpeg, '-v', 'error', '-f', 'lavfi', '-i', f'sine=frequency=997:sample_rate={rate}:duration=0.23',
                 '-ac', channels, '-c:a', 'flac', '-strict', '-2', source], env)
            for codec in ('pcm_s16le', 'pcm_u8', 'libmp3lame'):
                candidate, reference = (directory / f'{codec}-{side}.avi' for side in ('candidate', 'reference'))
                args = ['-i', source, '-vn', '-c:a', codec]
                run([opts.ffmpeg, '-v', 'error', *AUDIO_DECODE_INPUT, *args, reference], env)
                run([opts.coreav, 'ffmpeg', *args, candidate], env)
                scratch = directory / codec
                scratch.mkdir()
                verdict = decoded_output.compare(candidate, reference, str(opts.ffmpeg), str(opts.ffprobe), env, scratch)
                records.append({'rate': rate, 'channels': channels, 'codec': codec, 'verdict': verdict})
                assert verdict[0] == 'verified', records[-1]
        directory = opts.work / 'copy-video'
        directory.mkdir()
        source, candidate, reference = (directory / p for p in ('source.mp4', 'candidate.avi', 'reference.avi'))
        run([opts.ffmpeg, '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=64x48:rate=25:duration=0.24',
             '-c:v', 'libx264', '-bf', '0', '-an', source], env)
        args = ['-i', source, '-c', 'copy']
        run([opts.ffmpeg, '-v', 'error', *args, reference], env)
        run([opts.coreav, 'ffmpeg', *args, candidate], env)
        scratch = directory / 'decode'
        scratch.mkdir()
        record = {'codec': 'h264-copy', 'verdict': 'pending'}
        records.append(record)
        video_equal(candidate, reference, opts.ffmpeg, opts.ffprobe, env, scratch, record)
        record['verdict'] = 'exact pixels, fields and timing'
        existing = directory / 'existing.avi'
        existing.write_bytes(b'preserved')
        for args in (['-i', source, '-c:v', 'huffyuv'], ['-i', source], ['-i', source, '-c', 'copy', '-ss', '0.1']):
            result = subprocess.run([str(opts.coreav), 'ffmpeg', '-y', *map(str, args), str(existing)],
                                    env=env, capture_output=True, timeout=10, check=False)
            assert result.returncode == 4 and existing.read_bytes() == b'preserved', result.stderr
        explicit = subprocess.run([str(opts.coreav), '--strict-safe', 'ffmpeg', '-i', '/nonexistent/input.mp4',
                                   '-c:a', 'libmp3lame', str(existing)], env=env, capture_output=True, timeout=10, check=False)
        assert explicit.returncode == 4 and b'strict-safe' in explicit.stderr, explicit.stderr
        assert not list(opts.work.rglob('*.part')), 'unfinished atomic output'
        print(json.dumps(records, indent=2))
    finally:
        (opts.work / 'results.json').write_text(json.dumps(records, indent=2) + '\n')


if __name__ == '__main__':
    main()
