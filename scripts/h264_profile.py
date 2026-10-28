#!/usr/bin/env python3
"""Where the H.264 decoder spends its time (step 6 of docs/design/h264.md).

    python3 -I scripts/h264_profile.py --decoder target/release/examples/decode \
        --frames 300 [--report profile.json] STREAM.264 ...

Records each stream with `perf record -F 999`, reads the flat symbol
shares back, and groups them the way the N2 spike's profile does, so one
round of optimisation can be compared with the last. Shares are of the
decoder process's samples; the categories below name the clause each
part of the decoder implements.

A round of step 6 is: profile, change one thing, profile again. The
numbers here are a guide, not a bar — the bar is the ratio to FFmpeg on
the held-out draw (P1 and P2), measured separately.

The training set's method (docs/measurements/h264-training/README.md):

    python3 -I scripts/h264_profile.py --decoder BIN --event cpu-clock \
        --discard --class-frames sd=1800,720p=900,1080p=300,4k=150 \
        [--pin 2 | --threads 8] --report out.json DIR/*.h264

`--class-frames` takes each stream's frame count from its class, the
second-to-last dot-separated part of the file name (`<id>.<track>.<class>.h264`).
The ranking is the mean per-stream share (every stream weighs the same);
the report adds the pooled share (every sample weighs the same) and the
mean per class. With `--threads`, samples are split by thread name: the
report gives the caller's thread's share of the samples and how busy it
is, its samples over the time from the first sample to the last at the
sampling frequency.
"""
import argparse
import collections
import json
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
import time

# `  20.44%  decode  [.] symbol`, with the sort keys before the `[.]`.
SAMPLE = re.compile(r'^\s*(\d+\.\d+)%\s+(.*?)\s+\[[.k]\]\s+(.+?)\s*$')

# A symbol belongs to the first category whose patterns it matches. The
# order matters: the kernels are named for what they compute, and the
# control code that calls them is not.
CATEGORIES = [
    ('CABAC engine and binarizations (9.3.3.2)',
     ('cabac::', 'Cabac::', 'decode_decision', 'decode_bypass', 'decode_terminate')),
    ('CAVLC (9.2)', ('cavlc::', 'Cavlc::')),
    ('Residual blocks and reconstruction (8.5)', ('residual::', 'recon::', 'idct', 'transform')),
    ('Deblocking: strengths and driver (8.7.2.1)', ('deblock::', 'boundary_strength', 'filter_rows')),
    ('Deblocking: the edge filter (8.7.2.3)', ('filter_edge', 'filter_luma', 'filter_chroma')),
    ('Inter prediction control (8.4.1)', ('inter::', 'motion', 'ref_list', 'direct')),
    ('Interpolation kernels, vector (8.4.2.2)',
     ('simd::x86::luma_mc', 'simd::x86::chroma_mc', 'simd::x86::combine',
      'simd::neon::luma_mc', 'simd::neon::chroma_mc', 'simd::neon::combine')),
    ('Interpolation kernels, reference (8.4.2.2)', ('luma_mc', 'chroma_mc', 'combine')),
    ('Intra prediction (8.3)', ('intra::',)),
    ('Macroblock layer (7.3.5)', ('mb::', 'syntax::', 'slice::')),
    ('Parameter sets, NAL framing, bit reading (7.3)',
     ('params::', 'nal::', 'bits::', 'rbsp', 'parse_sps', 'parse_pps')),
    ('Decoded picture buffer and output (8.2.5, C.4.5)', ('dpb::', 'picture::', 'Picture::')),
    ('Memory fill and copy', ('memcpy', 'memset', 'memmove', '__rust_alloc', 'malloc', 'free')),
    ('Writing the frames out', ('write', 'Write', 'flush')),
]


def category(symbol):
    for name, patterns in CATEGORIES:
        if any(p in symbol for p in patterns):
            return name
    return 'Other'


def parse(report):
    """`perf report --stdio` lines to {symbol: share in percent}."""
    shares = collections.Counter()
    for line in report.splitlines():
        m = SAMPLE.match(line)
        if m:
            # perf writes the IPC columns after the symbol.
            symbol = re.split(r'\s{2,}', m.group(3))[0].strip()
            shares[symbol] += float(m.group(1))
    return shares


def profile(args, decoder, stream, frames, work):
    """The symbol shares of one stream, as `perf report` prints them, and,
    with threads, the caller's thread's share and busy fraction."""
    data = work / 'perf.data'
    record = ['perf', 'record', '-F', str(args.freq), '-o', str(data)]
    if args.event:
        record += ['-e', args.event]
    command = [decoder, '--frames', str(frames)]
    if args.threads > 1:
        command += ['--threads', str(args.threads)]
    if args.discard:
        command.append('--discard')
    command.append(str(stream))
    if args.pin is not None:
        command = ['taskset', '-c', str(args.pin)] + command
    start = time.perf_counter()
    run = subprocess.run(record + ['--'] + command,
                         stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    wall = time.perf_counter() - start
    if run.returncode != 0:
        return None, None, run.stderr.strip()[-200:]
    base = ['perf', 'report', '-i', str(data), '--stdio', '--no-children', '--percent-limit', '0.01']
    shares = parse(subprocess.run(base + ['--sort', 'dso,symbol'],
                                  capture_output=True, text=True).stdout)
    # perf record notices the child's exit up to a second late, so the
    # run's time is taken from its first and last samples, and a stream's
    # weight in the pooled share from its sample count.
    stamps = subprocess.run(['perf', 'script', '-i', str(data), '-F', 'time'],
                            capture_output=True, text=True).stdout.split()
    times = [float(t.rstrip(':')) for t in stamps if t.rstrip(':').replace('.', '', 1).isdigit()]
    extra = {'record_seconds': round(wall, 3), 'samples': len(times),
             'sampled_seconds': round(max(times) - min(times), 3) if times else 0.0}
    if args.threads > 1:
        caller = pathlib.Path(decoder).name[:15]
        report = subprocess.run(base + ['-n', '--sort', 'comm'], capture_output=True,
                                text=True).stdout
        share, count = 0.0, 0
        for line in report.splitlines():
            c = re.match(r'^\s*(\d+\.\d+)%\s+(\d+)\s+(\S+)\s*$', line)
            if c and c.group(3) == caller:
                share, count = float(c.group(1)) / 100, int(c.group(2))
        extra['caller_share'] = round(share, 4)
        span = extra['sampled_seconds'] or wall
        extra['caller_busy'] = round(count / (span * args.freq), 4)
        extra['caller_symbols'] = {k: round(v, 3) for k, v in parse(subprocess.run(
            base + ['--comm', caller, '--sort', 'dso,symbol'], capture_output=True,
            text=True).stdout).most_common(12)}
    data.unlink(missing_ok=True)
    return shares, extra, None


def stream_class(name):
    parts = name.split('.')
    return parts[-2] if len(parts) >= 3 else 'unknown'


def main():
    p = argparse.ArgumentParser()
    p.add_argument('streams', nargs='+')
    p.add_argument('--decoder', default='target/release/examples/decode')
    p.add_argument('--frames', type=int, default=300)
    p.add_argument('--class-frames', help='sd=1800,720p=900,...: frames by class')
    p.add_argument('--freq', type=int, default=999)
    p.add_argument('--event', help='perf event, for example cpu-clock')
    p.add_argument('--pin', type=int, help='run the decoder under taskset -c PIN')
    p.add_argument('--threads', type=int, default=1)
    p.add_argument('--discard', action='store_true', help='decode with --discard')
    p.add_argument('--report')
    args = p.parse_args()
    if not shutil.which('perf'):
        raise SystemExit('perf is not on PATH')
    if args.pin is not None and args.threads > 1:
        raise SystemExit('--pin puts every thread on one core; use one or the other')
    class_frames = {}
    if args.class_frames:
        for item in args.class_frames.split(','):
            k, v = item.split('=')
            class_frames[k] = int(v)
    total, pooled, per_stream, extras, failed = (collections.Counter(), collections.Counter(),
                                                 {}, {}, {})
    by_class = collections.defaultdict(collections.Counter)
    class_count = collections.Counter()
    with tempfile.TemporaryDirectory() as tmp:
        work = pathlib.Path(tmp)
        for stream in args.streams:
            name = pathlib.Path(stream).name
            klass = stream_class(name)
            if class_frames and klass not in class_frames:
                raise SystemExit(f'{name}: class {klass!r} is not in --class-frames')
            frames = class_frames[klass] if class_frames else args.frames
            shares, extra, error = profile(args, args.decoder, stream, frames, work)
            if error:
                failed[name] = error
                print(f'{name}: {error}', file=sys.stderr)
                continue
            # Every stream counts the same, whatever its length.
            scale = sum(shares.values()) or 1
            for symbol, share in shares.items():
                total[symbol] += share / scale
                by_class[klass][symbol] += share / scale
                # Pooled: weighted by the stream's samples.
                pooled[symbol] += share / scale * extra['samples']
            class_count[klass] += 1
            per_stream[name] = {s: round(v, 3) for s, v in shares.most_common(12)}
            extras[name] = dict(extra, frames=frames)
            print(f'{name}: {len(shares)} symbols', flush=True)
    streams = len(per_stream) or 1
    grouped = collections.Counter()
    for symbol, share in total.items():
        grouped[category(symbol)] += 100 * share / streams
    print()
    print('| Part | Share |')
    print('|---|---|')
    for name, share in grouped.most_common():
        print(f'| {name} | {share:.1f}% |')
    symbols = [(s, round(100 * v / streams, 2)) for s, v in total.most_common(40)]
    pooled_total = sum(pooled.values()) or 1
    print()
    print('The symbols that hold the most (mean per-stream share, pooled):')
    for symbol, share in symbols:
        print(f'  {share:5.2f}%  {100 * pooled[symbol] / pooled_total:5.2f}%  {symbol}')
    if args.threads > 1:
        busy = sorted(e['caller_busy'] for e in extras.values())
        share = sorted(e['caller_share'] for e in extras.values())
        if busy:
            print(f'\ncaller thread: busy median {busy[len(busy) // 2]:.3f}, '
                  f'share of samples median {share[len(share) // 2]:.3f}')
    if args.report:
        top = [s for s, _ in symbols]
        models = [line.split(':', 1)[1].strip() for line in
                  open('/proc/cpuinfo').read().splitlines() if line.startswith('model name')]
        pathlib.Path(args.report).write_text(json.dumps({
            'machine': {'cpu_model': models[0] if models else '', 'logical_cpus': len(models)},
            'streams': sorted(per_stream), 'frames': None if class_frames else args.frames, 'class_frames': class_frames,
            'freq': args.freq, 'event': args.event, 'pin': args.pin, 'threads': args.threads,
            'discard': args.discard, 'decoder': args.decoder, 'failed': failed,
            'by_part': {k: round(v, 3) for k, v in grouped.most_common()},
            'by_symbol': dict(symbols),
            'by_symbol_pooled': {s: round(100 * pooled[s] / pooled_total, 2) for s in top},
            'by_class': {c: {s: round(100 * by_class[c][s] / class_count[c], 2) for s in top}
                         for c in sorted(by_class)},
            'class_count': dict(class_count),
            'per_stream': per_stream, 'per_stream_run': extras}, indent=1) + '\n')


if __name__ == '__main__':
    main()
