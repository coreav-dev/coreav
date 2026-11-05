#!/usr/bin/env python3
"""Check sandboxed AV1 output with the pinned oracle and CoreAV's AV1 decoder."""

import argparse
import json
import math
from pathlib import Path
import subprocess
import sys

import oracle_pin


FRAMES = 36


def run(command):
    return subprocess.run([str(value) for value in command], check=True, capture_output=True,
                          timeout=180).stdout


def check(coreav, oracle, work, moving=False):
    problems = list(oracle_pin.verify_bin(oracle))
    if problems:
        raise ValueError('\n'.join(problems))
    work.mkdir(parents=True, exist_ok=True)
    ffmpeg = oracle / 'ffmpeg'
    source = work / 'source.mp4'
    encoded = work / 'encoded.mp4'
    reference = work / 'reference.yuv'
    decoded = work / 'oracle.yuv'
    own = work / 'coreav.yuv'
    width, height, rate, frames, preset = (256, 256, 24, 48, 0) if moving else (128, 96, 6, FRAMES, 8)
    pattern = f'testsrc2=size={width}x{height}:rate={rate}'
    if moving:
        pattern += ',scroll=horizontal=0.03:vertical=0.02'
    run([ffmpeg, '-v', 'error', '-nostdin', '-y', '-f', 'lavfi', '-i',
         pattern, '-frames:v', str(frames), '-c:v', 'libx264',
         '-preset', 'ultrafast', '-crf', '18', '-pix_fmt', 'yuv420p',
         '-x264-params', 'colorprim=bt709:transfer=bt709:colormatrix=bt709',
         '-color_primaries', 'bt709', '-color_trc', 'bt709', '-colorspace', 'bt709',
         '-color_range', 'tv', source])
    run([ffmpeg, '-v', 'error', '-nostdin', '-y', '-i', source, '-an',
         '-pix_fmt', 'yuv420p', '-f', 'rawvideo', reference])
    receipt = json.loads(run([coreav, 'transcode', source, encoded, '--force',
                             '--video', 'av1', '--preset', str(preset), '--crf', '32']))
    if receipt.get('schema') != 'coreav.transcode/2':
        raise ValueError('explicit AV1 transcode did not return its v2 receipt')
    if receipt['video']['encoding']['segments'] < (1 if moving else 2):
        raise ValueError('the AV1 fixture did not cross a bounded-session boundary')
    probe = json.loads(run([oracle / 'ffprobe', '-v', 'error', '-count_frames',
                           '-select_streams', 'v:0', '-show_entries',
                           'stream=codec_name,width,height,pix_fmt,nb_read_frames',
                           '-of', 'json', encoded]))
    expected = {'codec_name': 'av1', 'width': width, 'height': height,
                'pix_fmt': 'yuv420p', 'nb_read_frames': str(frames)}
    if probe.get('streams') != [expected]:
        raise ValueError('AV1 output frame count, dimensions or pixel format changed')
    run([ffmpeg, '-v', 'error', '-nostdin', '-y', '-c:v', 'libaom-av1', '-i', encoded,
         '-an', '-fps_mode', 'passthrough', '-pix_fmt', '+yuv420p', '-f', 'rawvideo', decoded])
    run([coreav, 'ffmpeg', '-y', '-i', encoded, '-an', '-pix_fmt', 'yuv420p',
         '-f', 'rawvideo', own])
    original = reference.read_bytes()
    pixels = decoded.read_bytes()
    if len(original) != frames * width * height * 3 // 2 or len(pixels) != len(original):
        raise ValueError('decoder returned an incomplete or extra frame')
    if own.read_bytes() != pixels:
        raise ValueError('CoreAV and the oracle disagree on decoded AV1 pixels')
    squared_error = sum((left - right) ** 2 for left, right in zip(original, pixels, strict=True))
    psnr = math.inf if squared_error == 0 else 10 * math.log10(255 ** 2 * len(pixels) / squared_error)
    if psnr < 20:
        raise ValueError(f'AV1 fixture PSNR {psnr:.2f} dB is below the 20 dB sanity floor')
    print(json.dumps({'frames': frames, 'width': width, 'height': height, 'pix_fmt': 'yuv420p',
                      'preset': preset,
                      'psnr_db': round(psnr, 3) if math.isfinite(psnr) else 'inf'}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--coreav', type=Path, required=True)
    parser.add_argument('--oracle', type=Path, required=True)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--moving', action='store_true')
    options = parser.parse_args()
    try:
        check(options.coreav.resolve(), options.oracle.resolve(), options.work.resolve(), options.moving)
    except subprocess.CalledProcessError as error:
        print(error.stderr.decode(errors='replace'), file=sys.stderr)
        return 1
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
