#!/usr/bin/env python3
"""Development check of the x264 program's input depth (docs/measurements/transcode-x264-10bit.md, "Development
check"): which depth the program reads 10-bit frames at fastest, and `coreav transcode` before and after the change.

    ab.py --bin DIR --files DIR --out DIR --tmp /dev/shm/ab [--reps 3]

DIR holds ffmpeg, x264, coreav-old and coreav-new. Needs python3-numpy for the shifted inputs. Uses profile.py's
timer (every thread's CPU time from /proc). Results in OUT/ab.jsonl.
"""
import argparse, json, os, pathlib, subprocess, sys

import numpy

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from profile import timed  # noqa: E402

SOURCE = 'e97b74358172'
SIZE = (3840, 2160)
RATE = '24/1'
# name, ffmpeg pix_fmt, x264 csp, x264 input depth, shift applied to the 10-bit samples (None: ffmpeg's 16-bit form)
DEPTHS = [
    ('420-d10', 'yuv420p10le', 'i420', 10, 0),
    ('420-d12', 'yuv420p10le', 'i420', 12, 2),
    ('420-d14', 'yuv420p10le', 'i420', 14, 4),
    ('420-d16', 'yuv420p10le', 'i420', 16, 6),
    ('420-d16-ffmpeg', 'yuv420p16le', 'i420', 16, None),
    ('422-d10', 'yuv422p10le', 'i422', 10, 0),
    ('422-d16', 'yuv422p10le', 'i422', 16, 6),
]
TRANSCODE = ['e97b74358172', '199f46a5dada', 'c39b69673584']


def main():
    ap = argparse.ArgumentParser()
    for name in ('--bin', '--files', '--out', '--tmp'):
        ap.add_argument(name, required=True)
    ap.add_argument('--reps', type=int, default=3)
    a = ap.parse_args()
    b = pathlib.Path(a.bin).resolve()
    out, tmp = pathlib.Path(a.out), pathlib.Path(a.tmp)
    out.mkdir(parents=True, exist_ok=True)
    tmp.mkdir(parents=True, exist_ok=True)
    files = {p.name[:12]: p.resolve() for p in pathlib.Path(a.files).iterdir()}
    res = open(out / 'ab.jsonl', 'a')

    def emit(rec):
        res.write(json.dumps(rec) + '\n')
        res.flush()
        print(json.dumps(rec)[:300], flush=True)

    raw = tmp / 'raw.yuv'
    inputs = {}
    for name, pix_fmt, csp, depth, shift in DEPTHS:
        subprocess.run([str(b / 'ffmpeg'), '-nostdin', '-v', 'error', '-i', str(files[SOURCE]), '-map', '0:v:0', '-f',
                        'rawvideo', '-pix_fmt', pix_fmt, '-y', str(raw)], check=True)
        if shift:
            samples = numpy.fromfile(raw, dtype='<u2')
            (samples << shift).astype('<u2').tofile(raw)
        cmd = [str(b / 'x264'), '--demuxer', 'raw', '--input-csp', csp, '--output-csp', csp, '--input-depth', str(depth),
               '--output-depth', '10', '--input-res', f'{SIZE[0]}x{SIZE[1]}', '--fps', RATE, '--preset', 'medium',
               '--crf', '23', '--log-level', 'error', '--no-progress', '--muxer', 'raw', '--output',
               str(tmp / 'out.264'), '-']
        for r in range(a.reps):
            rec = timed(cmd, stdin=raw)
            rec['sha256'] = subprocess.run(['sha256sum', str(tmp / 'out.264')], capture_output=True,
                                           text=True).stdout[:16]
            emit(dict(rec, kind='depth', arm=name, rep=r))
        raw.unlink()

    env = dict(os.environ, PATH=f'{b}:{os.environ["PATH"]}')
    for src in TRANSCODE:
        def coreav(build, encoder, name):
            return [str(b / build), 'transcode', str(files[src]), str(tmp / f'{name}.mp4'), '--preset', 'medium',
                    '--crf', '23', '--audio', 'none', '--encoder', encoder, f'--{encoder}', str(b / encoder),
                    '--force']
        cmds = {
            'old-x264': coreav('coreav-old', 'x264', 'a'),
            'new-x264': coreav('coreav-new', 'x264', 'b'),
            'new-ffmpeg': coreav('coreav-new', 'ffmpeg', 'c'),
            'baseline': [str(b / 'ffmpeg'), '-nostdin', '-noautorotate', '-flags', '+unaligned', '-i', str(files[src]),
                         '-map', '0:v:0', '-an', '-c:v', 'libx264', '-preset', 'medium', '-crf', '23', '-fps_mode',
                         'passthrough', '-y', str(tmp / 'd.mp4')],
        }
        order = list(cmds)
        for r in range(a.reps):
            for arm in order[r % 4:] + order[:r % 4]:
                rec = timed(cmds[arm], env=env)
                if arm.endswith('x264'):
                    video = subprocess.run([str(b / 'ffmpeg'), '-v', 'error', '-i', str(tmp / f'{"a" if arm == "old-x264" else "b"}.mp4'),
                                            '-map', '0:v', '-c', 'copy', '-f', 'h264', '-'], capture_output=True).stdout
                    rec['video_sha256'] = __import__('hashlib').sha256(video).hexdigest()[:16]
                emit(dict(rec, kind='transcode', case=src, arm=arm, rep=r))
    return 0


if __name__ == '__main__':
    sys.exit(main())
