"""Check actual MP3 worker starts with the unchanged compatibility classifier."""
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
           'COREAV_FALLBACK': '/nonexistent/coreav-mp3-worker-fallback'}
    checked = 0
    try:
        for samples in (4608, 4992):
            for rate in (48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000):
                for channels in (1, 2):
                    source = args.work / f'source-{samples}-{rate}-{channels}.mp4'
                    run([oracle.ffmpeg, '-nostdin', '-v', 'error', '-f', 'lavfi', '-i',
                         f'sine=frequency=440:sample_rate={rate}', '-af', f'atrim=end_sample={samples}',
                         '-ac', channels, '-c:a', 'flac', source])
                    for mode, options in (('default', []), ('constant', ['-b:a', '128k']),
                                          ('quality', ['-q:a', '2.5'])):
                        name = f'{samples}-{rate}-{channels}-{mode}'
                        candidate, reference = [args.work / f'{name}-{side}.mp3'
                                                for side in ('candidate', 'reference')]
                        run([args.coreav, 'ffmpeg', '-i', source, *options, candidate], env)
                        starts = log.drain()
                        assert len(starts) == 2, (name, starts)
                        assert all(start['program'] == 'ffmpeg' and
                                   encode_breadth.mp3_encoder_start(start['args']) and
                                   encode_breadth.encoder_start(start['program'], start['args'])
                                   for start in starts), (name, starts)
                        rejected = [*starts[0]['args']]
                        rejected[rejected.index('mp3')] = 'matroska'
                        assert not encode_breadth.mp3_encoder_start(rejected)
                        run([oracle.ffmpeg, '-nostdin', '-v', 'error', '-i', source, *options, reference])
                        scratch = args.work / f'{name}-comparison'
                        scratch.mkdir()
                        verdict = decoded_output.compare(candidate, reference, oracle.ffmpeg,
                                                         oracle.ffprobe, os.environ, scratch,
                                                         exact_frame_timeline=True)
                        assert verdict == ('verified', 'exact decoded output'), (name, verdict)
                        checked += 1
    finally:
        log.close()
    print(f'{checked} MP3 commands: both encoder starts accepted; exact decoded output and timing')


if __name__ == '__main__':
    main()
