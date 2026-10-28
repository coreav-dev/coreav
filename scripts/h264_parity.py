#!/usr/bin/env python3
"""Decode speed against FFmpeg under a symmetric harness.

The harness-parity rule says the two binaries of a comparison must differ
only in what is being measured. Pairing our `--discard` against FFmpeg's
`-f rawvideo -` breaks it: we drop the frame, FFmpeg writes it. This
script measures both symmetric pairings instead, and the mixed one for
comparison, so the cost of the asymmetry is visible rather than assumed:

    discard : ours --discard   against  ffmpeg -f null -
    write   : ours (frames to stdout, which this script discards)
              against  ffmpeg -f rawvideo -

and the mixed pairing, ours --discard against ffmpeg -f rawvideo -,
which is the one earlier rounds used.

FFmpeg runs with `-fps_mode passthrough`: its default frame-rate conversion
duplicates or drops the frames of a stream whose timestamps are not a steady
rate, so that `-frames:v N` does not decode N pictures of it (four training
streams; docs/design/h264.md, "Snapshot outliers").

A ratio is trustworthy only where the two symmetric pairings agree; where
they disagree the output path, not the decoder, is being measured.

`--pin CORE` runs every command under `taskset -c CORE`. Worth using:
unpinned, the same command's rounds spread 13.7% between fastest and
slowest on the measurement machine, and pinned 4.9%, so pinning nearly
triples the resolution of a comparison.

`--threads N` decodes with N threads on both sides: `ffmpeg -threads N`
and `--threads N` for ours (bar G3 of docs/design/h264.md, step 7). The
default, 1, gives the commands of the single-threaded rounds unchanged:
`-threads 1` for FFmpeg and no option for ours. It cannot be combined with
`--pin`, which would put every thread on one core. A list (`--threads
1,8`) times every count in the same interleaved rounds, names each
command and pairing with `@N`, and adds the scaling of each pairing at
each count against 1 thread: per stream, `(ours_N / ffmpeg_N) /
(ours_1 / ffmpeg_1)`, the quantity bar G3 is stated in.

Every report records the machine (`machine`: the CPU model as
/proc/cpuinfo names it, the clock it reports and the logical CPU count),
because a ratio to FFmpeg moves with the machine
(docs/measurements/h264-training/README.md). `--require-cpu TEXT` makes a
run gating: it stops before timing anything unless the CPU model contains
TEXT, and stops again, without a report, if the model changed during the
run.

Host faults inside a call (docs/design/h264.md, "Measurement method: host
faults inside a call"): `--shuffle-seed N` draws the order of the commands
anew in each round; every stream's row stores its per-round times and
orders. `--null-arm NAME` times a second copy of decoder NAME's binary, and
a stream whose copy deviates by more than 4% (8% with several threads), or
one of whose commands has a slowest round more than 50% over its fastest, is timed again (at most 2
retries, every attempt stored, a stream still faulty is flagged
`unresolved`, never dropped). `--gate-stream PATH --gate-every N` times
FFmpeg on a reference stream before the first stream, after every N streams
and after the last; a sample more than 5% from the baseline (the median of three samples that agree), still beyond it at an immediate second sample, marks the streams
since the last good sample as faulty.

`--min-seconds S` makes every timed call last at least S seconds: each
command of a stream is run K times back to back in one call, K the smallest
count for which the fastest command's warm-up, repeated K times, lasts S (the
same K for every command, kept for the stream's retries, written to the row as
`repeat`). Times stay per run, so ratios and `seconds` keep their meaning.
Eight-thread calls of the training streams last 0.17 to 0.7 s, where
process start-up jitter is a visible share of the call; 3 s is the value the
rounds use.

Per stream: one warm-up per command, then `--rounds` interleaved rounds.
A command's time is the median of its rounds, a stream's ratio is ours
over FFmpeg's, and the report gives the median per-stream ratio with a
bootstrap 95% confidence interval, overall and per resolution class.
"""
import argparse
import json
import math
import os
import random
import statistics
import subprocess
import time

RESAMPLES = 2_000
# Fixed, because Python randomizes str hashing per process and an
# interval that changes between runs cannot be checked.
SEED = 20_261_021
# Thresholds of docs/design/h264.md, "Measurement method".
# From the idle-host calibrations of docs/design/h264.md, "Measurement method".
NULL_DEVIATION = 0.04
ROUND_SPREAD = 1.50
NULL_DEVIATION_MT = 0.08       # several threads, unpinned
ROUND_SPREAD_MT = 1.50
GATE_DRIFT = 0.05
MAX_RETRIES = 2
GATE_RUNS = 3
GATE_BASELINE = 3


def timed(argv, repeat=1):
    """The time of one run of argv: `repeat` runs back to back, each a new
    process, and their total over `repeat`."""
    start = time.perf_counter()
    with open(os.devnull, 'wb') as sink:
        for _ in range(repeat):
            proc = subprocess.run(argv, stdout=sink, stderr=subprocess.DEVNULL)
            if proc.returncode != 0:
                return None
    return (time.perf_counter() - start) / repeat


def class_of(name):
    for label in ('sd', '720p', '1080p', '4k'):
        if f'.{label}.' in name:
            return label
    return 'other'


def interval(values, seed):
    if len(values) < 2:
        return (None, None)
    rng = random.Random(seed)
    medians = sorted(
        statistics.median(rng.choices(values, k=len(values)))
        for _ in range(RESAMPLES))
    return medians[int(0.025 * RESAMPLES)], medians[int(0.975 * RESAMPLES) - 1]


def tag(args, name, n):
    return name if len(args.threads) == 1 else f'{name}@{n}'


def commands(args, stream):
    """Every command of one stream, so that all of them are timed in the
    same interleaved rounds. Timing two decoders in separate passes would
    compare them across sessions, which is what the drift in
    docs/measurements/ was traced to."""
    out = {}
    for n in args.threads:
        # passthrough: FFmpeg's default frame-rate conversion duplicates or
        # drops frames of a stream whose timestamps are not a steady rate, so
        # that `-frames:v N` decodes fewer or more than N pictures of it and
        # `-f rawvideo` times another amount of work than ours does
        # (docs/design/h264.md, "Snapshot outliers").
        ff = [args.ffmpeg, '-nostdin', '-v', 'error', '-threads', str(n),
              '-i', stream, '-frames:v', str(args.frames), '-fps_mode', 'passthrough']
        out[tag(args, 'ff_null', n)] = ff + ['-f', 'null', '-']
        out[tag(args, 'ff_raw', n)] = ff + ['-f', 'rawvideo', '-']
        for name, path in args.decoders:
            argv = [path, '--frames', str(args.frames)]
            if n != 1:
                argv += ['--threads', str(n)]
            out[tag(args, f'{name}_discard', n)] = argv + ['--discard', stream]
            # The decoder writes its frames to stdout, which `timed` sends
            # to /dev/null. It has no output-file option; passing one
            # would be rejected.
            out[tag(args, f'{name}_write', n)] = argv + [stream]
    return out


def pairings(args):
    out = {}
    for n in args.threads:
        for name, _ in args.decoders:
            t = lambda c: tag(args, c, n)
            out[t(f'{name}:discard')] = (t(f'{name}_discard'), t('ff_null'))
            out[t(f'{name}:write')] = (t(f'{name}_write'), t('ff_raw'))
            # Not symmetric, and reported so that the cost of the
            # asymmetry is visible rather than assumed: earlier rounds
            # used it.
            out[t(f'{name}:mixed')] = (t(f'{name}_discard'), t('ff_raw'))
    return out


def scalings(args):
    """For each pairing at N threads, the same pairing at 1 thread."""
    out = {}
    if len(args.threads) == 1 or 1 not in args.threads:
        return out
    for n in args.threads:
        if n == 1:
            continue
        for name, _ in args.decoders:
            for kind in ('discard', 'write'):
                out[f'scaling:{name}:{kind}@{n}'] = (
                    f'{name}:{kind}@{n}', f'{name}:{kind}@1')
    return out


def machine():
    """The CPU model, the clock it reports and the logical CPU count, from
    /proc/cpuinfo (empty where the file is missing)."""
    try:
        text = open('/proc/cpuinfo').read()
    except OSError:
        return {'cpu_model': '', 'cpu_mhz': None, 'logical_cpus': os.cpu_count()}
    models = [line.split(':', 1)[1].strip() for line in text.splitlines()
              if line.startswith('model name')]
    mhz = [float(line.split(':', 1)[1]) for line in text.splitlines()
           if line.startswith('cpu MHz')]
    def first(key):
        for line in text.splitlines():
            if line.split(':', 1)[0].strip() == key:
                return line.split(':', 1)[1].strip()
        return ''
    # A hypervisor may show one generic model string for different parts;
    # family, model and stepping tell them apart.
    return {'cpu_model': models[0] if models else '',
            'cpu_family': first('cpu family'), 'cpu_model_number': first('model'),
            'cpu_stepping': first('stepping'),
            'cpu_mhz': round(statistics.median(mhz), 1) if mhz else None,
            'logical_cpus': len(models) or os.cpu_count()}


def repeat_for(args, warm):
    """How many times every command of a stream is run back to back in one
    timed call, so that the fastest lasts at least `--min-seconds`; 1 when
    the option is not given or a warm-up failed."""
    if not args.min_seconds:
        return 1
    if any(w is None for w in warm.values()):
        print('a warm-up failed: this stream is timed once per call', flush=True)
        return 1
    return max(1, math.ceil(args.min_seconds / max(min(warm.values()), 1e-6)))


def time_stream(args, cmds, stream_name, attempt=0, repeat=None):
    """One attempt at one stream: a warm-up of each command, then the
    interleaved rounds, in a per-round random order when a seed is given
    (drawn from the seed, the stream's name and the attempt, so that a
    retry does not replay the order that was faulty). A timed value is of
    one run: the call repeats the command `repeat` times (from the warm-up
    when not given: the same for every command of the stream).
    Returns (times by command, order of each round, repeat) or None if a
    command failed."""
    warm = {name: timed(argv) for name, argv in cmds.items()}   # not recorded
    if repeat is None:
        repeat = repeat_for(args, warm)
    times = {name: [] for name in cmds}
    orders = []
    rng = None
    if args.shuffle_seed is not None:
        rng = random.Random(f'{args.shuffle_seed}:{stream_name}:{attempt}')
    for _ in range(args.rounds):
        order = list(cmds)
        if rng is not None:
            rng.shuffle(order)
        orders.append(order)
        for name in order:
            value = timed(cmds[name], repeat)
            if value is None:
                return None
            times[name].append(value)
    return times, orders, repeat


def thresholds(args):
    """(null deviation, round spread) the verdict acts on: those of one
    pinned thread, or the looser ones of a call with several threads, or the
    ones the call is given for a host whose noise is known to be lower."""
    if getattr(args, 'null_bar', None) or getattr(args, 'spread_bar', None):
        base = NULL_DEVIATION_MT if max(args.threads) > 1 else NULL_DEVIATION
        spread = ROUND_SPREAD_MT if max(args.threads) > 1 else ROUND_SPREAD
        return args.null_bar or base, args.spread_bar or spread
    if max(args.threads) > 1:
        return NULL_DEVIATION_MT, ROUND_SPREAD_MT
    return NULL_DEVIATION, ROUND_SPREAD


def judge(args, times):
    """The host-fault verdict of one attempt: the null arm's deviation per
    output mode and the round spread of every command. Both are recorded
    always; only a call with `--null-arm` acts on them, so that a call
    without it times exactly what it did before."""
    deviation, spread_bar = thresholds(args)
    verdict = {'null': {}, 'spread': {}, 'faulty': False}
    for name, v in times.items():
        spread = max(v) / min(v)
        verdict['spread'][name] = spread
        if args.null_arm and spread > spread_bar:
            verdict['faulty'] = True
    if args.null_arm:
        for name in times:
            if '~null' not in name:
                continue
            base = name.replace('~null', '')
            dev = statistics.median(times[name]) / statistics.median(times[base]) - 1
            verdict['null'][base] = dev
            if abs(dev) > deviation:
                verdict['faulty'] = True
    return verdict


def gate_sample(args, repeat=None):
    """FFmpeg's time on the reference stream: the median of GATE_RUNS timed
    calls of one run each, with `--gate-threads` threads, pinned like the
    other commands or to `--gate-pin`. With `--min-seconds` a call repeats the
    run, as the arms' calls do (only with `--gate-threads`), K from a first run (given in `repeat` after the
    first sample). Returns (seconds per run, repeat)."""
    prefix = args.prefix
    if args.gate_pin is not None:
        prefix = ['taskset', '-c', str(args.gate_pin)]
    argv = prefix + [args.ffmpeg, '-nostdin', '-v', 'error', '-threads', str(args.gate_threads or 1),
                     '-i', args.gate_stream, '-frames:v', str(args.gate_frames),
                     '-fps_mode', 'passthrough', '-f', 'null', '-']
    if repeat is None:
        repeat = 1
        if args.gate_threads is not None and args.min_seconds:
            probe = timed(argv)
            if probe is None:
                raise SystemExit('the gate stream failed to decode: no report')
            repeat = repeat_for(args, {'gate': probe})
    values = [timed(argv, repeat) for _ in range(GATE_RUNS)]
    if any(v is None for v in values):
        raise SystemExit('the gate stream failed to decode: no report')
    return statistics.median(values), repeat


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--decoder', action='append', required=True,
                    metavar='NAME=PATH',
                    help='repeatable; every decoder is timed in the same '
                         'rounds as the others and as FFmpeg')
    ap.add_argument('--ffmpeg', default='ffmpeg')
    ap.add_argument('--frames', type=int, default=300)
    ap.add_argument('--rounds', type=int, default=3)
    ap.add_argument('--report')
    ap.add_argument('--pin', type=int, metavar='CORE',
                    help='run every command on this core (taskset -c)')
    ap.add_argument('--threads', default='1', metavar='N[,N...]',
                    help='threads on both sides: ffmpeg -threads N, ours '
                         '--threads N (default 1, the single-threaded '
                         'commands); a list times each count in the same '
                         'rounds')
    ap.add_argument('--require-cpu', metavar='TEXT',
                    help='a gating run: stop unless the CPU model contains TEXT')
    ap.add_argument('--shuffle-seed', type=int, metavar='N',
                    help='draw the order of the commands anew in each round '
                         '(default: the fixed order)')
    ap.add_argument('--null-arm', metavar='NAME',
                    help='time a second copy of decoder NAME, and time a stream again '
                         'where the copy deviates by more than 4%% (8%% with several '
                         "threads) or a command's round spread exceeds 1.50")
    ap.add_argument('--gate-stream', metavar='PATH',
                    help='a reference stream timed with FFmpeg inside the call')
    ap.add_argument('--gate-every', type=int, default=4, metavar='N',
                    help='streams between in-call gate samples (default 4)')
    ap.add_argument('--min-seconds', type=float, metavar='S',
                    help='run every command of a stream K times back to back '
                         'in each timed call, K the smallest count that makes '
                         'the fastest call last S seconds (the same K for '
                         'every command); times stay per run')
    ap.add_argument('--null-bar', type=float, metavar='X',
                    help='the null-arm deviation bar of this call, instead of the '
                         'one for its thread count (docs/design/h264.md, "Where '
                         'the eight-thread arms run")')
    ap.add_argument('--spread-bar', type=float, metavar='X',
                    help='the round-spread bar of this call (max/min of a '
                         "command's rounds), likewise")
    ap.add_argument('--gate-threads', type=int, metavar='N',
                    help='threads of the in-call gate, and with --min-seconds its '
                         'calls repeat like the arms (default: one thread, as '
                         'before); N for a call of N threads, so that the gate '
                         'sees what the arms see')
    ap.add_argument('--gate-pin', type=int, metavar='CORE',
                    help='run the in-call gate on this core (taskset -c), also in a call '
                         'that is not pinned: nothing else runs while it is sampled')
    ap.add_argument('--gate-frames', type=int, default=300, metavar='F',
                    help='frames of the gate stream (default 300)')
    ap.add_argument('streams', nargs='+')
    args = ap.parse_args()
    host = machine()
    print(f"machine: {host['cpu_model']} (family {host['cpu_family']}, model "
          f"{host['cpu_model_number']}, stepping {host['cpu_stepping']}), {host['cpu_mhz']} MHz, "
          f"{host['logical_cpus']} logical CPUs", flush=True)
    if args.require_cpu and args.require_cpu not in host['cpu_model']:
        raise SystemExit(f'the CPU model is not the bar machine\'s '
                         f'({args.require_cpu!r}): not timing anything')
    try:
        args.threads = [int(n) for n in args.threads.split(',')]
    except ValueError:
        ap.error('--threads takes a number or a comma-separated list')
    if min(args.threads) < 1 or len(set(args.threads)) != len(args.threads):
        ap.error('--threads counts must be distinct and at least 1')
    if args.min_seconds is not None and not (0 < args.min_seconds < float('inf')):
        ap.error('--min-seconds must be a positive number')
    if args.gate_threads is not None:
        if args.gate_threads < 1:
            ap.error('--gate-threads must be at least 1')
        if not args.gate_stream:
            ap.error('--gate-threads needs --gate-stream')
        if args.pin is not None:
            ap.error('--pin puts every thread on one core; leave it out with --gate-threads')
    for name in ('null_bar', 'spread_bar'):
        v = getattr(args, name)
        if v is not None and not (0 < v < float('inf')):
            ap.error(f'--{name.replace("_", "-")} must be a positive number')
    if args.min_seconds and len(args.threads) > 1:
        ap.error('--min-seconds takes one thread count: K follows the fastest '
                 'command, so the other counts would last much longer')
    if max(args.threads) > 1 and args.pin is not None:
        ap.error('--pin puts every thread on one core; leave it out with --threads')
    args.decoders = [tuple(d.split('=', 1)) if '=' in d else ('ours', d)
                     for d in args.decoder]
    names = [n for n, _ in args.decoders]
    if args.null_arm and args.null_arm not in names:
        ap.error(f'--null-arm {args.null_arm!r} is not one of the decoders {names}')
    if args.gate_every < 1:
        ap.error('--gate-every must be at least 1')
    args.prefix = ['taskset', '-c', str(args.pin)] if args.pin is not None else []
    pairs = pairings(args)
    scales = scalings(args)

    rows = []
    paths = []              # the stream path of each row
    gates = []
    gate_base = None
    gate_repeat = None
    gate_stable = None
    last_good = 0           # rows before this index passed a gate sample
    attempts_allowed = MAX_RETRIES + 1

    def take(after, kind):
        """One gate sample, recorded, and whether it is within GATE_DRIFT of the baseline."""
        nonlocal gate_repeat
        value, gate_repeat = gate_sample(args, gate_repeat)
        drift = value / gate_base - 1
        gates.append({'after_streams': after, 'seconds': value, 'repeat': gate_repeat, 'kind': kind,
                      'drift': drift, 'drifted': abs(drift) > GATE_DRIFT})
        print(f'gate after {after} streams ({kind}): {value:.3f} s ({drift:+.1%})', flush=True)
        return abs(drift) <= GATE_DRIFT

    def baseline():
        """The call's reference: the median of the last GATE_BASELINE samples once they agree within
        GATE_DRIFT, so that one or two outliers at the start (a call whose first sample came out 6%
        low had it for the baseline of every later one) are not it. Up to 2 * GATE_BASELINE samples;
        if they never agree the median of the last ones is used and `gate_baseline_stable` is False."""
        nonlocal gate_base, gate_repeat, gate_stable
        values = []
        for _ in range(2 * GATE_BASELINE):
            value, gate_repeat = gate_sample(args, gate_repeat)
            values.append(value)
            last = values[-GATE_BASELINE:]
            if len(last) == GATE_BASELINE and max(last) / min(last) - 1 <= GATE_DRIFT:
                break
        last = values[-GATE_BASELINE:]
        gate_stable = max(last) / min(last) - 1 <= GATE_DRIFT
        gate_base = statistics.median(last)
        for value in values:
            drift = value / gate_base - 1
            gates.append({'after_streams': 0, 'seconds': value, 'repeat': gate_repeat, 'kind': 'baseline',
                          'drift': drift, 'drifted': abs(drift) > GATE_DRIFT})
        print(f'gate baseline: {gate_base:.3f} s (samples {", ".join(f"{v:.3f}" for v in values)})'
              + ('' if gate_stable else ' NOT STABLE'), flush=True)

    def sample(after):
        """A check of the gate: a sample beyond GATE_DRIFT is taken again at once, and counts as a drift
        only if the second is beyond it too (one sample can be an outlier, a level shift is not)."""
        return take(after, 'check') or take(after, 'confirm')

    def measure(stream, used, repeat=None):
        """Time one stream, from attempt number `used`, until it passes the
        verdict or its attempts (the first and MAX_RETRIES retries) are spent. Returns
        the attempts, or None if a command failed."""
        name = os.path.basename(stream)
        cmds = commands(args, stream)
        if args.null_arm:
            for n in args.threads:
                for mode in ('discard', 'write'):
                    cmds[tag(args, f'{args.null_arm}~null_{mode}', n)] = \
                        cmds[tag(args, f'{args.null_arm}_{mode}', n)]
        cmds = {k: args.prefix + argv for k, argv in cmds.items()}
        attempts = []
        for attempt in range(used, attempts_allowed):
            got = time_stream(args, cmds, name, attempt, repeat)
            if got is None:
                return None
            times, orders, repeat = got
            verdict = judge(args, times)
            attempts.append({'rounds': times, 'order': orders, 'repeat': repeat, **verdict})
            if not verdict['faulty']:
                break
            print(f'{name[:40]:40} host fault (attempt {attempt + 1}): '
                  f'null {verdict["null"]}, max spread '
                  f'{max(verdict["spread"].values()):.3f}', flush=True)
        return attempts

    def finish(name, attempts):
        """The row of a stream from all its attempts: the last one is what
        is summarised, every one is kept in `history`."""
        last = attempts[-1]
        times = last['rounds']
        median = {k: statistics.median(v) for k, v in times.items()
                  if '~null' not in k}
        row = {'stream': name, 'class': class_of(name), 'seconds': median,
               'ratio': {p: median[a] / median[b] for p, (a, b) in pairs.items()},
               'rounds': times, 'order': last['order'], 'repeat': last['repeat'],
               'null': last['null'], 'spread': last['spread'],
               'attempts': len(attempts), 'unresolved': last['faulty'],
               'history': attempts}
        for s_, (a, b) in scales.items():
            row['ratio'][s_] = row['ratio'][a] / row['ratio'][b]
        return row

    def retime(first, end):
        """Time again the rows first..end, which ran while the host drifted,
        within each stream's attempts."""
        for i in range(first, end):
            old = rows[i]
            more = None
            if old['attempts'] < attempts_allowed:
                more = measure(paths[i], old['attempts'], old['repeat'])
            if more is None:
                old['unresolved'] = True
                old['gate_drift'] = True
                continue
            rows[i] = finish(old['stream'], old['history'] + more)
            rows[i]['gate_drift'] = True
            print(f'{old["stream"][:40]:40} timed again after gate drift', flush=True)

    if args.gate_stream:
        # A first run is cold (caches, a file not yet read, clocks): the
        # baseline every later sample is compared with is taken after one
        # sample that is thrown away, or the call would read the warm-up as
        # a drift of every sample after it.
        gate_sample(args)
        baseline()
    for index, stream in enumerate(args.streams):
        attempts = measure(stream, 0)
        if attempts is None:
            print(f'{os.path.basename(stream)}: FAILED, skipped', flush=True)
        else:
            row = finish(os.path.basename(stream), attempts)
            rows.append(row)
            paths.append(stream)
            flag = ' UNRESOLVED' if row['unresolved'] else ''
            shown = '  '.join(f'{p}={row["ratio"][p]:.2f}' for p in pairs)
            print(f'{row["stream"][:40]:40} {shown}{flag}', flush=True)
        if args.gate_stream and ((index + 1) % args.gate_every == 0
                                 or index == len(args.streams) - 1):
            done = len(rows)
            if not sample(done):
                retime(last_good, done)
                if not sample(done):
                    # still drifted: what was timed since the last good
                    # sample is flagged, not trusted
                    for r in rows[last_good:done]:
                        r['unresolved'] = True
            last_good = done

    print()
    now = machine()
    if args.require_cpu and any(now[k] != host[k] for k in
                                ('cpu_model', 'cpu_family', 'cpu_model_number', 'cpu_stepping')):
        raise SystemExit('the CPU changed during the run: no report')
    report = {'machine': host, 'required_cpu': args.require_cpu,
              'frames': args.frames, 'rounds': args.rounds, 'pinned_core': args.pin,
              'threads': args.threads,
              'decoders': dict(args.decoders), 'ffmpeg': args.ffmpeg,
              'resamples': RESAMPLES, 'streams': rows, 'summary': {},
              'method': {'shuffle_seed': args.shuffle_seed,
                         'null_arm': args.null_arm,
                         'null_deviation': thresholds(args)[0],
                         'round_spread': thresholds(args)[1],
                         'max_retries': MAX_RETRIES,
                         'min_seconds': args.min_seconds,
                         'gate_stream': args.gate_stream,
                         'gate_every': args.gate_every if args.gate_stream else None,
                         'gate_drift_threshold': GATE_DRIFT, 'gates': gates,
                         'unresolved': sum(r['unresolved'] for r in rows),
                         'retried': sum(r['attempts'] > 1 for r in rows),
                         'gate_base': gate_base, 'gate_baseline_stable': gate_stable,
                         'gate_drift_streams': sum(bool(r.get('gate_drift')) for r in rows)}}
    for pairing in list(pairs) + list(scales):
        values = [r['ratio'][pairing] for r in rows]
        low, high = interval(values, seed=SEED)
        report['summary'][pairing] = {
            'n': len(values), 'median': statistics.median(values),
            'ci_low': low, 'ci_high': high}
        band = '' if low is None else f'  [{low:.3f}, {high:.3f}]'
        print(f'{pairing:24} n={len(values):3}  median '
              f'{statistics.median(values):.3f}{band}')
    if args.report:
        with open(args.report, 'w') as handle:
            json.dump(report, handle, indent=2)
        print(f'\nwrote {args.report}')


if __name__ == '__main__':
    main()
