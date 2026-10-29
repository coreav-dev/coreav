#!/usr/bin/env python3
"""Instruction-level view for docs/measurements/transcode-x264-10bit.md, "An instruction-level view of get_frame":
samples of the x264 main thread per instruction offset in get_frame, for slow and fast input arms, with two builds
of the x264 program.

    why4.py --bin DIR --files DIR --out DIR --tmp DIR [--events cpu-clock] [--max-frames N]

OUT/why4.jsonl holds the timed runs, which sampled events this host allows, and per arm and event the sample counts
by offset in get_frame and, for load-latency samples, by kind of data address. OUT/perf holds each arm's top symbols
on the main thread. OUT/private holds the annotated disassembly, the thread's memory maps and the raw samples; it is
not published.
"""
import argparse, collections, json, os, pathlib, re, subprocess, sys, threading, time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from why import SOURCE, report, sha  # noqa: E402
from why3 import run  # noqa: E402

import numpy  # noqa: E402

SYM = 'get_frame'
EVENTS = {
    'cycles': [['-e', 'cycles:pp', '-c', '100003'], ['-e', 'cycles', '-c', '100003']],
    'load-bound': [['-e', 'exe_activity.bound_on_loads', '-c', '100003']],
    'slow-loads': [['-d', '-e', 'mem_trans_retired.load_latency_gt_8:P', '-c', '20011'],
                   ['-d', '-e', 'cpu/mem-loads,ldlat=8/P', '-c', '20011']],
}


def probe(args):
    """True when perf can sample with these arguments on this host."""
    p = subprocess.run(['perf', 'record', '-q', '-o', '/dev/null'] + args + ['--', 'sh', '-c', 'i=0; while [ $i -lt 200000 ]; do i=$((i+1)); done'],
                       capture_output=True, text=True)
    return p.returncode == 0 and 'not supported' not in p.stderr.lower(), p.stderr[-300:]


def region(addr, maps, stack):
    for lo, hi, name in maps:
        if lo <= addr < hi:
            if name in stack:
                return 'stack'
            if name.startswith('/memfd:') or name.endswith('.yuv'):
                return 'input'
            if name == '' or name.startswith('['):
                return 'anonymous'
            return 'file:' + os.path.basename(name).split('.')[0][:12]
    return 'unmapped'


def main():
    ap = argparse.ArgumentParser()
    for name in ('--bin', '--files', '--out', '--tmp'):
        ap.add_argument(name, required=True)
    ap.add_argument('--events', help='sample this one event for every kind (smoke runs)')
    ap.add_argument('--max-frames', type=int, help='at most this many frames (smoke runs)')
    a = ap.parse_args()
    b = pathlib.Path(a.bin).resolve()
    out, tmp = pathlib.Path(a.out), pathlib.Path(a.tmp)
    priv = out / 'private'
    for d in (out / 'perf', priv, tmp):
        d.mkdir(parents=True, exist_ok=True)
    files = {p.name[:12]: p.resolve() for p in pathlib.Path(a.files).iterdir()}
    res = open(out / 'why4.jsonl', 'a')

    def emit(rec):
        res.write(json.dumps(rec) + '\n')
        res.flush()
        print(json.dumps(rec)[:300], flush=True)

    events = {}
    for kind, choices in EVENTS.items():
        for args in ([['-e', a.events, '-c', '100003']] if a.events else choices):
            ok, err = probe(args)
            emit({'kind': 'probe', 'event': kind, 'args': args, 'ok': ok, 'err': '' if ok else err})
            if ok:
                events[kind] = args
                break

    w, h = 3840, 2160
    raw10 = tmp / '4k-10.yuv'
    cmd = [str(b / 'ffmpeg'), '-nostdin', '-v', 'error', '-i', str(files[SOURCE]), '-map', '0:v:0']
    cmd += ['-frames:v', str(a.max_frames)] if a.max_frames else []
    subprocess.run(cmd + ['-f', 'rawvideo', '-pix_fmt', 'yuv420p10le', '-y', str(raw10)], check=True)
    raw16 = tmp / '4k-16.yuv'
    src = numpy.memmap(raw10, dtype='<u2', mode='r')
    with open(raw16, 'wb') as f16:
        for i in range(0, src.size, 1 << 24):
            (src[i:i + (1 << 24)].astype('<u2') << 6).astype('<u2').tofile(f16)
    del src
    fd = os.memfd_create('4k-16')
    with open(raw16, 'rb') as f:
        while chunk := f.read(1 << 24):
            os.write(fd, chunk)
    memfd = pathlib.Path(f'/proc/{os.getpid()}/fd/{fd}')
    frames = raw10.stat().st_size // (w * h * 3)
    emit({'kind': 'input', 'frames': frames, 'bytes_10': raw10.stat().st_size, 'bytes_16': raw16.stat().st_size})

    programs = {'pinned': str(b / 'x264')}
    if os.path.exists('/usr/bin/x264'):
        programs['debian'] = '/usr/bin/x264'

    def x264(exe, depth, tag):
        return [exe, '--demuxer', 'raw', '--input-csp', 'i420', '--input-depth', str(depth), '--input-res', f'{w}x{h}',
                '--fps', '24/1', '--output-csp', 'i420', '--output-depth', '10', '--preset', 'medium', '--crf', '23',
                '--log-level', 'error', '--no-progress', '--muxer', 'raw', '--output', str(tmp / f'{tag}.264'), '-']

    for prog, exe in programs.items():
        arms = {
            'd10-pipe': (x264(exe, 10, 'a'), ('pipe', raw10, 1 << 16)),
            'd10-file': (x264(exe, 10, 'b'), ('file', raw10)),
            'd16-pipe': (x264(exe, 16, 'c'), ('pipe', raw16, 1 << 16)),
            'd16-file': (x264(exe, 16, 'd'), ('file', raw16)),
            'd16-memfd': (x264(exe, 16, 'e'), ('file', memfd)),
        }
        for arm, (cmd, stdin) in arms.items():
            emit(dict(run(cmd, stdin), kind='time', program=prog, arm=arm, sha256=sha(cmd[cmd.index('--output') + 1])))
            for kind, args in events.items():
                data = tmp / 'perf.data'
                maps = priv / f'{prog}-{arm}-{kind}-maps.txt'

                def attach(pid, args=args, maps=maps):
                    def snap():
                        try:
                            maps.write_text(pathlib.Path(f'/proc/{pid}/maps').read_text())
                        except OSError:
                            pass
                    threading.Timer(3.0 if not a.max_frames else 0.2, snap).start()
                    return subprocess.Popen(['perf', 'record', '-q', '-t', str(pid), '-o', str(data)] + args,
                                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                rec = run(cmd, stdin, attach=attach)
                (out / 'perf' / f'{prog}-{arm}-{kind}-sym.txt').write_text(report(data, 'sym', top=20))
                ann = subprocess.run(['perf', 'annotate', '-i', str(data), '--stdio', '--no-source', SYM],
                                     capture_output=True, text=True)
                (priv / f'{prog}-{arm}-{kind}-annotate.txt').write_text(ann.stdout + ann.stderr[-2000:])
                script = subprocess.run(['perf', 'script', '-i', str(data), '-F', 'ip,sym,symoff,addr'
                                         if '-d' in args else 'ip,sym,symoff'], capture_output=True, text=True).stdout
                (priv / f'{prog}-{arm}-{kind}-script.txt').write_text(script)
                mapping, stack = [], set()
                if maps.exists():
                    for line in maps.read_text().splitlines():
                        f = line.split(None, 5)
                        lo, hi = (int(x, 16) for x in f[0].split('-'))
                        mapping.append((lo, hi, f[5] if len(f) > 5 else ''))
                    stack = {n for _, _, n in mapping if n.startswith('[stack')}
                total, offs, kinds = 0, collections.Counter(), collections.Counter()
                for line in script.splitlines():
                    total += 1
                    m = re.search(r'\b' + SYM + r'\+0x([0-9a-f]+)', line)
                    if not m:
                        continue
                    offs[int(m.group(1), 16)] += 1
                    if '-d' in args:
                        kinds[region(int(line.split()[0], 16), mapping, stack)] += 1
                emit(dict(rec, kind='samples', program=prog, arm=arm, event=kind, args=args, total=total,
                          in_sym=sum(offs.values()), offsets={hex(k): v for k, v in sorted(offs.items())},
                          address_kinds=dict(kinds)))
                data.unlink(missing_ok=True)
    os.close(fd)
    for p in (raw10, raw16):
        p.unlink(missing_ok=True)
    return 0


if __name__ == '__main__':
    sys.exit(main())
