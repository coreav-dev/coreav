#!/usr/bin/env python3
"""Encode feature-isolating H.264 streams with the pinned ffmpeg binary.

Each stream varies one coding feature so that the decode-time ratio
between two decoders can be read against that feature alone. The encoder
is used as a black box: only documented command-line options.
"""
import argparse
import json
import os
import subprocess

FFMPEG = os.environ.get('FFMPEG_BIN', '/opt/coreav-oracle-bin/ffmpeg')

# name -> extra encoder arguments. Everything else is held fixed.
VARIANTS = {
    # entropy coder
    'cabac':            ['-coder', '1'],
    'cavlc':            ['-coder', '0'],
    # deblocking
    'deblock_on':       ['-x264opts', 'deblock=0:0'],
    'deblock_off':      ['-x264opts', 'no-deblock'],
    # picture types
    'intra_only':       ['-g', '1'],
    'p_only':           ['-bf', '0'],
    'b_pyramid':        ['-bf', '3', '-x264opts', 'b-pyramid=normal'],
    # motion precision
    'subpel_full':      ['-x264opts', 'me=umh:subme=9'],
    'subpel_none':      ['-x264opts', 'me=dia:subme=0'],
    # transform
    'dct8x8_on':        ['-x264opts', '8x8dct=1'],
    'dct8x8_off':       ['-x264opts', '8x8dct=0'],
}

QPS = [18, 24, 30, 38, 45]
SIZES = [('416x240', 'qcif'), ('854x480', 'sd'), ('1280x720', '720p'),
         ('1920x1080', '1080p'), ('3840x2160', '4k')]


def encode(source, out, frames, extra, scale=None, qp=24):
    """Encode one variant. Returns its size in bytes."""
    argv = [FFMPEG, '-nostdin', '-v', 'error', '-y', '-i', source,
            '-frames:v', str(frames)]
    if scale:
        argv += ['-vf', f'scale={scale}']
    argv += ['-c:v', 'libx264', '-qp', str(qp), '-an', *extra,
             '-f', 'h264', out]
    subprocess.run(argv, check=True, stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL)
    return os.path.getsize(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--source', required=True, help='raw or coded source clip')
    ap.add_argument('--out', required=True, help='directory for the streams')
    ap.add_argument('--frames', type=int, default=300)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)

    made = []
    for name, extra in VARIANTS.items():
        path = os.path.join(args.out, f'{name}.h264')
        made.append({'name': name, 'axis': 'feature', 'path': path,
                     'bytes': encode(args.source, path, args.frames, extra)})
        print(f'{name:16} {made[-1]["bytes"]:>12,} bytes', flush=True)
    for qp in QPS:
        path = os.path.join(args.out, f'qp{qp}.h264')
        made.append({'name': f'qp{qp}', 'axis': 'qp', 'qp': qp, 'path': path,
                     'bytes': encode(args.source, path, args.frames, [], qp=qp)})
        print(f'{"qp"+str(qp):16} {made[-1]["bytes"]:>12,} bytes', flush=True)
    for scale, label in SIZES:
        path = os.path.join(args.out, f'size_{label}.h264')
        made.append({'name': f'size_{label}', 'axis': 'resolution',
                     'scale': scale, 'path': path,
                     'bytes': encode(args.source, path, args.frames, [],
                                     scale=scale)})
        print(f'{"size_"+label:16} {made[-1]["bytes"]:>12,} bytes', flush=True)

    with open(os.path.join(args.out, 'streams.json'), 'w') as handle:
        json.dump(made, handle, indent=2)
    print(f'\n{len(made)} streams')


if __name__ == '__main__':
    main()
