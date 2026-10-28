"""Summary of run.py runs against the bars of docs/design/range-extraction.md
("Bars").

    python3 -I summarize.py SETS RUN_DIR [RUN_DIR ...] [--out FILE]

SETS is a held-out set, {"held_out": [...]} as sets.py writes it: every
held-out object must be accounted for. The runs may each hold some engines
(run.py --engines), each engine in exactly one run, a held-out run
(`--held-out`) from a clean tree; for each engine, its rows come from the
run that measured it, with that run's FFmpeg times.
An object an engine fails on, or whose output differs, fails E-1 and the
engine's speed bar. Per
object and engine, the median of its runs' seconds; the ratio of each
engine's to FFmpeg's, over the objects where the native
build extracted the track, FFmpeg succeeded in every run and the engine
succeeded in every run (warm-up included) with the native build's output;
every other object is listed with its reason. Medians over objects with
bootstrap 95% intervals (10,000 resamples), overall and per class; bytes
received as a share of the object and requests, reported. An engine whose
records hold an extent run (`ENGINE_extent`, the extent advice of before)
also gets its bytes, requests and time against that run.

Other options come from an `extra_summaries` module beside this file, when
this tree has one (its `options(parser)` and `summaries(args, held, runs)`,
which adds keys to the result)."""
import argparse
import json
import sys
import random
import statistics
from pathlib import Path

LARGE = 64 << 20
CLASSES = {
    'audio-only': lambda r: r['stratum'] == 'audio-only',
    'with video': lambda r: r['stratum'] != 'audio-only',
    'up to 64 MiB': lambda r: r['size'] <= LARGE,
    'over 64 MiB': lambda r: r['size'] > LARGE,
}


def bootstrap(values, rounds=10000, seed=31):
    rng = random.Random(seed)
    medians = sorted(statistics.median(rng.choices(values, k=len(values))) for _ in range(rounds))
    return [medians[int(0.025 * rounds)], medians[int(0.975 * rounds)]]


def stats(values):
    if not values:
        return {'n': 0}
    out = {'n': len(values), 'median': statistics.median(values), 'max': max(values), 'min': min(values)}
    if len(values) > 1:
        out['ci95'] = bootstrap(values)
    return out


def load(run):
    meta = json.loads((Path(run) / 'meta.json').read_text())
    if 'engines' not in meta:
        raise SystemExit(f'{run}: meta.json names no engines')
    records = {}
    for line in (Path(run) / 'results.jsonl').read_text().splitlines():
        if line.strip():
            record = json.loads(line)
            if record['sha256'] in records:
                raise SystemExit(f'{run}: {record["sha256"]} appears twice')
            records[record['sha256']] = record
    return meta, records


def all_runs(record, engine):
    """Every run of `engine` on the object, the warm-up included."""
    warmup = record.get(f'{engine}_warmup')
    return ([warmup] if warmup else []) + record.get(engine, [])


def engine_rows(engine, held, runs):
    """(rows, excluded, equality failures, missing) for one engine."""
    source = [(meta, records) for meta, records in runs if engine in meta['engines']]
    if not source:
        return [], [], [], [f['sha256'] for f in held]
    if len(source) > 1:
        raise SystemExit(f'{engine} is measured in {len(source)} runs; each path is measured once')
    meta, records = source[0]
    rows, excluded, unequal, missing = [], [], [], []
    for f in held:
        r = records.get(f['sha256'])
        if r is None:
            missing.append(f['sha256'])
            continue
        native = r['native']
        if not native['ok']:
            excluded.append({'sha256': f['sha256'], 'why': 'the native build refuses the track'})
            continue
        runs_ = all_runs(r, engine)
        ok = bool(r.get(engine)) and all(t.get('ok') and t.get('sha256') == native['sha256'] for t in runs_)
        if not ok:
            unequal.append(f['sha256'])
            excluded.append({'sha256': f['sha256'], 'why': f'{engine} failed or differs from the native build'})
            continue
        ffmpeg_runs = all_runs(r, 'ffmpeg')
        if not r['ffmpeg'] or not all(t['ok'] for t in ffmpeg_runs):
            excluded.append({'sha256': f['sha256'], 'why': 'FFmpeg failed'})
            continue
        seconds = statistics.median(t['seconds'] for t in r[engine])
        ffmpeg = statistics.median(t['seconds'] for t in r['ffmpeg'])
        s = [t.get('stats') or {} for t in r[engine]]
        received = statistics.median(x.get('bytesReceived', x.get('bytes_received', 0)) for x in s)
        row = {'sha256': f['sha256'], 'size': f['size'], 'stratum': f['stratum'],
               'seconds': seconds, 'ffmpeg_seconds': ffmpeg, 'ratio': seconds / ffmpeg,
               'requests': statistics.median(x.get('requests', 0) for x in s), 'received_share': received / f['size']}
        extent = r.get(f'{engine}_extent')
        if extent and all(t.get('ok') and t.get('sha256') == native['sha256'] for t in extent):
            row['extent_seconds'] = extent[0]['seconds']
            row['extent_received_share'] = extent[0]['stats']['bytesReceived'] / f['size']
            row['extent_requests'] = extent[0]['stats']['requests']
        rows.append(row)
    return rows, excluded, unequal, missing, meta


def report(engine, held, runs):
    measured = engine_rows(engine, held, runs)
    if len(measured) == 4:
        return {'measured': False, 'missing': len(measured[3])}
    rows, excluded, unequal, missing, meta = measured
    ratios = [r['ratio'] for r in rows]
    classes = {name: stats([r['ratio'] for r in rows if test(r)]) for name, test in CLASSES.items()}
    proper = meta.get('held_out') is True and meta.get('dirty') is False
    bar = (proper and not missing and not unequal and bool(ratios) and statistics.median(ratios) <= 1.0
           and all(c['n'] > 0 and c['median'] <= 1.0 for c in classes.values()))
    faster = {name: c['n'] > 1 and c['ci95'][1] < 1.0 for name, c in classes.items()}
    out = {
        'commit': meta.get('commit'),
        'held_out_run': meta.get('held_out'),
        'dirty': meta.get('dirty'),
        'compared': len(rows),
        'excluded': excluded,
        'missing': missing,
        'equality_failures': unequal,
        'bar_every_output_equal': proper and not unequal and not missing,
        'all': stats(ratios),
        'classes': classes,
        'bar_median_at_most_1.00': bar,
        'faster_upper_bound_below_1.00': {'all': len(ratios) > 1 and bootstrap(ratios)[1] < 1.0, **faster},
        'received_share': stats([r['received_share'] for r in rows]),
        'requests': stats([r['requests'] for r in rows]),
        'rows': rows,
    }
    with_extent = [r for r in rows if 'extent_seconds' in r]
    if with_extent:
        out['extent_received_share'] = stats([r['extent_received_share'] for r in with_extent])
        out['extent_requests'] = stats([r['extent_requests'] for r in with_extent])
        out['over_extent'] = stats([r['seconds'] / r['extent_seconds'] for r in with_extent])
    return out


def main():
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    try:
        import extra_summaries
    except ImportError:
        extra_summaries = None
    parser = argparse.ArgumentParser()
    parser.add_argument('sets')
    parser.add_argument('runs', nargs='+')
    if extra_summaries:
        extra_summaries.options(parser)
    parser.add_argument('--out')
    args = parser.parse_args()
    held = json.loads(Path(args.sets).read_text())['held_out']
    if len(held) != 60:
        print(f'note: {len(held)} objects listed, not the 60 of the held-out set', file=sys.stderr)
    runs = [load(run) for run in args.runs]
    # Every engine a run measured, in the order the runs name them; the CLI
    # always, measured or not.
    engines = list(dict.fromkeys(e for meta, _ in runs for e in meta['engines']))
    engines = sorted(set(engines) | {'cli'}, key=lambda e: (e == 'cli', e))
    result = {'held_out': len(held), **{e: report(e, held, runs) for e in engines}}
    if extra_summaries:
        result.update(extra_summaries.summaries(args, held, runs, stats, LARGE))
    text = json.dumps(result, indent=1)
    if args.out:
        Path(args.out).write_text(text + '\n')
    print(text)


if __name__ == '__main__':
    main()
