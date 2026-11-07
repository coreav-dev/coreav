"""Check actual AAC worker starts with the unchanged compatibility classifier."""
import argparse
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(ROOT / 'tools/compat-corpus'), str(ROOT / 'scripts')]
import decoded_output
import encode_breadth
import test_oracle


def run(args, env=None):
    result = subprocess.run(list(map(str, args)), env=env, capture_output=True, timeout=60)
    if result.returncode:
        raise AssertionError(f'command exited {result.returncode}: {result.stderr.decode(errors="replace")}')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    args = parser.parse_args()
    oracle = test_oracle.require()
    args.work.mkdir(parents=True, exist_ok=True)
    log = encode_breadth.ProgramLog(args.work / 'programs', oracle.ffmpeg, oracle.ffmpeg)
    env = {**os.environ, 'COREAV_FFMPEG': str(log.wrappers['ffmpeg']),
           'COREAV_FALLBACK': '/nonexistent/coreav-aac-worker-fallback',
           'COREAV_STRICT_SAFE': '0'}
    checked = 0
    try:
        for rate, channels in ((48000, 1), (44100, 2)):
            source = args.work / f'source-{rate}-{channels}.mp4'
            run([oracle.ffmpeg, '-nostdin', '-v', 'error', '-f', 'lavfi', '-i',
                 f'sine=frequency=440:sample_rate={rate}', '-af', 'atrim=end_sample=4992',
                 '-ac', channels, '-c:a', 'flac', source])
            for mode, options, extension in (
                    ('implicit', [], 'mp4'), ('explicit', ['-c:a', 'aac'], 'mp4'),
                    ('audio-only', ['-vn', '-c:a', 'aac', '-b:a', '96k'], 'm4a')):
                name = f'{rate}-{channels}-{mode}'
                candidate, reference = [args.work / f'{name}-{side}.{extension}'
                                        for side in ('candidate', 'reference')]
                run([args.coreav, 'ffmpeg', '-i', source, *options, candidate], env)
                starts = log.drain()
                assert len(starts) == 2, (name, starts)
                assert all(start['program'] == 'ffmpeg' and
                           encode_breadth.aac_encoder_start(start['args']) and
                           encode_breadth.encoder_start(start['program'], start['args'])
                           for start in starts), (name, starts)
                run([oracle.ffmpeg, '-nostdin', '-v', 'error', '-i', source, *options, reference])
                scratch = args.work / f'{name}-comparison'
                scratch.mkdir()
                verdict = decoded_output.compare(candidate, reference, oracle.ffmpeg,
                                                 oracle.ffprobe, os.environ, scratch,
                                                 registered_codecs=('aac',),
                                                 exact_frame_timeline=True)
                assert verdict == ('verified', 'exact decoded output'), (name, verdict)
                checked += 1
    finally:
        log.close()
    print(f'{checked} AAC commands: both encoder starts accepted; exact decoded output and timing')


if __name__ == '__main__':
    main()
