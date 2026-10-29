#!/usr/bin/env python3
"""Second run for docs/measurements/transcode-x264-10bit.md, "Why the conversion is slow": the x264 main thread's
memory-bound stalls split further, in the four arms of why.py, and the scheduler record that the first run could
not take.

    why2.py --bin DIR --files DIR --out DIR --tmp DIR

For each arm (10 or 16 bits, the regular file on standard input or a 64 KiB pipe): perf stat on the main thread
with each event group of GROUPS, then with each metric list of METRICS; one function-level perf record of the
first group, by thread; and one perf sched record, run with sudo. Results in OUT/why2.jsonl, OUT/stat, OUT/perf,
OUT/sched.
"""
import argparse, gzip, json, pathlib, re, subprocess, sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from why import SOURCE, run, sha  # noqa: E402

import numpy  # noqa: E402

GROUPS = {
    'alias': ['ld_blocks.address_alias', 'ld_blocks.no_sr', 'mem_inst_retired.all_loads',
              'mem_inst_retired.all_stores'],
    'tlb': ['dtlb_load_misses.walk_active', 'dtlb_load_misses.stlb_hit', 'dtlb_store_misses.walk_active',
            'dtlb_store_misses.stlb_hit'],
    'bound': ['exe_activity.bound_on_stores', 'exe_activity.bound_on_loads', 'resource_stalls.sb',
              'l1d_pend_miss.fb_full'],
    'other': ['machine_clears.memory_ordering', 'resource_stalls.scoreboard', 'memory_activity.stalls_l1d_miss',
              'mem_inst_retired.stlb_miss_loads'],
}
METRICS = {
    'level3': 'tma_l1_bound,tma_l2_bound,tma_l3_bound,tma_dram_bound,tma_store_bound',
    'l1': 'tma_dtlb_load,tma_store_fwd_blk,tma_split_loads,tma_lock_latency,tma_fb_full',
    'store': 'tma_store_latency,tma_dtlb_store,tma_split_stores,tma_streaming_stores',
}


def main():
    ap = argparse.ArgumentParser()
    for name in ('--bin', '--files', '--out', '--tmp'):
        ap.add_argument(name, required=True)
    ap.add_argument('--max-frames', type=int, help='at most this many frames (smoke runs)')
    a = ap.parse_args()
    b = pathlib.Path(a.bin).resolve()
    out, tmp = pathlib.Path(a.out), pathlib.Path(a.tmp)
    for d in (out, out / 'perf', out / 'sched', out / 'stat', tmp):
        d.mkdir(parents=True, exist_ok=True)
    files = {p.name[:12]: p.resolve() for p in pathlib.Path(a.files).iterdir()}
    res = open(out / 'why2.jsonl', 'a')

    def emit(rec):
        res.write(json.dumps(rec) + '\n')
        res.flush()
        print(json.dumps(rec)[:300], flush=True)

    raw10, raw16 = tmp / '4k-10.yuv', tmp / '4k-16.yuv'
    limit = ['-frames:v', str(a.max_frames)] if a.max_frames else []
    subprocess.run([str(b / 'ffmpeg'), '-nostdin', '-v', 'error', '-i', str(files[SOURCE]), '-map', '0:v:0'] + limit +
                   ['-f', 'rawvideo', '-pix_fmt', 'yuv420p10le', '-y', str(raw10)], check=True)
    src = numpy.memmap(raw10, dtype='<u2', mode='r')
    with open(raw16, 'wb') as f:
        for i in range(0, src.size, 1 << 24):
            (src[i:i + (1 << 24)] << 6).astype('<u2').tofile(f)
    w, h = 3840, 2160
    frames = raw10.stat().st_size // (w * h * 3)

    def x264(depth, tag):
        return [str(b / 'x264'), '--demuxer', 'raw', '--input-csp', 'i420', '--output-csp', 'i420', '--input-depth',
                str(depth), '--output-depth', '10', '--input-res', f'{w}x{h}', '--fps', '24/1', '--preset', 'medium',
                '--crf', '23', '--log-level', 'error', '--no-progress', '--muxer', 'raw', '--output',
                str(tmp / f'{tag}.264'), '-']

    arms = {
        'x264-d10-file': (x264(10, 'd10'), raw10, 'file'),
        'x264-d16-file': (x264(16, 'd16'), raw16, 'file'),
        'x264-d10-pipe': (x264(10, 'd10p'), raw10, 'pipe'),
        'x264-d16-pipe': (x264(16, 'd16p'), raw16, 'pipe'),
    }
    emit({'kind': 'input', 'frames': frames, 'samples': frames * w * h * 3 // 2})
    for arm, (cmd, raw, mode) in arms.items():
        stats = [(f'group-{g}', ['-e', '{' + ','.join(['cycles', 'instructions'] + evs) + '}'])
                 for g, evs in GROUPS.items()]
        stats += [(f'metric-{m}', ['-M', ms]) for m, ms in METRICS.items()]
        for tag, args in stats:
            log = out / 'stat' / f'4k-{arm}-{tag}.txt'

            def attach(pid, log=log, args=args):
                return subprocess.Popen(['perf', 'stat', '-t', str(pid), '-o', str(log)] + args,
                                        stdout=subprocess.DEVNULL, stderr=open(f'{log}.err', 'w'))
            emit(dict(run(cmd, raw, mode, attach=attach, hold=0.3), kind='stat', arm=arm, input=mode, what=tag,
                      sha256=sha(cmd[cmd.index('--output') + 1])))
        data = tmp / 'group.data'
        ev = '{' + ','.join(['cycles', 'instructions'] + GROUPS['alias']) + '}:S'
        rec = run(['perf', 'record', '-q', '-e', ev, '-c', '2000003', '-o', str(data), '--'] + cmd, raw, mode)
        rep = subprocess.run(['perf', 'report', '-i', str(data), '--no-children', '--sort', 'pid,sym', '--stdio',
                              '-g', 'none', '--group', '--show-total-period'], capture_output=True, text=True).stdout
        lines = [l for l in rep.splitlines() if l.strip() and (not l.startswith('#') or 'event' in l.lower())]
        (out / 'perf' / f'4k-{arm}-alias.txt').write_text('\n'.join(lines[:45]) + '\n')
        data.unlink(missing_ok=True)
        emit(dict(rec, kind='counters', arm=arm, input=mode, group='alias'))
        data = tmp / 'sched.data'
        rec = run(['sudo', 'perf', 'sched', 'record', '-q', '-o', str(data), '--'] + cmd, raw, mode)
        summary = subprocess.run(['sudo', 'perf', 'sched', 'timehist', '-i', str(data), '-s'], capture_output=True,
                                 text=True)
        (out / 'sched' / f'4k-{arm}-summary.txt').write_text(summary.stdout[-30000:] + summary.stderr[-2000:])
        hist = subprocess.run(['sudo', 'perf', 'sched', 'timehist', '-i', str(data), '--state'], capture_output=True,
                              text=True).stdout.splitlines()
        tasks = {}
        for line in hist:
            m = re.search(r'\s(\S+)\[(\d+)(?:/(\d+))?\]\s', line)
            if m and m.group(1).startswith('x264'):
                tasks.setdefault(int(m.group(2)), []).append(line)
        main_tid = min(tasks) if tasks else None
        with gzip.open(out / 'sched' / f'4k-{arm}-main.txt.gz', 'wt') as f:
            f.write('\n'.join(hist[:3] + tasks.get(main_tid, [])) + '\n')
        subprocess.run(['sudo', 'rm', '-f', str(data)])
        emit(dict(rec, kind='sched', arm=arm, input=mode, main_tid=main_tid,
                  main_lines=len(tasks.get(main_tid, [])), x264_tasks=len(tasks)))
    return 0


if __name__ == '__main__':
    sys.exit(main())
