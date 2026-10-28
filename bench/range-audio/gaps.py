"""The gaps between the regions extraction advises (README.md, "Tuning"),
from the audio packets ffprobe lists for each object over a presigned URL
(`-show_packets`, black box), through the copy window's decisions
(`crates/coreav-isobmff-write/src/copy.rs`: a 64 KiB refill from a
sample's start, or ending at the end of the file; larger samples alone).

    python3 -I gaps.py INPUTS OUT.json [--only SHA,...]

For each object: the number of regions, their bytes as a share of the
object, and the gap sizes' quantiles."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'shared'))
from benchlib import R2  # noqa: E402

WINDOW = 64 << 10


def regions(packets, total):
    out, start, held = [], 0, 0
    for pos, size in packets:
        if size > WINDOW:
            held = 0
            reads = (pos, size)
        elif start <= pos and pos + size <= start + held:
            continue
        else:
            start = min(pos, max(0, total - WINDOW))
            held = min(total - start, WINDOW)
            reads = (start, held)
        if out and reads[0] <= out[-1][0] + out[-1][1]:
            out[-1][1] = max(out[-1][0] + out[-1][1], reads[0] + reads[1]) - out[-1][0]
        else:
            out.append(list(reads))
    return out


def quantiles(values):
    values = sorted(values)
    if not values:
        return {}
    pick = lambda q: values[min(len(values) - 1, int(q * len(values)))]  # noqa: E731
    return {'p10': pick(0.1), 'p50': pick(0.5), 'p90': pick(0.9), 'max': values[-1]}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('inputs')
    parser.add_argument('out')
    parser.add_argument('--stream', type=int, default=None)
    parser.add_argument('--only', default='')
    args = parser.parse_args()
    files = json.loads(Path(args.inputs).read_text())['files']
    if args.only:
        files = [f for f in files if any(f['sha256'].startswith(p) for p in args.only.split(','))]
    r2 = R2()
    result = {}
    for f in files:
        url = r2.presign(f['key'])
        done = subprocess.run(['ffprobe', '-v', 'error', '-select_streams', 'a:0', '-show_entries',
                               'packet=pos,size', '-of', 'compact=p=0:nk=0', url], capture_output=True,
                              text=True, timeout=3600)
        packets = []
        for line in done.stdout.splitlines():
            fields = dict(part.split('=', 1) for part in line.split('|') if '=' in part)
            if fields.get('pos', 'N/A') not in ('', 'N/A'):
                packets.append((int(fields['pos']), int(fields['size'])))
        packets.sort()
        reg = regions(packets, f['size'])
        gaps = [b[0] - (a[0] + a[1]) for a, b in zip(reg, reg[1:])]
        result[f['sha256']] = {'size': f['size'], 'packets': len(packets),
                               'regions': len(reg), 'share': sum(r[1] for r in reg) / f['size'],
                               'gaps': quantiles(gaps),
                               'gaps_over': {f'{m}MiB': sum(g >= m << 20 for g in gaps) for m in (1, 2, 4, 8)}}
        print(f['sha256'][:12], json.dumps(result[f['sha256']]), file=sys.stderr, flush=True)
    Path(args.out).write_text(json.dumps(result, indent=1) + '\n')


if __name__ == '__main__':
    main()
