#!/usr/bin/env python3
"""Diagnosis: one thread, pinned, perf stat counts of each arm on the 32 training streams, arms in a shuffled order
for each stream and round. Writes out/stat.jsonl (one line a call) and out/topdown.jsonl."""
import json, os, random, subprocess, sys

ARMS = ['base', 'a2', 'p2', 'p4', 'p14', 'k']
FRAMES = {'sd': 600, '720p': 300, '1080p': 100, '4k': 50}
EVENTS = 'cycles:u,instructions:u,branch-misses:u,ld_blocks.store_forward:u,resource_stalls.sb:u'
streams = sorted(f for f in os.listdir('es') if f.endswith('.h264'))
rng = random.Random(20261030)


def stat(arm, s, extra, out):
    n = FRAMES[s.split('.')[2]]
    cmd = ['perf', 'stat', '-x', ',', '-o', out] + extra + ['taskset', '-c', '2', f'./decode-{arm}', '--threads', '1',
                                                          '--frames', str(n), '--discard', f'es/{s}']
    r = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    if r.returncode != 0:
        raise SystemExit(f'{arm} {s} rc={r.returncode} {r.stderr[:300]}')
    return [l.rstrip('\n') for l in open(out) if l.strip() and not l.startswith('#')]


def run(kind, rounds, extra, path):
    with open(path, 'a') as o:
        for r in range(rounds):
            for s in streams:
                order = ARMS[:]
                rng.shuffle(order)
                for arm in order:
                    rows = stat(arm, s, extra, 'out/tmp.csv')
                    o.write(json.dumps({'round': r, 'stream': s, 'arm': arm, 'rows': rows}) + '\n')
            print(kind, 'round', r, 'done', flush=True)


run('stat', int(sys.argv[1]), ['-e', EVENTS], 'out/stat.jsonl')
run('topdown', int(sys.argv[2]), ['-M', 'TopdownL1,TopdownL2'], 'out/topdown.jsonl')
