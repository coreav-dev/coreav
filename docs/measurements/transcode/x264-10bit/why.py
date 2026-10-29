#!/usr/bin/env python3
"""Why the x264 program's conversion of 10-bit input costs what it does (docs/measurements/transcode-x264-10bit.md,
"Why the conversion is slow").

    why.py --part intel|amd --bin DIR --files DIR --out DIR --tmp DIR [--reps 3] [--max-frames N]

The 4K ProRes frames decoded once by the pinned ffmpeg to yuv420p10le on TMP, and a copy with every sample
shifted up 6 bits. Arms: the pinned x264 program with --input-depth 10 or 16, from the regular file on standard
input ("file") or through a 64 KiB pipe this script fills ("pipe"); libx264 through the pinned ffmpeg from the
10-bit file. Part intel answers questions 1 to 6: timing, function profiles, scheduler and syscall waits of the
x264 main thread, top-down levels, sampled counter groups by thread and function, --fps, Debian's packaged x264
and ffmpeg, and other frame sizes. Part amd times the four x264 arms and libx264 and profiles them by function.
Results in OUT/why.jsonl; perf tables in OUT/perf, OUT/sched, OUT/stat. perf and strace must be on PATH.
"""
import argparse, fcntl, gzip, json, os, pathlib, re, signal, subprocess, sys, threading, time, traceback

import numpy

SOURCE = 'e97b74358172'
F_SETPIPE_SZ = 1031
TICK = os.sysconf('SC_CLK_TCK')
PERIOD = '2000003'
GROUPS = {
    'mem': ['mem_load_retired.l1_miss', 'mem_load_retired.l2_miss', 'mem_load_retired.l3_miss',
            'mem_inst_retired.all_loads'],
    'stall': ['cycle_activity.stalls_l1d_miss', 'cycle_activity.stalls_l2_miss', 'cycle_activity.stalls_l3_miss',
              'cycle_activity.stalls_total'],
    'assist': ['assists.any', 'assists.fp', 'machine_clears.count', 'ld_blocks.store_forward'],
    'align': ['ld_blocks_partial.address_alias', 'mem_inst_retired.split_loads', 'mem_inst_retired.split_stores',
              'mem_inst_retired.all_stores'],
    'fp1': ['fp_arith_inst_retired.scalar_single', 'fp_arith_inst_retired.scalar_double',
            'fp_arith_inst_retired.128b_packed_single', 'fp_arith_inst_retired.256b_packed_single'],
    'fp2': ['fp_arith_inst_retired.512b_packed_single', 'fp_arith_inst_retired.128b_packed_double',
            'fp_arith_inst_retired.256b_packed_double', 'fp_arith_inst_retired.512b_packed_double'],
    'int': ['int_vec_retired.add_128', 'int_vec_retired.add_256', 'int_vec_retired.shuffles',
            'int_vec_retired.mul_256'],
    'branch': ['br_inst_retired.all_branches', 'br_misp_retired.all_branches', 'uops_retired.slots',
               'mem_load_retired.fb_hit'],
}
ALL_ARMS_GROUPS = ('mem', 'stall', 'assist', 'align', 'fp1')
SIZES = [('720p', 1280, 720, None), ('1080p', 1920, 1080, None), ('8k', 7680, 4320, 48)]


def cpu_of(pid):
    """{tid: (comm, cpu seconds)} of pid's threads, or {} once it is gone."""
    out = {}
    try:
        tids = os.listdir(f'/proc/{pid}/task')
    except OSError:
        return out
    for tid in tids:
        try:
            with open(f'/proc/{pid}/task/{tid}/stat') as f:
                s = f.read()
        except OSError:
            continue
        fields = s[s.rindex(')') + 2:].split()
        out[int(tid)] = (s[s.index('(') + 1:s.rindex(')')], (int(fields[11]) + int(fields[12])) / TICK)
    return out


def feeder(raw, fd):
    try:
        with open(raw, 'rb') as f:
            while True:
                b = f.read(1 << 20)
                if not b:
                    break
                view = memoryview(b)
                while view:
                    view = view[os.write(fd, view):]
    except BrokenPipeError:
        pass
    finally:
        os.close(fd)


def run(cmd, raw, mode, attach=None, hold=0.0):
    """Run cmd with raw on standard input, as the regular file itself (mode 'file') or through a 64 KiB pipe
    ('pipe'). attach(pid) may start a helper on the new process, which gets hold seconds before a pipe is filled
    and SIGINT when the process ends. Returns wall time and the started process's main-thread and total CPU."""
    seen = {}
    t0 = time.monotonic()
    feed = None
    if mode == 'file':
        with open(raw, 'rb') as inp:
            p = subprocess.Popen(cmd, stdin=inp, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    else:
        r, w = os.pipe()
        fcntl.fcntl(w, F_SETPIPE_SZ, 1 << 16)
        p = subprocess.Popen(cmd, stdin=r, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        os.close(r)
        feed = threading.Thread(target=feeder, args=(raw, w))
    helper = attach(p.pid) if attach else None
    if feed:
        time.sleep(hold)
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
            'total_cpu': round(sum(c for _, c in seen.values()), 2), 'threads': len(seen),
            'err': err[0].decode(errors='replace')[-400:] if p.returncode and err else ''}


def report(data, sort, extra=(), top=60):
    rep = subprocess.run(['perf', 'report', '-i', str(data), '--no-children', '--sort', sort, '--stdio', '-g',
                          'none'] + list(extra), capture_output=True, text=True).stdout
    return '\n'.join([l for l in rep.splitlines() if l.strip() and not l.startswith('#')][:top]) + '\n'


def sha(path):
    return subprocess.run(['sha256sum', str(path)], capture_output=True, text=True).stdout[:16]


def supported(events):
    """The events that perf stat counts here beside cycles and instructions, in one group."""
    ok = []
    for e in events:
        r = subprocess.run(['perf', 'stat', '-x,', '-e', f'{{cycles,instructions,{e}}}', '--', 'true'],
                           capture_output=True, text=True)
        if r.returncode == 0 and '<not supported>' not in r.stderr and '<not counted>' not in r.stderr:
            ok.append(e)
    return ok


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--part', required=True, choices=['intel', 'amd'])
    for name in ('--bin', '--files', '--out', '--tmp'):
        ap.add_argument(name, required=True)
    ap.add_argument('--reps', type=int, default=3)
    ap.add_argument('--max-frames', type=int, help='at most this many frames an input (smoke runs)')
    a = ap.parse_args()
    b = pathlib.Path(a.bin).resolve()
    out, tmp = pathlib.Path(a.out), pathlib.Path(a.tmp)
    for d in (out, out / 'perf', out / 'sched', out / 'stat', tmp):
        d.mkdir(parents=True, exist_ok=True)
    files = {p.name[:12]: p.resolve() for p in pathlib.Path(a.files).iterdir()}
    ffmpeg = str(b / 'ffmpeg')
    res = open(out / 'why.jsonl', 'a')

    def emit(rec):
        res.write(json.dumps(rec) + '\n')
        res.flush()
        print(json.dumps(rec)[:300], flush=True)

    def step(name, fn):
        t = time.monotonic()
        try:
            fn()
            emit({'step': name, 'ok': True, 'seconds': round(time.monotonic() - t, 1)})
        except Exception:
            emit({'step': name, 'ok': False, 'error': traceback.format_exc()[-1500:]})

    def make_raw(tag, w=None, h=None, frames=None):
        raw10 = tmp / f'{tag}-10.yuv'
        limit = min([n for n in (frames, a.max_frames) if n] or [0])
        cmd = [ffmpeg, '-nostdin', '-v', 'error', '-i', str(files[SOURCE]), '-map', '0:v:0']
        cmd += ['-frames:v', str(limit)] if limit else []
        cmd += ['-vf', f'scale={w}:{h}'] if w else []
        subprocess.run(cmd + ['-f', 'rawvideo', '-pix_fmt', 'yuv420p10le', '-y', str(raw10)], check=True)
        return raw10

    def shift16(raw10):
        raw16 = raw10.with_name(raw10.name.replace('-10.', '-16.'))
        src = numpy.memmap(raw10, dtype='<u2', mode='r')
        with open(raw16, 'wb') as f:
            for i in range(0, src.size, 1 << 24):
                (src[i:i + (1 << 24)] << 6).astype('<u2').tofile(f)
        return raw16

    def x264(depth, w, h, fps='24/1', exe=None, tag='x'):
        return [exe or str(b / 'x264'), '--demuxer', 'raw', '--input-csp', 'i420', '--output-csp', 'i420',
                '--input-depth', str(depth), '--output-depth', '10', '--input-res', f'{w}x{h}', '--fps', fps,
                '--preset', 'medium', '--crf', '23', '--log-level', 'error', '--no-progress', '--muxer', 'raw',
                '--output', str(tmp / f'{tag}.264'), '-']

    def libx264(w, h, exe=None, tag='l'):
        return [exe or ffmpeg, '-nostdin', '-hide_banner', '-loglevel', 'error', '-nostats', '-f', 'rawvideo',
                '-pix_fmt', 'yuv420p10le', '-video_size', f'{w}x{h}', '-framerate', '24', '-i', 'pipe:0', '-c:v',
                'libx264', '-preset', 'medium', '-crf', '23', '-f', 'h264', '-y', str(tmp / f'{tag}.264')]

    def output(cmd):
        return cmd[cmd.index('--output') + 1] if '--output' in cmd else cmd[-1]

    def timing(case, arms, samples, frames):
        order = list(arms)
        for r in range(a.reps):
            for arm in order[r % len(order):] + order[:r % len(order)]:
                cmd, raw, mode = arms[arm]
                emit(dict(run(cmd, raw, mode), kind='time', case=case, arm=arm, input=mode, rep=r, samples=samples,
                          frames=frames, sha256=sha(output(cmd))))

    def profile(case, arm, cmd, raw, mode, sort='pid,sym'):
        data = tmp / 'perf.data'
        rec = run(['perf', 'record', '-q', '-e', 'cpu-clock', '-F', '999', '-o', str(data), '--'] + cmd, raw, mode)
        (out / 'perf' / f'{case}-{arm}-cpu.txt').write_text(report(data, sort))
        data.unlink(missing_ok=True)
        emit(dict(rec, kind='profile', case=case, arm=arm, input=mode))

    w, h = 3840, 2160
    raw10 = make_raw('4k')
    raw16 = shift16(raw10)
    frames = raw10.stat().st_size // (w * h * 3)
    samples = frames * w * h * 3 // 2
    four = {
        'x264-d10-file': (x264(10, w, h, tag='d10'), raw10, 'file'),
        'x264-d16-file': (x264(16, w, h, tag='d16'), raw16, 'file'),
        'x264-d10-pipe': (x264(10, w, h, tag='d10p'), raw10, 'pipe'),
        'x264-d16-pipe': (x264(16, w, h, tag='d16p'), raw16, 'pipe'),
    }
    main_arms = dict(four, **{'libx264-file': (libx264(w, h), raw10, 'file')})

    step('time-4k', lambda: timing('4k', main_arms, samples, frames))

    def profiles():
        for arm, (cmd, raw, mode) in main_arms.items():
            profile('4k', arm, cmd, raw, mode, 'pid,sym' if arm.startswith('x264') else 'pid,dso,sym')
    step('profile-4k', profiles)
    if a.part == 'amd':
        return 0

    def sched():
        for arm, (cmd, raw, mode) in four.items():
            data = tmp / 'sched.data'
            rec = run(['perf', 'sched', 'record', '-q', '-g', '-o', str(data), '--'] + cmd, raw, mode)
            summary = subprocess.run(['perf', 'sched', 'timehist', '-i', str(data), '-s'], capture_output=True,
                                     text=True).stdout
            (out / 'sched' / f'4k-{arm}-summary.txt').write_text(summary[-20000:])
            hist = []
            for extra in (['--state', '-g', '--max-stack', '12'], ['-g'], []):
                hist = subprocess.run(['perf', 'sched', 'timehist', '-i', str(data)] + extra, capture_output=True,
                                      text=True).stdout.splitlines()
                if len(hist) > 3:
                    break
            tasks = {}
            for line in hist:
                m = re.search(r'\s(\S+)\[(\d+)(?:/(\d+))?\]\s', line)
                if m and m.group(1).startswith('x264'):
                    tasks.setdefault(int(m.group(2)), []).append(line)
            main_tid = min(tasks) if tasks else None
            with gzip.open(out / 'sched' / f'4k-{arm}-main.txt.gz', 'wt') as f:
                f.write('\n'.join(hist[:3] + tasks.get(main_tid, [])) + '\n')
            data.unlink(missing_ok=True)
            emit(dict(rec, kind='sched', case='4k', arm=arm, input=mode, main_tid=main_tid,
                      main_lines=len(tasks.get(main_tid, [])), x264_tasks=len(tasks)))
            log = tmp / 'strace.txt'
            rec = run(['strace', '-c', '-w', '-o', str(log)] + cmd, raw, mode)
            (out / 'sched' / f'4k-{arm}-strace.txt').write_text(log.read_text())
            emit(dict(rec, kind='strace', case='4k', arm=arm, input=mode))
    step('sched-4k', sched)

    def topdown():
        for arm, (cmd, raw, mode) in four.items():
            for tag, args in (('topdown', ['-M', 'TopdownL1,TopdownL2']), ('topdown-l1', ['--topdown']),
                              ('basic', ['-e', 'task-clock,cycles,instructions'])):
                log = out / 'stat' / f'4k-{arm}-{tag}.txt'

                def attach(pid, log=log, args=args):
                    return subprocess.Popen(['perf', 'stat', '-t', str(pid), '-o', str(log)] + args,
                                            stdout=subprocess.DEVNULL, stderr=open(f'{log}.err', 'w'))
                emit(dict(run(cmd, raw, mode, attach=attach, hold=0.3), kind='stat', case='4k', arm=arm,
                          input=mode, what=tag))
    step('topdown-4k', topdown)

    groups = {g: supported(evs) for g, evs in GROUPS.items()}
    emit({'kind': 'events', 'groups': groups,
          'missing': {g: [e for e in GROUPS[g] if e not in v] for g, v in groups.items()}})

    def counters():
        for g, evs in groups.items():
            if not evs:
                continue
            arms = four if g in ALL_ARMS_GROUPS else {k: v for k, v in four.items() if k.endswith('file')}
            for arm, (cmd, raw, mode) in arms.items():
                data = tmp / 'group.data'
                ev = '{' + ','.join(['cycles', 'instructions'] + evs) + '}:S'
                rec = run(['perf', 'record', '-q', '-e', ev, '-c', PERIOD, '-o', str(data), '--'] + cmd, raw, mode)
                (out / 'perf' / f'4k-{arm}-{g}.txt').write_text(
                    report(data, 'pid,sym', ['--group', '--show-total-period'], top=40))
                data.unlink(missing_ok=True)
                emit(dict(rec, kind='counters', case='4k', arm=arm, input=mode, group=g,
                          events=['cycles', 'instructions'] + evs))
    step('counters-4k', counters)

    step('fps-4k', lambda: timing('4k-fps', {
        'x264-d10-file-fps24': (x264(10, w, h, '24/1', tag='f24'), raw10, 'file'),
        'x264-d10-file-fps60': (x264(10, w, h, '60/1', tag='f60'), raw10, 'file')}, samples, frames))
    raw16.unlink()

    def debian():
        x, f = '/usr/bin/x264', '/usr/bin/ffmpeg'
        if not (os.path.exists(x) and os.path.exists(f)):
            raise RuntimeError('Debian x264 or ffmpeg missing')
        arms = {'debian-x264-d10-file': (x264(10, w, h, exe=x, tag='dx'), raw10, 'file'),
                'debian-libx264-file': (libx264(w, h, exe=f, tag='dl'), raw10, 'file')}
        for arm, (cmd, raw, mode) in arms.items():
            emit(dict(run(cmd, raw, mode), kind='time', case='4k-debian', arm=arm, input=mode, rep=0,
                      samples=samples, frames=frames, sha256=sha(output(cmd))))
            profile('4k-debian', arm, cmd, raw, mode, 'pid,dso,sym')
    step('debian-4k', debian)
    raw10.unlink()

    for tag, sw, sh, limit in SIZES:
        def size(tag=tag, sw=sw, sh=sh, limit=limit):
            r10 = make_raw(tag, sw, sh, limit)
            n = r10.stat().st_size // (sw * sh * 3)
            arms = {'x264-d10-file': (x264(10, sw, sh, tag=f'{tag}x'), r10, 'file'),
                    'libx264-file': (libx264(sw, sh, tag=f'{tag}l'), r10, 'file')}
            try:
                timing(tag, arms, n * sw * sh * 3 // 2, n)
                profile(tag, 'x264-d10-file', *arms['x264-d10-file'])
            finally:
                r10.unlink()
        step(f'size-{tag}', size)
    return 0


if __name__ == '__main__':
    sys.exit(main())
