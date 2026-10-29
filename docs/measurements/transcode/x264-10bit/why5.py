#!/usr/bin/env python3
"""Register sampling for docs/measurements/transcode-x264-10bit.md, "An instruction-level view of get_frame",
addendum: where the conversion loop's input, output and error rows lie, for slow and fast input arms.

    why5.py --bin DIR --files DIR --out DIR --tmp DIR [--event cpu-clock] [--max-frames N]

Each cycle sample of the x264 main thread records the registers that hold the three row addresses and the index.
OUT/why5.jsonl holds, per program and arm, the loop's sample count, the region of the input address, and how often
two of the addresses share their offset within a 4 KiB page. OUT/private holds the memory maps; it is not published.
"""
import argparse, collections, json, os, pathlib, re, subprocess, sys, threading, time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from why import SOURCE, sha  # noqa: E402
from why3 import run  # noqa: E402

import numpy  # noqa: E402

LOOPS = {'pinned': (0x1ba30 + 0x3b8, 0x1ba30 + 0x402), 'debian': (0x1a980 + 0x410, 0x1a980 + 0x466)}


def region(addr, maps):
    names = [name for lo, hi, name in maps if lo <= addr < hi]
    if any(n.startswith('/memfd:') or n.endswith('.yuv') for n in names):
        return 'input'
    if any(n.startswith('[stack') for n in names):
        return 'stack'
    if any(n in ('', '[heap]') for n in names):
        return 'anonymous'
    return 'other' if names else 'unmapped'


def main():
    ap = argparse.ArgumentParser()
    for name in ('--bin', '--files', '--out', '--tmp'):
        ap.add_argument(name, required=True)
    ap.add_argument('--event', default='cycles')
    ap.add_argument('--max-frames', type=int)
    a = ap.parse_args()
    b = pathlib.Path(a.bin).resolve()
    out, tmp = pathlib.Path(a.out), pathlib.Path(a.tmp)
    priv = out / 'private'
    for d in (priv, tmp):
        d.mkdir(parents=True, exist_ok=True)
    files = {p.name[:12]: p.resolve() for p in pathlib.Path(a.files).iterdir()}
    res = open(out / 'why5.jsonl', 'a')

    def emit(rec):
        res.write(json.dumps(rec) + '\n')
        res.flush()
        print(json.dumps(rec)[:300], flush=True)

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

    programs = {'pinned': str(b / 'x264')}
    if os.path.exists('/usr/bin/x264'):
        programs['debian'] = '/usr/bin/x264'

    def x264(exe, depth, tag):
        return [exe, '--demuxer', 'raw', '--input-csp', 'i420', '--input-depth', str(depth), '--input-res', f'{w}x{h}',
                '--fps', '24/1', '--output-csp', 'i420', '--output-depth', '10', '--preset', 'medium', '--crf', '23',
                '--log-level', 'error', '--no-progress', '--muxer', 'raw', '--output', str(tmp / f'{tag}.264'), '-']

    for prog, exe in programs.items():
        lo, hi = LOOPS[prog]
        arms = {
            'd10-pipe': (x264(exe, 10, 'a'), ('pipe', raw10, 1 << 16)),
            'd10-file': (x264(exe, 10, 'b'), ('file', raw10)),
            'd16-pipe': (x264(exe, 16, 'c'), ('pipe', raw16, 1 << 16)),
            'd16-file': (x264(exe, 16, 'd'), ('file', raw16)),
            'd16-memfd': (x264(exe, 16, 'e'), ('file', memfd)),
        }
        for arm, (cmd, stdin) in arms.items():
            data = tmp / 'perf.data'
            snaps = []

            def attach(pid, snaps=snaps):
                def snap():
                    while True:
                        try:
                            snaps.append(pathlib.Path(f'/proc/{pid}/maps').read_text())
                        except OSError:
                            return
                        time.sleep(0.05)
                threading.Thread(target=snap, daemon=True).start()
                return subprocess.Popen(['perf', 'record', '-q', '-t', str(pid), '-o', str(data), '-e', a.event,
                                         '-c', '100003', '--user-regs=bx,bp,dx,r12'],
                                        stdout=subprocess.DEVNULL, stderr=open(priv / f'{prog}-{arm}-record.err', 'w'))
            rec = run(cmd, stdin, attach=attach)
            (priv / f'{prog}-{arm}-maps.txt').write_text('\n'.join(sorted({l for t in snaps for l in t.splitlines()})))
            maps, base = set(), None
            for text in snaps:
                for line in text.splitlines():
                    f = line.split(None, 5)
                    l, r = (int(x, 16) for x in f[0].split('-'))
                    name = f[5] if len(f) > 5 else ''
                    maps.add((l, r, name))
                    if base is None and name.endswith('/x264') and int(f[2], 16) == 0:
                        base = l
            script = subprocess.run(['perf', 'script', '-i', str(data), '-F', 'ip,uregs'], capture_output=True,
                                    text=True).stdout
            data.unlink(missing_ok=True)
            n, total = 0, 0
            where = collections.Counter()
            same = collections.Counter()
            diffs = collections.Counter()
            power = collections.Counter()
            for line in script.splitlines():
                parts = line.split()
                if not parts:
                    continue
                total += 1
                try:
                    ip = int(parts[0], 16)
                except ValueError:
                    continue
                regs = dict(re.findall(r'\b(BX|BP|DX|R12):(0x[0-9a-f]+)', line))
                if base is None or not lo <= ip - base < hi or len(regs) < 4:
                    continue
                bx, bp, dx, r12 = (int(regs[k], 16) for k in ('BX', 'BP', 'DX', 'R12'))
                n += 1
                inp, outp, err = bp + dx, r12 + dx, bx + dx
                where[region(inp, maps)] += 1
                same['input-output'] += (inp - outp) % 4096 == 0
                same['input-error'] += (inp - err) % 4096 == 0
                same['output-error'] += (outp - err) % 4096 == 0
                diffs[(inp - outp) % 4096] += 1
                dist = abs(inp - outp)
                power[(dist & -dist).bit_length() - 1 if dist else 64] += 1
            emit(dict(rec, kind='regs', program=prog, arm=arm, samples=total, loop=n, input_region=dict(where),
                      same_page_offset={k: round(v / n, 4) if n else None for k, v in same.items()},
                      input_minus_output_mod_4096=dict(diffs.most_common(8)),
                      distance_power_of_two={k: power[k] for k in sorted(power)},
                      sha256=sha(cmd[cmd.index('--output') + 1])))
    os.close(fd)
    for p in (raw10, raw16):
        p.unlink(missing_ok=True)
    return 0


if __name__ == '__main__':
    sys.exit(main())
