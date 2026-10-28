#!/usr/bin/env python3
"""Instruction and simulated cache/branch counts for a decoder command.

Hardware counters are unavailable on the measurement machine (no CPU PMU
under the hypervisor), so counts come from Valgrind's cachegrind:
instructions retired are counted exactly, while cache and branch figures
are a simulator's model and are labelled as such wherever they are used.

Usage:
    h264_counts.py --label ffmpeg -- ffmpeg -i in.h264 -fps_mode passthrough -f rawvideo -
"""
import argparse
import json
import os
import re
import statistics
import subprocess
import sys
import time

VALGRIND = os.environ.get('VALGRIND_BIN', '/tmp/vgroot/usr/bin/valgrind')
VALGRIND_LIB = os.environ.get('VALGRIND_LIB', '/tmp/vgroot/usr/libexec/valgrind')

FIELDS = {
    'I   refs': 'instructions',
    'D   refs': 'data_refs',
    'D1  misses': 'd1_misses',
    'LLd misses': 'lld_misses',
    'LL misses': 'll_misses',
    'Branches': 'branches',
    'Mispredicts': 'mispredicts',
}


def counts(argv):
    """Run argv under cachegrind and return its counters."""
    env = dict(os.environ, VALGRIND_LIB=VALGRIND_LIB)
    proc = subprocess.run(
        [VALGRIND, '--tool=cachegrind', '--branch-sim=yes',
         '--cachegrind-out-file=/dev/null', *argv],
        stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=env, text=True)
    if proc.returncode != 0:
        raise SystemExit(f'command failed under valgrind: {argv}\n{proc.stderr[-2000:]}')
    out = {}
    for line in proc.stderr.splitlines():
        # "==123== I   refs:      1,234,567" and the branch lines, whose
        # totals are followed by a "( Nc + Ni )" breakdown we ignore.
        m = re.match(r'==\d+==\s+(.+?):\s+([\d,]+)', line)
        if not m:
            continue
        name = FIELDS.get(m.group(1).strip())
        if name and name not in out:
            out[name] = int(m.group(2).replace(',', ''))
    missing = set(FIELDS.values()) - set(out)
    if missing:
        raise SystemExit(f'cachegrind did not report {sorted(missing)}')
    return out


def seconds(argv, reps):
    """Median wall-clock of argv over reps normal (not instrumented) runs."""
    times = []
    for _ in range(reps):
        start = time.perf_counter()
        with open(os.devnull, 'wb') as sink:
            proc = subprocess.run(argv, stdout=sink, stderr=subprocess.DEVNULL)
        if proc.returncode != 0:
            raise SystemExit(f'command failed: {argv}')
        times.append(time.perf_counter() - start)
    return statistics.median(times)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--label', required=True)
    ap.add_argument('--reps', type=int, default=5)
    ap.add_argument('--out')
    ap.add_argument('command', nargs=argparse.REMAINDER)
    args = ap.parse_args()
    argv = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not argv:
        raise SystemExit('no command given')

    row = {'label': args.label, 'command': argv}
    row['seconds'] = seconds(argv, args.reps)
    row.update(counts(argv))
    # Instructions per second is measured; IPC needs a clock this VM does
    # not expose, so the ratio of these is what the report compares.
    row['instructions_per_second'] = row['instructions'] / row['seconds']
    print(json.dumps(row))
    if args.out:
        with open(args.out, 'a') as handle:
            handle.write(json.dumps(row) + '\n')


if __name__ == '__main__':
    main()
