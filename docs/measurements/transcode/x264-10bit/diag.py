#!/usr/bin/env python3
"""Why `coreav transcode` did not get the x264 program's faster 16-bit input (docs/measurements/transcode-x264-10bit.md,
"Development check"): the same frames to the same x264 command, from a file and through pipes of different sizes.

    diag.py --bin DIR --files DIR --out DIR --tmp /dev/shm/diag [--reps 2]
"""
import argparse, fcntl, json, os, pathlib, subprocess, sys, time

import numpy

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from profile import timed  # noqa: E402

F_SETPIPE_SZ = 1031
SOURCE = 'e97b74358172'


def feed(argv):
    """feed.py-style entry: diag.py --feed PIPE_BYTES CHUNK RAW -- CMD...: RAW written to CMD's standard input."""
    size, chunk, raw = int(argv[0]), int(argv[1]), argv[2]
    cmd = argv[4:]
    r, w = os.pipe()
    if size:
        fcntl.fcntl(w, F_SETPIPE_SZ, size)
    p = subprocess.Popen(cmd, stdin=r, stdout=subprocess.DEVNULL)
    os.close(r)
    with open(raw, 'rb') as f:
        while True:
            b = f.read(chunk)
            if not b:
                break
            os.write(w, b) if len(b) < 65536 else _write_all(w, b)
    os.close(w)
    return p.wait()


def _write_all(fd, b):
    view = memoryview(b)
    while view:
        n = os.write(fd, view)
        view = view[n:]


def main():
    if len(sys.argv) > 1 and sys.argv[1] == '--feed':
        return feed(sys.argv[2:])
    ap = argparse.ArgumentParser()
    for name in ('--bin', '--files', '--out', '--tmp'):
        ap.add_argument(name, required=True)
    ap.add_argument('--reps', type=int, default=2)
    a = ap.parse_args()
    b = pathlib.Path(a.bin).resolve()
    out, tmp = pathlib.Path(a.out), pathlib.Path(a.tmp)
    out.mkdir(parents=True, exist_ok=True)
    tmp.mkdir(parents=True, exist_ok=True)
    files = {p.name[:12]: p.resolve() for p in pathlib.Path(a.files).iterdir()}
    res = open(out / 'diag.jsonl', 'a')
    raw = {10: tmp / 'raw10.yuv', 16: tmp / 'raw16.yuv'}
    subprocess.run([str(b / 'ffmpeg'), '-nostdin', '-v', 'error', '-i', str(files[SOURCE]), '-map', '0:v:0', '-f',
                    'rawvideo', '-pix_fmt', 'yuv422p10le', '-y', str(raw[10])], check=True)
    (numpy.fromfile(raw[10], dtype='<u2') << 6).astype('<u2').tofile(raw[16])
    tc = tmp / 'tc.txt'
    tc.write_text('# timecode format v2\n' + ''.join(f'{i * 1000 / 24:.6f}\n' for i in range(129)))

    def x264(depth, extra=(), src='-'):
        return ([str(b / 'x264'), '--demuxer', 'raw', '--input-csp', 'i422', '--output-csp', 'i422', '--input-depth',
                 str(depth), '--output-depth', '10', '--input-res', '3840x2160'] + list(extra) +
                ['--preset', 'medium', '--crf', '23', '--log-level', 'error', '--no-progress', '--muxer', 'raw',
                 '--output', str(tmp / 'o.264'), src])

    fps = ['--fps', '24/1']
    coreav = ['--tcfile-in', str(tc), '--colorprim', 'bt709', '--transfer', 'bt709', '--colormatrix', 'bt709']
    me = [sys.executable, str(pathlib.Path(__file__).resolve()), '--feed']
    cases = {}
    for d in (10, 16):
        cases[f'd{d}-stdin-file'] = (x264(d, fps), raw[d])
        cases[f'd{d}-path'] = (x264(d, fps, str(raw[d])), None)
        cases[f'd{d}-pipe-64k'] = (me + ['0', str(1 << 20), str(raw[d]), '--'] + x264(d, fps), None)
        cases[f'd{d}-pipe-1m'] = (me + [str(1 << 20), str(1 << 20), str(raw[d]), '--'] + x264(d, fps), None)
        cases[f'd{d}-pipe-64k-coreav-flags'] = (me + ['0', str(1 << 20), str(raw[d]), '--'] + x264(d, coreav), None)
    for r in range(a.reps):
        for name, (cmd, stdin) in cases.items():
            rec = timed(cmd, stdin=stdin)
            rec['sha256'] = subprocess.run(['sha256sum', str(tmp / 'o.264')], capture_output=True, text=True).stdout[:16]
            rec.update(case=name, rep=r)
            res.write(json.dumps(rec) + '\n')
            res.flush()
            print(json.dumps(rec)[:240], flush=True)
    for name, cmd, stdin in (('d16-stdin-file', x264(16, fps + ['--frames', '10']), raw[16]),
                             ('d16-path', x264(16, fps + ['--frames', '10'], str(raw[16])), None)):
        with open(stdin, 'rb') if stdin else open(os.devnull, 'rb') as inp:
            subprocess.run(['strace', '-f', '-e', 'trace=read,mmap,munmap', '-o', str(tmp / 'strace.txt')] + cmd,
                           stdin=inp, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=600)
        calls = {}
        for line in (tmp / 'strace.txt').read_text().splitlines():
            call = line.split(' ', 1)[-1].split('(', 1)[0]
            big = 'read' in call and line.rstrip().split('= ')[-1].isdigit() and int(line.rstrip().split('= ')[-1]) >= 1 << 20
            key = call + (' >=1MiB' if big else '')
            calls[key] = calls.get(key, 0) + 1
        res.write(json.dumps({'case': name, 'strace_counts': calls}) + '\n')
        res.flush()
    return 0


if __name__ == '__main__':
    sys.exit(main())
