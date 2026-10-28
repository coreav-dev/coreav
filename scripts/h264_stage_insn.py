#!/usr/bin/env python3
"""Instructions per macroblock by decode stage, from a Callgrind profile.

Instruction counts are exact, unlike the sampled shares a time profile
gives, so stages can be compared between two decoders that run at
different speeds. Normalising per macroblock makes the number independent
of the stream's length and resolution.

    h264_stage_insn.py --callgrind cg.out --macroblocks 1747200 [--label ours]
"""
import argparse
import json
import os
import re
import subprocess

VALGRIND_LIB = os.environ.get('VALGRIND_LIB', '/tmp/vgroot/usr/libexec/valgrind')
ANNOTATE = os.environ.get('CALLGRIND_ANNOTATE',
                          '/tmp/vgroot/usr/bin/callgrind_annotate')

# Order matters: a function joins the first stage that claims it.
STAGES = [
    ('chroma MC', ['chroma_mc']),
    ('deblocking', ['deblock', 'filter_edge', 'motion_strength']),
    ('luma MC', ['luma_mc', 'vertical_tap', 'tap_row', 'centre_tap']),
    ('CABAC', ['cabac', 'Cabac', 'coded_block_flag', 'CabacSyntax']),
    ('CAVLC', ['cavlc', 'Cavlc']),
    ('intra prediction', ['intra', 'pred_chroma', 'pred16', 'pred4', 'pred8',
                          'neighbours']),
    ('MC control + fetch', ['motion_compensate', 'inter_pred', 'fetch',
                            'reconstruct_inter']),
    ('IDCT + residual', ['idct', 'transform']),
    ('libc mem*', ['memset', 'memcpy', 'memmove']),
]

ROW = re.compile(r'\s*([\d,]+)\s+\([ \d.]+%\)\s+(.*)$')


def functions(path):
    """instructions per function name, excluding the totals row.

    A binary with debug info splits one function across several source
    files, because inlined code is attributed to the file it came from.
    Counts are therefore summed per function name, not taken per row.
    """
    env = dict(os.environ, VALGRIND_LIB=VALGRIND_LIB)
    out = subprocess.run([ANNOTATE, '--threshold=99.9', path],
                         capture_output=True, text=True, env=env).stdout
    # Only the function table counts. A binary with debug info makes
    # callgrind_annotate print annotated source lines further down in the
    # same number format, which would otherwise be counted a second time.
    lines = out.splitlines()
    try:
        start = next(i for i, l in enumerate(lines)
                     if re.match(r'\s*Ir\s+file:function', l))
    except StopIteration:
        start = 0
    table = []
    for line in lines[start + 1:]:
        if line.startswith('--'):
            continue
        if not line.strip():
            break
        table.append(line)

    totals = {}
    for line in table:
        m = ROW.match(line)
        if not m:
            continue
        rest = m.group(2)
        if 'PROGRAM TOTALS' in rest:
            continue
        name = rest.split(':', 1)[1] if ':' in rest else rest
        name = name.rsplit('[', 1)[0].strip()
        totals[name] = totals.get(name, 0) + int(m.group(1).replace(',', ''))
    return [(n, name) for name, n in totals.items()]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--callgrind', required=True)
    ap.add_argument('--macroblocks', type=int, required=True)
    ap.add_argument('--label', default='decoder')
    ap.add_argument('--total', type=int,
                    help='exact I refs, if known, for the residual row')
    ap.add_argument('--report')
    args = ap.parse_args()

    rows = functions(args.callgrind)
    total = args.total or sum(n for n, _ in rows)
    claimed, out = set(), []
    for stage, keys in STAGES:
        n = 0
        for count, name in rows:
            if name in claimed:
                continue
            if any(k in name for k in keys):
                n += count
                claimed.add(name)
        # claimed holds names, and each name appears once after summing
        out.append((stage, n))
    accounted = sum(n for _, n in out)
    out.append(('everything else', total - accounted))

    print(f'{args.label}: {total/1e9:.2f} G instructions, '
          f'{args.macroblocks:,} macroblocks')
    print(f'{"stage":22} {"Ginsn":>7} {"share":>7} {"insn/MB":>9}')
    for stage, n in out:
        print(f'{stage:22} {n/1e9:7.2f} {n/total*100:6.1f}% '
              f'{n/args.macroblocks:9.0f}')
    print(f'{"TOTAL":22} {total/1e9:7.2f} {100.0:6.1f}% '
          f'{total/args.macroblocks:9.0f}')

    if args.report:
        with open(args.report, 'w') as handle:
            json.dump({'label': args.label, 'instructions': total,
                       'macroblocks': args.macroblocks,
                       'stages': {s: n for s, n in out}}, handle, indent=2)


if __name__ == '__main__':
    main()
