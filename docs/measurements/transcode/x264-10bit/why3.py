#!/usr/bin/env python3
"""Input methods for docs/measurements/transcode-x264-10bit.md, "Input methods CoreAV controls": the same frames
handed to the pinned x264 program in ways a caller can choose, with timing, a function profile and the main
thread's stall counters for each.

    why3.py --bin DIR --files DIR --out DIR --tmp DIR --disk DIR [--reps 3] [--max-frames N]

Arms (ARMS below): 10-bit or 16-bit samples (16-bit: each 10-bit sample shifted up 6 bits; "16r": shifted up 6
with its top 6 bits copied into the low 6), handed over as a regular file on tmpfs, a regular file on the boot
disk, a memfd, a y4m file, or a pipe of 64 KiB or 1 MiB that this script fills. Each arm: REPS timed runs in
rotated order, one cpu-clock profile by thread and function, and one perf stat of the main thread with one
explicit event group. Results in OUT/why3.jsonl, OUT/perf, OUT/stat.
"""
import argparse, fcntl, json, os, pathlib, signal, subprocess, sys, threading, time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from why import SOURCE, cpu_of, feeder, report, sha  # noqa: E402

import numpy  # noqa: E402

F_SETPIPE_SZ = 1031
GROUP = ['cycles', 'instructions', 'exe_activity.bound_on_loads', 'cycle_activity.stalls_total',
         'mem_inst_retired.all_loads', 'ld_blocks.address_alias']


def run(cmd, stdin, attach=None):
    """cmd with stdin: ('file', path) opens path; ('pipe', path, size) fills a pipe of size bytes from path."""
    seen = {}
    t0 = time.monotonic()
    feed = None
    if stdin[0] == 'file':
        with open(stdin[1], 'rb') as inp:
            p = subprocess.Popen(cmd, stdin=inp, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    else:
        r, w = os.pipe()
        fcntl.fcntl(w, F_SETPIPE_SZ, stdin[2])
        p = subprocess.Popen(cmd, stdin=r, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        os.close(r)
        feed = threading.Thread(target=feeder, args=(stdin[1], w))
    helper = attach(p.pid) if attach else None
    if feed:
        time.sleep(0.3 if helper else 0)
        feed.start()
    err = []
    drain = threading.Thread(target=lambda: err.append(p.stderr.read()))
    drain.start()
    while p.poll() is None:
        seen.update(cpu_of(p.pid))
        time.sleep(0.05)
    wall = time.monotonic() - t0
    drain.join()
    if feed:
        feed.join()
    if helper:
        helper.send_signal(signal.SIGINT)
        helper.wait(timeout=60)
    return {'rc': p.returncode, 'wall': round(wall, 3), 'main_cpu': round(seen.get(p.pid, ('', 0.0))[1], 2),
            'total_cpu': round(sum(c for _, c in seen.values()), 2),
            'err': err[0].decode(errors='replace')[-400:] if p.returncode and err else ''}


def main():
    ap = argparse.ArgumentParser()
    for name in ('--bin', '--files', '--out', '--tmp', '--disk'):
        ap.add_argument(name, required=True)
    ap.add_argument('--reps', type=int, default=3)
    ap.add_argument('--max-frames', type=int, help='at most this many frames (smoke runs)')
    a = ap.parse_args()
    b = pathlib.Path(a.bin).resolve()
    out, tmp, disk = pathlib.Path(a.out), pathlib.Path(a.tmp), pathlib.Path(a.disk)
    for d in (out, out / 'perf', out / 'stat', tmp, disk):
        d.mkdir(parents=True, exist_ok=True)
    files = {p.name[:12]: p.resolve() for p in pathlib.Path(a.files).iterdir()}
    res = open(out / 'why3.jsonl', 'a')

    def emit(rec):
        res.write(json.dumps(rec) + '\n')
        res.flush()
        print(json.dumps(rec)[:300], flush=True)

    memfds = []

    def inputs(tag, w, h):
        raw10 = tmp / f'{tag}-10.yuv'
        cmd = [str(b / 'ffmpeg'), '-nostdin', '-v', 'error', '-i', str(files[SOURCE]), '-map', '0:v:0']
        cmd += ['-frames:v', str(a.max_frames)] if a.max_frames else []
        cmd += ['-vf', f'scale={w}:{h}'] if (w, h) != (3840, 2160) else []
        subprocess.run(cmd + ['-f', 'rawvideo', '-pix_fmt', 'yuv420p10le', '-y', str(raw10)], check=True)
        src = numpy.memmap(raw10, dtype='<u2', mode='r')
        made = {'10': raw10, '16': tmp / f'{tag}-16.yuv', '16r': tmp / f'{tag}-16r.yuv'}
        with open(made['16'], 'wb') as f16, open(made['16r'], 'wb') as f16r:
            for i in range(0, src.size, 1 << 24):
                v = src[i:i + (1 << 24)].astype('<u2')
                (v << 6).astype('<u2').tofile(f16)
                ((v << 6) | (v >> 4)).astype('<u2').tofile(f16r)
        frame = w * h * 3
        made['16-y4m'] = tmp / f'{tag}-16.y4m'
        with open(made['16'], 'rb') as f, open(made['16-y4m'], 'wb') as y:
            y.write(f'YUV4MPEG2 W{w} H{h} F24:1 Ip A1:1 C420p16\n'.encode())
            while True:
                chunk = f.read(frame)
                if not chunk:
                    break
                y.write(b'FRAME\n' + chunk)
        made['16-disk'] = disk / f'{tag}-16.yuv'
        subprocess.run(['cp', str(made['16']), str(made['16-disk'])], check=True)
        fd = os.memfd_create(f'{tag}-16')
        with open(made['16'], 'rb') as f:
            while True:
                chunk = f.read(1 << 24)
                if not chunk:
                    break
                os.write(fd, chunk)
        memfds.append(fd)
        made['16-memfd'] = pathlib.Path(f'/proc/{os.getpid()}/fd/{fd}')
        return made, raw10.stat().st_size // frame

    def x264(depth, w, h, tag, y4m=False):
        args = ['--demuxer', 'y4m'] if y4m else ['--demuxer', 'raw', '--input-csp', 'i420', '--input-depth',
                                                 str(depth), '--input-res', f'{w}x{h}', '--fps', '24/1']
        return [str(b / 'x264')] + args + ['--output-csp', 'i420', '--output-depth', '10', '--preset', 'medium',
                                           '--crf', '23', '--log-level', 'error', '--no-progress', '--muxer', 'raw',
                                           '--output', str(tmp / f'{tag}.264'), '-']

    for tag, w, h in (('4k', 3840, 2160), ('1080p', 1920, 1080)):
        made, frames = inputs(tag, w, h)
        arms = {
            'd10-file': (x264(10, w, h, 'a'), ('file', made['10'])),
            'd16-file': (x264(16, w, h, 'b'), ('file', made['16'])),
            'd16-pipe-64k': (x264(16, w, h, 'c'), ('pipe', made['16'], 1 << 16)),
            'd16-memfd': (x264(16, w, h, 'd'), ('file', made['16-memfd'])),
        }
        if tag == '4k':
            arms.update({
                'd16-pipe-1m': (x264(16, w, h, 'e'), ('pipe', made['16'], 1 << 20)),
                'd16-disk-file': (x264(16, w, h, 'f'), ('file', made['16-disk'])),
                'd16-y4m-file': (x264(16, w, h, 'g', y4m=True), ('file', made['16-y4m'])),
                'd16r-file': (x264(16, w, h, 'h'), ('file', made['16r'])),
            })
        samples = frames * w * h * 3 // 2
        emit({'kind': 'input', 'case': tag, 'frames': frames, 'samples': samples,
              'bytes_16bit': made['16'].stat().st_size})
        order = list(arms)
        for r in range(a.reps):
            for arm in order[r % len(order):] + order[:r % len(order)]:
                cmd, stdin = arms[arm]
                emit(dict(run(cmd, stdin), kind='time', case=tag, arm=arm, rep=r, samples=samples,
                          sha256=sha(cmd[cmd.index('--output') + 1])))
        for arm, (cmd, stdin) in arms.items():
            data = tmp / 'perf.data'
            rec = run(['perf', 'record', '-q', '-e', 'cpu-clock', '-F', '999', '-o', str(data), '--'] + cmd, stdin)
            (out / 'perf' / f'{tag}-{arm}-cpu.txt').write_text(report(data, 'pid,sym', top=40))
            data.unlink(missing_ok=True)
            emit(dict(rec, kind='profile', case=tag, arm=arm))
            log = out / 'stat' / f'{tag}-{arm}-group.txt'

            def attach(pid, log=log):
                return subprocess.Popen(['perf', 'stat', '-t', str(pid), '-o', str(log), '-e',
                                         '{' + ','.join(GROUP) + '}'],
                                        stdout=subprocess.DEVNULL, stderr=open(f'{log}.err', 'w'))
            emit(dict(run(cmd, stdin, attach=attach), kind='stat', case=tag, arm=arm))
        if tag == '4k':
            strace = tmp / 'strace.txt'
            for arm in ('d16-file', 'd16-memfd', 'd16-disk-file', 'd16-y4m-file'):
                cmd, stdin = arms[arm]
                short = cmd[:-1] + ['--frames', '10', '-']
                rec = run(['strace', '-e', 'trace=read,mmap', '-o', str(strace)] + short, stdin)
                calls = {}
                for line in strace.read_text().splitlines():
                    call = line.split('(', 1)[0].split()[-1]
                    calls[call] = calls.get(call, 0) + 1
                emit(dict(rec, kind='strace', case=tag, arm=arm, calls=calls))
        for fd in memfds:
            os.close(fd)
        memfds.clear()
        for p in made.values():
            if not str(p).startswith('/proc/'):
                p.unlink(missing_ok=True)
    return 0


if __name__ == '__main__':
    sys.exit(main())
