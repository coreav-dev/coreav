#!/usr/bin/env python3
"""Transparent huge pages and the x264 program's 10-bit conversion (docs/measurements/transcode-x264-10bit.md,
"Huge pages").

    thp.py --bin DIR --files DIR --out DIR --tmp DIR [--reps 3] [--max-frames N]

The 4K ProRes frames decoded once by the pinned ffmpeg to yuv420p10le, yuv422p10le and yuv420p on TMP, and the
4:2:0 frames shifted up to 16 bits. Each arm runs the pinned x264 program with the arguments of why3.py, through a
64 KiB pipe this script fills or from the regular file on standard input, in one of three settings: the host's
default, transparent huge pages disabled for the x264 process alone (PR_SET_THP_DISABLE, set by a launcher that
then executes x264; the flag is kept across exec), or disabled for the whole host (sysfs "never", restored after
the run). Each arm: REPS timed runs in rotated order, the x264 process's largest AnonHugePages seen, and one perf
stat of its main thread. One strace of the default 10-bit pipe arm on 5 frames counts its large anonymous mappings,
how many of them start on a 2 MiB boundary, and its madvise calls. Results in OUT/thp.jsonl and OUT/stat.
"""
import argparse, json, os, pathlib, re, signal, subprocess, sys, threading, time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from why import SOURCE, sha  # noqa: E402
from why3 import run  # noqa: E402

import numpy  # noqa: E402

GROUP = ['cycles', 'instructions', 'exe_activity.bound_on_loads', 'cycle_activity.stalls_total']
SYSFS = pathlib.Path('/sys/kernel/mm/transparent_hugepage/enabled')
LAUNCH = ('import ctypes, os, sys\n'
          'libc = ctypes.CDLL(None, use_errno=True)\n'
          'if libc.prctl(41, 1, 0, 0, 0) != 0:\n'
          '    sys.exit("PR_SET_THP_DISABLE failed: %d" % ctypes.get_errno())\n'
          'os.execv(sys.argv[1], sys.argv[1:])\n')


def host_thp(mode):
    subprocess.run(['sudo', 'tee', str(SYSFS)], input=mode.encode(), stdout=subprocess.DEVNULL, check=True)


def current_thp():
    return re.search(r'\[(\w+)\]', SYSFS.read_text()).group(1)


def huge_kib(pid, seen):
    while True:
        try:
            text = pathlib.Path(f'/proc/{pid}/smaps_rollup').read_text()
        except OSError:
            return
        m = re.search(r'^AnonHugePages:\s+(\d+) kB', text, re.M)
        if m:
            seen[0] = max(seen[0], int(m.group(1)))
        time.sleep(0.5)


def main():
    ap = argparse.ArgumentParser()
    for name in ('--bin', '--files', '--out', '--tmp'):
        ap.add_argument(name, required=True)
    ap.add_argument('--reps', type=int, default=3)
    ap.add_argument('--max-frames', type=int, help='at most this many frames (smoke runs)')
    a = ap.parse_args()
    b = pathlib.Path(a.bin).resolve()
    out, tmp = pathlib.Path(a.out), pathlib.Path(a.tmp)
    for d in (out / 'stat', tmp):
        d.mkdir(parents=True, exist_ok=True)
    files = {p.name[:12]: p.resolve() for p in pathlib.Path(a.files).iterdir()}
    launcher = tmp / 'nothp.py'
    launcher.write_text(LAUNCH)
    res = open(out / 'thp.jsonl', 'a')

    def emit(rec):
        res.write(json.dumps(rec) + '\n')
        res.flush()
        print(json.dumps(rec)[:300], flush=True)

    default = current_thp()
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(128 + signal.SIGTERM))
    emit({'kind': 'host', 'thp_enabled': SYSFS.read_text().strip(),
          'thp_defrag': pathlib.Path('/sys/kernel/mm/transparent_hugepage/defrag').read_text().strip()})
    w, h = 3840, 2160
    raw = {}
    for fmt in ('yuv420p10le', 'yuv422p10le', 'yuv420p'):
        raw[fmt] = tmp / f'4k-{fmt}.yuv'
        cmd = [str(b / 'ffmpeg'), '-nostdin', '-v', 'error', '-i', str(files[SOURCE]), '-map', '0:v:0']
        cmd += ['-frames:v', str(a.max_frames)] if a.max_frames else []
        subprocess.run(cmd + ['-f', 'rawvideo', '-pix_fmt', fmt, '-y', str(raw[fmt])], check=True)
    raw['16'] = tmp / '4k-16.yuv'
    src = numpy.memmap(raw['yuv420p10le'], dtype='<u2', mode='r')
    with open(raw['16'], 'wb') as f16:
        for i in range(0, src.size, 1 << 24):
            (src[i:i + (1 << 24)].astype('<u2') << 6).astype('<u2').tofile(f16)
    del src

    def x264(csp, depth, tag):
        return [str(b / 'x264'), '--demuxer', 'raw', '--input-csp', csp, '--input-depth', str(depth), '--input-res',
                f'{w}x{h}', '--fps', '24/1', '--output-csp', csp, '--output-depth', '8' if depth == 8 else '10',
                '--preset', 'medium', '--crf', '23', '--log-level', 'error', '--no-progress', '--muxer', 'raw',
                '--output', str(tmp / f'{tag}.264'), '-']

    pipe = lambda fmt: ('pipe', raw[fmt], 1 << 16)
    arms = {
        'd10-pipe': ('default', x264('i420', 10, 'a'), pipe('yuv420p10le')),
        'd10-pipe-nothp': ('process', x264('i420', 10, 'b'), pipe('yuv420p10le')),
        'd10-pipe-never': ('host', x264('i420', 10, 'c'), pipe('yuv420p10le')),
        'd10-file-nothp': ('process', x264('i420', 10, 'd'), ('file', raw['yuv420p10le'])),
        'd16-pipe-nothp': ('process', x264('i420', 16, 'e'), pipe('16')),
        'd16-file': ('default', x264('i420', 16, 'f'), ('file', raw['16'])),
        'd10-422-pipe': ('default', x264('i422', 10, 'g'), pipe('yuv422p10le')),
        'd10-422-pipe-nothp': ('process', x264('i422', 10, 'h'), pipe('yuv422p10le')),
        'd8-pipe': ('default', x264('i420', 8, 'i'), pipe('yuv420p')),
        'd8-pipe-nothp': ('process', x264('i420', 8, 'j'), pipe('yuv420p')),
    }

    def one(arm, attach=None):
        setting, cmd, stdin = arms[arm]
        if setting == 'process':
            cmd = [sys.executable, '-I', str(launcher)] + cmd
        if setting == 'host':
            host_thp('never')
        seen = [0]

        def watch(pid, attach=attach):
            threading.Thread(target=huge_kib, args=(pid, seen), daemon=True).start()
            return attach(pid) if attach else None
        try:
            rec = run(cmd, stdin, attach=watch)
        finally:
            if setting == 'host':
                host_thp(default)
        return dict(rec, arm=arm, setting=setting, anon_huge_kib=seen[0],
                    sha256=sha(cmd[cmd.index('--output') + 1]))

    order = list(arms)
    for r in range(a.reps):
        for arm in order[r % len(order):] + order[:r % len(order)]:
            emit(dict(one(arm), kind='time', rep=r))
    for arm in order:
        log = out / 'stat' / f'{arm}.txt'

        def attach(pid, log=log):
            return subprocess.Popen(['perf', 'stat', '-t', str(pid), '-o', str(log), '-e',
                                     '{' + ','.join(GROUP) + '}'],
                                    stdout=subprocess.DEVNULL, stderr=open(f'{log}.err', 'w'))
        emit(dict(one(arm, attach=attach), kind='stat'))
    trace = tmp / 'strace.txt'
    _, cmd, stdin = arms['d10-pipe']
    rec = run(['strace', '-f', '-e', 'trace=mmap,madvise', '-o', str(trace)] + cmd[:-1] + ['--frames', '5', '-'],
              stdin)
    large = aligned = 0
    advice = {}
    for line in trace.read_text().splitlines():
        m = re.search(r'mmap\(NULL, (\d+), [^)]*MAP_ANONYMOUS[^)]*\) = (0x[0-9a-f]+)', line)
        if m and int(m.group(1)) >= 1 << 20:
            large += 1
            aligned += int(m.group(2), 16) % (1 << 21) == 0
        m = re.search(r'madvise\(0x[0-9a-f]+, \d+, (\w+)\)', line)
        if m:
            advice[m.group(1)] = advice.get(m.group(1), 0) + 1
    emit(dict(rec, kind='strace', arm='d10-pipe', frames=5, anonymous_mappings_1mib=large,
              on_2mib_boundary=aligned, madvise=advice))
    for p in list(raw.values()) + [launcher]:
        p.unlink(missing_ok=True)
    return 0


if __name__ == '__main__':
    sys.exit(main())
