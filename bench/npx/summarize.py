"""Summarizes a bench/npx run against the bars of docs/design/npm-cli.md.

    python3 -I summarize.py OUT

Per case (file and operation), each command's time is the median of its
rounds; a case with a failed or timed-out round of any command is counted
apart ("timing_failed"), as are files the native command refuses and files
FFmpeg fails on. Statistics are bench/shared/benchlib.py's: medians
over files, bootstrap 95% intervals over files, 10,000 resamples, seed
20261018. Prints JSON."""
import json
from pathlib import Path
import statistics as st
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'shared'))
from benchlib import boot, dist, lines  # noqa: E402

OPS = ('probe', 'check', 'chapters', 'extract-audio', 'clip-head', 'clip-middle', 'remux', 'sanitize', 'split',
       'extract-subtitles')
# A class is judged when it has at least this many files.
CLASS_FLOOR = 10
SMALL = 16 << 20


def classes(record):
    container = 'matroska' if record['ext'] in ('mkv', 'webm') else 'isobmff'
    size = 'under 16 MiB' if record['size'] < SMALL else '16 MiB or more'
    return [container, size]


def summarize(directory):
    out_dir = Path(directory)
    meta = json.loads((out_dir / 'meta.json').read_text())
    records = lines(out_dir / 'results.jsonl')
    overhead = meta['npm_overhead']
    npm_exec = st.median(overhead['npx']) - st.median(overhead['installed'])
    out = {'files': len(records), 'path': meta.get('path', 'wasi'), 'npm_exec_seconds': npm_exec,
           'npx_version_seconds': st.median(overhead['npx']),
           'installed_version_seconds': st.median(overhead['installed']),
           # `--version` through each way to start the command.
           'launcher_costs_seconds': {key: st.median(values) for key, values in overhead.items()}}
    unequal = [{'sha256': r['sha256'][:12], 'op': o['op'], **o.get('difference', {})}
               for r in records for o in r['ops'] if not o['equal']]
    traps = sum(1 for r in records for o in r['ops'] if (o.get('difference') or {}).get('codes', [None, None])[1] == 70)
    # Skipped cases (middle clips without a duration) ran nothing.
    cases = sum(1 for r in records for o in r['ops'] if o['status'] != 'skipped')
    out['correctness'] = {'cases': cases, 'unequal': unequal, 'traps': traps,
                          'met': not unequal and traps == 0}
    met_all = True
    for op in OPS:
        counted = {'coreav_refused': 0, 'ffmpeg_failed': 0, 'skipped': 0, 'timing_failed': 0}
        ratios, by_class = [], {}
        native_ratios, overhead_ratios, npx_ratios = [], [], []
        seconds = {'launcher': [], 'native': [], 'ffmpeg': []}
        for r in records:
            case = next((o for o in r['ops'] if o['op'] == op), None)
            if case is None:
                continue
            if case['status'] != 'timed':
                counted[case['status']] += 1
                continue
            times = case['times']
            if any(t is None for tool in times.values() for t in tool):
                counted['timing_failed'] += 1
                continue
            m = {tool: st.median(values) for tool, values in times.items()}
            for tool in seconds:
                seconds[tool].append(m[tool])
            ratio = m['launcher'] / m['ffmpeg']
            ratios.append(ratio)
            native_ratios.append(m['native'] / m['ffmpeg'])
            overhead_ratios.append(m['launcher'] / m['native'])
            npx_ratios.append((m['launcher'] + npm_exec) / m['ffmpeg'])
            for name in classes(r):
                by_class.setdefault(name, []).append(ratio)
        judged = {k: st.median(v) <= 1.0 for k, v in by_class.items() if len(v) >= CLASS_FLOOR}
        # An operation, like a class, is judged with at least CLASS_FLOOR files.
        met = None
        if len(ratios) >= CLASS_FLOOR:
            met = st.median(ratios) <= 1.0 and all(judged.values())
            met_all = met_all and met
        ci = boot(ratios, st.median)
        out[op] = {
            'timed': len(ratios), 'counted_apart': counted,
            'launcher_vs_ffmpeg': dist(ratios),
            'by_class': {k: {**dist(v), 'judged': len(v) >= CLASS_FLOOR} for k, v in sorted(by_class.items())},
            'judged': met is not None, 'bar_met': met, 'classes_met': judged,
            'faster_claim': ci[1] is not None and ci[1] < 1.0,
            'reported': {'native_vs_ffmpeg': dist(native_ratios), 'launcher_vs_native': dist(overhead_ratios),
                         'npx_vs_ffmpeg': dist(npx_ratios),
                         'seconds': {tool: dist(values) for tool, values in seconds.items()}},
        }
    out['speed_bars_met'] = met_all
    return out


def main():
    if len(sys.argv) != 2:
        raise SystemExit('usage: summarize.py OUT')
    print(json.dumps(summarize(sys.argv[1]), indent=1, sort_keys=True))


if __name__ == '__main__':
    main()
