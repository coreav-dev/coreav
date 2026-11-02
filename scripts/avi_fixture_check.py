#!/usr/bin/env python3
"""Compare generated AVI fixtures with the shared pinned FFmpeg, without timing runs."""
import argparse
import json
import os
from pathlib import Path
import subprocess
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


def video_equal(candidate, reference, ffmpeg, ffprobe, env, scratch):
    documents = [decoded_output.probe(p, ffprobe, env) for p in (candidate, reference)]
    if any(d is None for d in documents):
        raise AssertionError('AVI video probe failed')
    left, right = documents
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
        video_equal(candidate, reference, opts.ffmpeg, opts.ffprobe, env, scratch)
        records.append({'codec': 'h264-copy', 'verdict': 'exact pixels, fields and timing'})
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
