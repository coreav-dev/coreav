"""Exercise both experimental hosts against decoded synthetic output."""

import argparse
import json
import os
from pathlib import Path

from av1_process_reference import probe, quality, run
import test_oracle


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', required=True)
    parser.add_argument('--production')
    parser.add_argument('--oracle', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    args = parser.parse_args()
    args.work.mkdir(exist_ok=False)
    try:
        oracle = test_oracle.require(args.oracle / 'ffmpeg', args.oracle / 'ffprobe')
    except test_oracle.QualificationError as error:
        raise SystemExit(str(error)) from error
    ffmpeg, ffprobe = str(oracle.ffmpeg), str(oracle.ffprobe)
    source = args.work / 'source.mp4'
    run([ffmpeg, '-v', 'error', '-f', 'lavfi', '-i', 'testsrc2=size=320x180:rate=25',
         '-t', '6.4', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-x264-params',
         'colorprim=bt709:transfer=bt709:colormatrix=bt709:range=tv', str(source)], 60)
    files = []
    for mode in ('0', '1'):
        destination = args.work / f'host-{mode}.mp4'
        environment = {**os.environ, 'COREAV_MEASURE_AV1_IN_PROCESS': mode}
        stdout, stderr, _ = run([args.coreav, 'transcode', '--video', 'av1', '--audio', 'none',
                                str(source), str(destination)], 60, environment)
        video = json.loads(stdout)['video']
        assert video['backend'] == 'native' and video['codec'] == 'h264'
        assert video['frames'] == 160 and video['encoding']['segments'] == 1
        expected = 'sandboxed' if mode == '0' else 'in-process-measurement'
        assert video['encoding']['isolation'] == expected
        assert b'coreav_measurement_decode ' in stderr
        observed = probe(ffprobe, destination)
        assert observed['frames'] == 160
        assert [observed['width'], observed['height']] == [320, 180]
        metric = quality(ffmpeg, source, destination, args.work / f'metric-{mode}.txt')
        assert metric['metric_frames'] == 160
        assert metric['psnr_db'] is None or metric['psnr_db'] >= 30
        files.append(destination.read_bytes())
    assert files[0] == files[1]
    if args.production:
        assert b'COREAV_MEASURE_AV1_IN_PROCESS' not in Path(args.production).read_bytes()
        destination = args.work / 'production.mp4'
        stdout, _, _ = run([args.production, 'transcode', '--video', 'av1', '--audio', 'none',
                            str(source), str(destination)], 60,
                           {**os.environ, 'COREAV_MEASURE_AV1_IN_PROCESS': '1'})
        encoding = json.loads(stdout)['video']['encoding']
        assert encoding['isolation'] == 'sandboxed' and encoding['segments'] == 2
    print('Both measurement hosts decoded 160 native H.264 frames in one session; outputs are identical')


if __name__ == '__main__':
    main()
