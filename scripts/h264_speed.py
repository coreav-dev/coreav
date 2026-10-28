#!/usr/bin/env python3
"""The decoder against FFmpeg, single-threaded (bars P1 and P2 of
docs/design/h264.md).

    python3 -I scripts/h264_speed.py --decoder target/release/examples/decode \
        --ffmpeg ffmpeg --frames 300 --rounds 3 [--report speed.json] STREAM.264 ...

Both sides decode the same Annex B elementary stream and throw the
frames away, so what is compared is decoding: this decoder is given
`--discard`, FFmpeg writes `rawvideo` to a `/dev/null` the kernel
discards without reading, with `-fps_mode passthrough` so that its frame-rate
conversion does not duplicate or drop frames of a stream with irregular
timestamps (docs/design/h264.md, "Snapshot outliers"). FFmpeg decodes with `-threads 1`, as this
decoder has one thread; the wall-clock comparison against an FFmpeg that
uses every core belongs to the threading step.

This pairing is asymmetric: `--discard` drops the frame while FFmpeg
writes it. `scripts/h264_parity.py` measures the two symmetric pairings
and this one side by side, and
`docs/measurements/h264-decoder-speed.md` records what the asymmetry is
worth. An earlier version of this docstring claimed the null muxer costs
FFmpeg more than a discarded write and that this pairing was therefore
the reading least favourable to this decoder; that is not what the
measurement shows. Bars P1 and P2 are measured with the method
`docs/design/h264.md` pre-registers; this is the instrument of the
rounds between them.

Per stream: one warm-up of each side, then `--rounds` rounds with the
two in alternating order; a side's time is the median of its rounds and
the stream's ratio is ours over theirs. The report gives the median of
the per-stream ratios with a bootstrap 95% confidence interval, overall
and per resolution class (from the file name, as the N2 spike named
them: `.sd.`, `.720p.`, `.1080p.`, `.4k.`).
"""
import argparse
import collections
import json
import pathlib
import random
import statistics
import subprocess
import sys
import time

RESAMPLES = 2_000


def timed(argv):
    start = time.perf_counter()
    with open('/dev/null', 'wb') as sink:
        done = subprocess.run(argv, stdout=sink, stderr=subprocess.PIPE)
    return time.perf_counter() - start, done


def class_of(name):
    for cls in ('4k', '1080p', '720p', 'sd'):
        if f'.{cls}.' in name:
            return cls
    return 'unclassed'


def interval(values, seed):
    rng = random.Random(seed)
    medians = sorted(statistics.median([values[rng.randrange(len(values))] for _ in values])
                     for _ in range(RESAMPLES))
    return medians[int(0.025 * RESAMPLES)], medians[int(0.975 * RESAMPLES) - 1]


def main():
    p = argparse.ArgumentParser()
    p.add_argument('streams', nargs='+')
    p.add_argument('--decoder', default='target/release/examples/decode')
    p.add_argument('--ffmpeg', default='ffmpeg')
    p.add_argument('--frames', type=int, default=300)
    p.add_argument('--rounds', type=int, default=3)
    p.add_argument('--kernels', default='auto')
    p.add_argument('--seed', type=int, default=20261021)
    p.add_argument('--report')
    args = p.parse_args()
    rows = []
    for stream in args.streams:
        name = pathlib.Path(stream).name
        ours = [args.decoder, '--frames', str(args.frames), '--kernels', args.kernels,
                '--discard', stream]
        theirs = [args.ffmpeg, '-nostdin', '-v', 'error', '-threads', '1', '-i', stream,
                  '-frames:v', str(args.frames), '-fps_mode', 'passthrough', '-f', 'rawvideo', '-']
        failed = None
        for argv in (ours, theirs):
            _, done = timed(argv)
            if done.returncode != 0:
                failed = f'{pathlib.Path(argv[0]).name}: {done.stderr.decode(errors="replace")[-200:]}'
        if failed:
            rows.append({'stream': name, 'status': 'failed', 'error': failed})
            print(f'{name}: {failed}', file=sys.stderr, flush=True)
            continue
        mine, yours = [], []
        for round_ in range(args.rounds):
            order = [(mine, ours), (yours, theirs)]
            if round_ % 2:
                order.reverse()
            for into, argv in order:
                seconds, done = timed(argv)
                if done.returncode == 0:
                    into.append(seconds)
        if len(mine) < args.rounds or len(yours) < args.rounds:
            rows.append({'stream': name, 'status': 'failed',
                         'error': f'{len(mine)} of {args.rounds} rounds here, '
                                  f'{len(yours)} of {args.rounds} for FFmpeg'})
            continue
        ours_s, theirs_s = statistics.median(mine), statistics.median(yours)
        rows.append({'stream': name, 'status': 'ok', 'class': class_of(name),
                     'ours_seconds': round(ours_s, 4), 'ffmpeg_seconds': round(theirs_s, 4),
                     'ratio': ours_s / theirs_s})
        print(f'{name:40s} ours {ours_s:7.3f}s  ffmpeg {theirs_s:7.3f}s  ratio {ours_s / theirs_s:.3f}',
              flush=True)
    ok = [r for r in rows if r['status'] == 'ok']
    summary = {'streams': len(ok), 'frames': args.frames, 'rounds': args.rounds,
               'kernels': args.kernels, 'failed': len(rows) - len(ok)}
    if ok:
        ratios = [r['ratio'] for r in ok]
        low, high = interval(ratios, args.seed)
        summary['median_ratio'] = round(statistics.median(ratios), 4)
        summary['ci95'] = [round(low, 4), round(high, 4)]
        summary['faster_than_ffmpeg'] = sum(1 for r in ratios if r < 1.0)
        by = collections.defaultdict(list)
        for r in ok:
            by[r['class']].append(r['ratio'])
        summary['by_class'] = {
            c: {'streams': len(v), 'median_ratio': round(statistics.median(v), 4),
                'ci95': [round(x, 4) for x in interval(v, args.seed)],
                'faster_than_ffmpeg': sum(1 for r in v if r < 1.0)}
            for c, v in sorted(by.items())}
    print()
    print(json.dumps(summary, indent=1))
    if args.report:
        pathlib.Path(args.report).write_text(json.dumps({'summary': summary, 'streams': rows}, indent=1) + '\n')


if __name__ == '__main__':
    main()
