#!/usr/bin/env python3
"""Where the x264 program's time goes on 10-bit input, against FFmpeg's libx264 on the same frames.

    profile.py --bin DIR --files DIR --out DIR --tmp /dev/shm/prof [--reps 3]

docs/measurements/transcode-x264-10bit.md, "Method". For each case: the source's frames decoded once by the
pinned ffmpeg to a raw file on TMP; then, REPS rounds in rotated order, the x264 program with the arguments
`coreav transcode` gives it and `ffmpeg -c:v libx264` reading the same raw file as standard input (the file
itself, not a pipe), each timed from outside with every thread's CPU time sampled from /proc; then one function-level `perf record` of each. On
VARIANTS_CASE, other input forms of the x264 program, each timed and profiled once. Then `coreav transcode` on
the source itself, each arm and the FFmpeg command, REPS rounds rotated. Results in OUT/results.jsonl, perf
tables in OUT/perf-*.txt. Uses only the Python standard library; `perf` must be on PATH.
"""
import argparse, json, os, pathlib, subprocess, sys, time

CASES = [
    # name, source sha256 prefix, raw pix_fmt, x264 csp, depth
    ('prores-4k-422p10', 'e97b74358172', 'yuv422p10le', 'i422', 10),
    ('prores-4k-420p10', 'e97b74358172', 'yuv420p10le', 'i420', 10),
    ('prores-4k-420p8', 'e97b74358172', 'yuv420p', 'i420', 8),
    ('prores-1080-422p10', '199f46a5dada', 'yuv422p10le', 'i422', 10),
    ('hevc-1080-420p8', 'c39b69673584', 'yuv420p', 'i420', 8),
]
VARIANTS_CASE = 'prores-4k-420p10'
TRANSCODE = ['e97b74358172', '199f46a5dada', 'c39b69673584']
TICK = os.sysconf('SC_CLK_TCK')


def tree(pid):
    out, todo = [], [pid]
    while todo:
        p = todo.pop()
        out.append(p)
        try:
            for t in os.listdir(f'/proc/{p}/task'):
                with open(f'/proc/{p}/task/{t}/children') as f:
                    todo.extend(int(c) for c in f.read().split())
        except OSError:
            pass
    return out


def timed(cmd, stdin=None, env=None):
    """Wall time and per-thread CPU seconds (user + system) of cmd and its descendants."""
    seen = {}
    t0 = time.monotonic()
    with open(stdin, 'rb') if stdin else open(os.devnull, 'rb') as inp:
        p = subprocess.Popen(cmd, stdin=inp, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=env)
        while p.poll() is None:
            for pid in tree(p.pid):
                try:
                    tids = os.listdir(f'/proc/{pid}/task')
                except OSError:
                    continue
                for tid in tids:
                    try:
                        with open(f'/proc/{pid}/task/{tid}/stat') as f:
                            s = f.read()
                    except OSError:
                        continue
                    comm = s[s.index('(') + 1:s.rindex(')')]
                    fields = s[s.rindex(')') + 2:].split()
                    seen[(pid, int(tid))] = (comm, (int(fields[11]) + int(fields[12])) / TICK)
            time.sleep(0.05)
        err = p.stderr.read().decode(errors='replace')[-400:]
    wall = time.monotonic() - t0
    procs = {}
    for (pid, tid), (comm, cpu) in seen.items():
        d = procs.setdefault(pid, {'comm': None, 'main': 0.0, 'total': 0.0, 'threads': 0, 'max_other': 0.0})
        d['total'] += cpu
        d['threads'] += 1
        if tid == pid:
            d['comm'], d['main'] = comm, cpu
        else:
            d['max_other'] = max(d['max_other'], cpu)
    procs = [dict(v, total=round(v['total'], 2), main=round(v['main'], 2), max_other=round(v['max_other'], 2))
             for v in procs.values() if v['total'] >= 0.05]
    return {'rc': p.returncode, 'wall': round(wall, 3), 'procs': procs, 'err': err if p.returncode else ''}


def probe(ffprobe, path):
    r = subprocess.run([ffprobe, '-v', 'error', '-select_streams', 'v:0', '-show_entries',
                        'stream=width,height,r_frame_rate,pix_fmt', '-of', 'json', str(path)],
                       capture_output=True, text=True, check=True)
    return json.loads(r.stdout)['streams'][0]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--bin', required=True)
    ap.add_argument('--files', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--tmp', required=True)
    ap.add_argument('--reps', type=int, default=3)
    ap.add_argument('--cases', help='only these cases (comma-separated names)')
    ap.add_argument('--transcode', default=','.join(TRANSCODE), help='sources to transcode (sha256 prefixes)')
    ap.add_argument('--max-frames', type=int, help='decode at most this many frames a case (smoke runs)')
    a = ap.parse_args()
    b = pathlib.Path(a.bin).resolve()
    out, tmp = pathlib.Path(a.out), pathlib.Path(a.tmp)
    out.mkdir(parents=True, exist_ok=True)
    tmp.mkdir(parents=True, exist_ok=True)
    files = {p.name[:12]: p.resolve() for p in pathlib.Path(a.files).iterdir()}
    ffmpeg, ffprobe, x264, coreav = (str(b / n) for n in ('ffmpeg', 'ffprobe', 'x264', 'coreav'))
    res = open(out / 'results.jsonl', 'a')

    def emit(rec):
        res.write(json.dumps(rec) + '\n')
        res.flush()
        print(json.dumps(rec)[:300], flush=True)

    for name, src, pix_fmt, csp, depth in CASES:
        if a.cases and name not in a.cases.split(','):
            continue
        info = probe(ffprobe, files[src])
        w, h, rate = info['width'], info['height'], info['r_frame_rate']
        raw = tmp / 'raw.yuv'
        limit = ['-frames:v', str(a.max_frames)] if a.max_frames else []
        subprocess.run([ffmpeg, '-nostdin', '-v', 'error', '-i', str(files[src]), '-map', '0:v:0'] + limit +
                       ['-f', 'rawvideo', '-pix_fmt', pix_fmt, '-y', str(raw)], check=True)
        frames = raw.stat().st_size // (w * h * (1 if depth == 8 else 2) * {'i420': 3, 'i422': 4}[csp] // 2)
        arms = {
            'x264': [x264, '--demuxer', 'raw', '--input-csp', csp, '--output-csp', csp, '--input-depth', str(depth),
                     '--output-depth', str(depth), '--input-res', f'{w}x{h}', '--fps', rate, '--preset', 'medium',
                     '--crf', '23', '--log-level', 'error', '--no-progress', '--muxer', 'raw', '--output',
                     str(tmp / 'x264.264'), '-'],
            'libx264': [ffmpeg, '-nostdin', '-hide_banner', '-loglevel', 'error', '-nostats', '-f', 'rawvideo',
                        '-pix_fmt', pix_fmt, '-video_size', f'{w}x{h}', '-framerate', rate, '-i', 'pipe:0',
                        '-c:v', 'libx264', '-preset', 'medium', '-crf', '23', '-f', 'h264', '-y',
                        str(tmp / 'libx264.264')],
        }
        order = list(arms)
        for r in range(a.reps):
            for arm in order[r % 2:] + order[:r % 2]:
                emit(dict(timed(arms[arm], stdin=raw), kind='raw', case=name, arm=arm, rep=r, frames=frames,
                          size=f'{w}x{h}', pix_fmt=pix_fmt))
        for arm, cmd in arms.items():
            data = tmp / 'perf.data'
            with open(raw, 'rb') as inp:
                subprocess.run(['perf', 'record', '-q', '-e', 'cpu-clock', '-F', '999', '-o', str(data), '--'] + cmd,
                               stdin=inp, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            sort = 'pid,symbol' if arm == 'x264' else 'comm'
            rep = subprocess.run(['perf', 'report', '-i', str(data), '--no-children', '--sort', sort, '--stdio',
                                  '-g', 'none'], capture_output=True, text=True).stdout
            lines = [l for l in rep.splitlines() if l.strip() and not l.startswith('#')][:25]
            (out / f'perf-{name}-{arm}.txt').write_text('\n'.join(lines) + '\n')
            data.unlink(missing_ok=True)
        if name == VARIANTS_CASE:
            variants = {
                'y4m': (['--demuxer', 'y4m'], ['-f', 'yuv4mpegpipe', '-strict', '-1', '-pix_fmt', pix_fmt]),
                'depth16': (['--demuxer', 'raw', '--input-csp', csp, '--input-depth', '16', '--input-res', f'{w}x{h}',
                             '--fps', rate], ['-f', 'rawvideo', '-pix_fmt', 'yuv420p16le']),
                'thread-input': (['--demuxer', 'raw', '--input-csp', csp, '--input-depth', str(depth), '--input-res',
                                  f'{w}x{h}', '--fps', rate, '--thread-input'], None),
            }
            tail = ['--output-csp', csp, '--output-depth', str(depth), '--preset', 'medium', '--crf', '23',
                    '--log-level', 'error', '--no-progress', '--muxer', 'raw', '--output', str(tmp / 'v.264'), '-']
            for vname, (vargs, remake) in variants.items():
                src_raw = raw
                if remake:
                    src_raw = tmp / 'variant.in'
                    subprocess.run([ffmpeg, '-nostdin', '-v', 'error', '-i', str(files[src]), '-map', '0:v:0'] + limit
                                   + remake + ['-y', str(src_raw)], check=True)
                cmd = [x264] + vargs + tail
                emit(dict(timed(cmd, stdin=src_raw), kind='variant', case=name, arm=vname, rep=0, frames=frames,
                          size=f'{w}x{h}', pix_fmt=pix_fmt))
                data = tmp / 'perf.data'
                with open(src_raw, 'rb') as inp:
                    subprocess.run(['perf', 'record', '-q', '-e', 'cpu-clock', '-F', '999', '-o', str(data), '--']
                                   + cmd, stdin=inp, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                rep = subprocess.run(['perf', 'report', '-i', str(data), '--no-children', '--sort', 'pid,symbol',
                                      '--stdio', '-g', 'none'], capture_output=True, text=True).stdout
                lines = [l for l in rep.splitlines() if l.strip() and not l.startswith('#')][:25]
                (out / f'perf-{name}-x264-{vname}.txt').write_text('\n'.join(lines) + '\n')
                data.unlink(missing_ok=True)
                if remake:
                    src_raw.unlink()
        raw.unlink()

    env = dict(os.environ, PATH=f'{b}:{os.environ["PATH"]}')
    for src in [s for s in a.transcode.split(',') if s]:
        cmds = {
            'coreav-x264': [coreav, 'transcode', str(files[src]), str(tmp / 'a.mp4'), '--preset', 'medium',
                            '--crf', '23', '--audio', 'none', '--encoder', 'x264', '--x264', x264, '--force'],
            'coreav-ffmpeg': [coreav, 'transcode', str(files[src]), str(tmp / 'b.mp4'), '--preset', 'medium',
                              '--crf', '23', '--audio', 'none', '--encoder', 'ffmpeg', '--ffmpeg', ffmpeg, '--force'],
            'baseline': [ffmpeg, '-nostdin', '-noautorotate', '-flags', '+unaligned', '-i', str(files[src]), '-map',
                         '0:v:0', '-an', '-c:v', 'libx264', '-preset', 'medium', '-crf', '23', '-fps_mode',
                         'passthrough', '-y', str(tmp / 'c.mp4')],
        }
        order = list(cmds)
        for r in range(a.reps):
            for arm in order[r % 3:] + order[:r % 3]:
                emit(dict(timed(cmds[arm], env=env), kind='transcode', case=src, arm=arm, rep=r))
    return 0


if __name__ == '__main__':
    sys.exit(main())
