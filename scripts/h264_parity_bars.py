#!/usr/bin/env python3
"""The bars of an A/B round, from the reports of scripts/h264_parity.py: the one analyzer every round uses.

    python3 -I scripts/h264_parity_bars.py BARS.json DIR [--summary PATH]

DIR holds the reports of the round's calls, `<call>.<class>.json` (`g.sd.json`, `t8.4k.json`, ...), each a call of
scripts/h264_parity.py that timed the decoders `new` and `old` of BARS.json in the same rounds. BARS.json is all a
round supplies (docs/design/h264.md, "A round's bars: one analyzer, complete reports"):

    {"new": "k", "old": "base", "seed": 20261024,
     "calls": {"g":  {"label": "1 thread pinned, 32 training", "streams": "docs/measurements/h264-training/es.json"},
               "h8": {"label": "8 threads, 64 held-out", "streams": "docs/measurements/h264-n2-heldout/es.json"}},
     "subsets": {"cavlc": ["5185721dda11", "6f7799768413"]},
     "bars": [{"name": "X-1", "checks": [{"call": "g", "pairing": "discard", "groups": ["overall"],
                                          "median_max": 0.995, "ci_high_below": 1.0}]},
              {"name": "X-2", "checks": [{"call": "g", "pairing": "write", "groups": ["classes"], "median_max": 1.005}]}]}

- `calls`: each call's stream set is a manifest, a JSON object keyed by stream name whose values carry the stream's
  `class` (the `es.json` of a set), path relative to the repository root; `classes` (optional) narrows it.
- `subsets` (optional): named groups of the streams whose name starts with one of the prefixes; a call whose set has
  none of them has no such group, and a check that asks for it there is an error.
- `bars`: a bar passes when each of its checks does. A check reads one call and one pairing (`discard` or `write`)
  and holds for each of its `groups` (`overall`, the default; a class; `classes` for each class of the call; or a
  subset): the median per-stream ratio new/old is at most `median_max`, and the upper end of its 95% bootstrap
  interval is below `ci_high_below`, of those given. A key the script does not know, an empty `bars` or `groups`
  and two bars of one name are errors, not ignored: each would let a check pass without being evaluated.

Each call's reports must be a complete measurement of its stream set, or no bar is evaluated: per class, the
report exists, lists no `failed` stream, was asked for exactly the class's streams (`requested`, where the report
has it), and has exactly one row with the times and ratios to FFmpeg of both decoders for each of them, every one a
finite number above zero. A refusal names every missing, failed and unexpected stream and every bad value, and exits
with status 2. A call none of whose reports exists is pending, and so is
each bar that reads it.

Per call, pairing and group: the median per-stream ratio with a 95% bootstrap interval over streams (2,000
resamples, random.Random(seed)) and how many streams are faster; each build's median ratio to FFmpeg; the
harness's host-fault records. `--summary PATH` writes them as JSON, with every stream's ratio. Exit status 0 when
every bar passes, 1 when one fails or is pending, 2 for a refusal or a bars file that cannot be used.
"""
import argparse
import json
import math
import os
import random
import statistics
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CLASSES = ('sd', '720p', '1080p', '4k')
PAIRINGS = ('discard', 'write')
RESAMPLES = 2_000
SEED = 20_261_024
SPEC_KEYS = {'new', 'old', 'seed', 'calls', 'subsets', 'bars'}
CALL_KEYS = {'label', 'streams', 'classes'}
BAR_KEYS = {'name', 'checks'}
CHECK_KEYS = {'call', 'pairing', 'groups', 'median_max', 'ci_high_below'}
REFUSED = 2


class Refused(Exception):
    """The bars file or the reports cannot be analyzed; the message says why."""


def unknown(where, keys, allowed):
    extra = sorted(set(keys) - allowed)
    if extra:
        raise Refused(f'{where}: unknown keys {extra} (allowed: {sorted(allowed)})')


def manifest(path, classes):
    """{stream name: class} of a set's manifest, narrowed to `classes` when given."""
    try:
        with open(os.path.join(ROOT, path)) as f:
            streams = {name: entry['class'] for name, entry in json.load(f).items()}
    except (OSError, ValueError, AttributeError, KeyError, TypeError) as error:
        raise Refused(f'{path}: not a stream manifest (an object of streams with their `class`): {error}') from None
    if classes is not None:
        streams = {name: cls for name, cls in streams.items() if cls in classes}
    if not streams:
        raise Refused(f'{path}: no streams' + ('' if classes is None else f' of the classes {classes}'))
    return streams


def load_spec(path):
    """BARS.json, checked: a misspelt key would otherwise leave a check that holds whatever is measured."""
    try:
        with open(path) as f:
            spec = json.load(f)
    except (OSError, ValueError) as error:
        raise Refused(f'{path}: {error}') from None
    if not isinstance(spec, dict):
        raise Refused(f'{path}: not a JSON object')
    unknown(path, spec, SPEC_KEYS)
    for key in ('new', 'old', 'calls', 'bars'):
        if key not in spec:
            raise Refused(f'{path}: no `{key}`')
    if not isinstance(spec['bars'], list) or not spec['bars']:
        raise Refused(f'{path}: `bars` must list at least one bar; a round with no bar evaluates nothing')
    calls = {}
    for name, call in spec['calls'].items():
        unknown(f'call {name}', call, CALL_KEYS)
        if 'streams' not in call:
            raise Refused(f'call {name}: no `streams` manifest')
        calls[name] = {'label': call.get('label', name), 'streams': manifest(call['streams'], call.get('classes'))}
    subsets = spec.get('subsets', {})
    for name in subsets:
        if name in CLASSES or name in ('overall', 'classes'):
            raise Refused(f'subset {name}: the name of a built-in group')
    names = [bar.get('name') for bar in spec['bars']]
    twice = sorted({str(n) for n in names if names.count(n) > 1})
    if twice:
        raise Refused(f'bars named more than once: {", ".join(twice)} (a later verdict would hide an earlier one)')
    for bar in spec['bars']:
        unknown(f"bar {bar.get('name')}", bar, BAR_KEYS)
        if not bar.get('name') or not bar.get('checks'):
            raise Refused(f'a bar needs a `name` and at least one check: {bar}')
        where = f"bar {bar['name']}"
        for check in bar['checks']:
            unknown(where, check, CHECK_KEYS)
            if check.get('call') not in calls:
                raise Refused(f"{where}: call {check.get('call')!r} is not one of {sorted(calls)}")
            if check.get('pairing') not in PAIRINGS:
                raise Refused(f"{where}: pairing {check.get('pairing')!r} is not one of {list(PAIRINGS)}")
            if 'median_max' not in check and 'ci_high_below' not in check:
                raise Refused(f'{where}: a check needs `median_max`, `ci_high_below` or both')
            streams = calls[check['call']]['streams']
            groups = check.get('groups', ['overall'])
            if not groups:
                raise Refused(f'{where}: `groups` is empty, so the check would compare nothing; '
                              'leave it out for overall')
            for group in groups:
                if group not in ('overall', 'classes') and group not in streams.values() and group not in subsets:
                    raise Refused(f'{where}: group {group!r} is not overall, classes, a class of call '
                                  f"{check['call']} or a subset")
                if group in subsets and not any(s.startswith(tuple(subsets[group])) for s in streams):
                    raise Refused(f"{where}: subset {group} has no stream in call {check['call']}'s set")
    return {'new': spec['new'], 'old': spec['old'], 'seed': spec.get('seed', SEED), 'calls': calls,
            'subsets': subsets, 'bars': spec['bars']}


def class_order(classes):
    present = set(classes)
    return [c for c in CLASSES if c in present] + sorted(present - set(CLASSES))


def load_call(d, name, streams, new, old):
    """The rows of one call in class order and its host-fault records, or None when none of its reports exists.
    Raises Refused, naming every stream that is missing, failed or unexpected, when the reports are not a
    complete measurement of `streams`."""
    paths = {cls: os.path.join(d, f'{name}.{cls}.json') for cls in class_order(streams.values())}
    if not any(os.path.exists(p) for p in paths.values()):
        return None
    needed = [f'{b}_{p}' for b in (new, old) for p in PAIRINGS]
    ratios = [f'{b}:{p}' for b in (new, old) for p in PAIRINGS]
    problems = []
    rows = []
    records = {}
    for cls, path in paths.items():
        want = sorted(s for s, c in streams.items() if c == cls)
        tag = f'{name}.{cls}'
        if not os.path.exists(path):
            problems.append(f'{tag}: no report; its {len(want)} streams are missing: {", ".join(want)}')
            continue
        try:
            with open(path) as f:
                report = json.load(f)
            got = list(report['streams'])
            decoders = dict(report['decoders'])
        except (OSError, ValueError, KeyError, TypeError) as error:
            problems.append(f'{tag}: not a report of scripts/h264_parity.py: {error!r}')
            continue
        for build in (new, old):
            if build not in decoders:
                problems.append(f'{tag}: no decoder {build!r} (the report has {sorted(decoders)})')
        for failure in report.get('failed') or []:
            problems.append(f"{tag}: the harness failed on {failure.get('stream')} ({failure.get('reason')})")
        if 'requested' in report:
            asked = report['requested']
            missing = sorted(set(want) - set(asked))
            extra = sorted(set(asked) - set(want))
            if missing or extra or len(asked) != len(set(asked)):
                problems.append(f'{tag}: the call was not asked for exactly the set'
                                + (f'; not asked for: {", ".join(missing)}' if missing else '')
                                + (f'; asked for streams outside it: {", ".join(extra)}' if extra else '')
                                + ('; asked twice for a stream' if len(asked) != len(set(asked)) else ''))
        names = [str(row.get('stream')) for row in got]
        missing = sorted(set(want) - set(names))
        extra = sorted(set(names) - set(want))
        twice = sorted({n for n in names if names.count(n) > 1})
        if missing:
            problems.append(f'{tag}: {len(names)} rows for {len(want)} streams; no row for: {", ".join(missing)}')
        if extra:
            problems.append(f'{tag}: rows for streams outside the set: {", ".join(extra)}')
        if twice:
            problems.append(f'{tag}: more than one row for: {", ".join(twice)}')
        for row in got:
            seconds, ratio = row.get('seconds') or {}, row.get('ratio') or {}
            lacking = [k for k in needed if k not in seconds] + [k for k in ratios if k not in ratio]
            if lacking:
                problems.append(f"{tag}: the row of {row.get('stream')} has no {', '.join(lacking)}")
            bad = [f'seconds.{k} = {seconds[k]!r}' for k in needed if k in seconds and not positive(seconds[k])]
            bad += [f'ratio.{k} = {ratio[k]!r}' for k in ratios if k in ratio and not positive(ratio[k])]
            if bad:
                problems.append(f"{tag}: the row of {row.get('stream')} has values that are not finite positive "
                                f"numbers: {', '.join(bad)}")
        method = report.get('method') or {}
        records[tag] = {'unresolved': [r.get('stream') for r in got if r.get('unresolved')],
                        'retried': method.get('retried'), 'gate_drift_streams': method.get('gate_drift_streams'),
                        'null_max': max((abs(v) for r in got for v in (r.get('null') or {}).values()), default=None)}
        rows += [dict(row, **{'class': cls}) for row in got]
    if problems:
        raise Refused('\n'.join(problems))
    return rows, records


def positive(value):
    """A time or a ratio: a finite number above zero (JSON lets NaN and Infinity through)."""
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value > 0


def interval(values, seed):
    rng = random.Random(seed)
    medians = sorted(statistics.median(rng.choices(values, k=len(values))) for _ in range(RESAMPLES))
    return medians[int(0.025 * RESAMPLES)], medians[int(0.975 * RESAMPLES) - 1]


def analyze(spec, d):
    """The summary of every call whose reports exist, and the verdict of every bar: True, False, or None
    (pending: a call it reads has no report)."""
    new, old = spec['new'], spec['old']
    calls = {}
    for name, call in spec['calls'].items():
        loaded = load_call(d, name, call['streams'], new, old)
        if loaded is None:
            continue
        rows, records = loaded
        groups = {'overall': rows}
        for cls in class_order(call['streams'].values()):
            groups[cls] = [r for r in rows if r['class'] == cls]
        for subset, prefixes in spec['subsets'].items():
            sel = [r for r in rows if r['stream'].startswith(tuple(prefixes))]
            if sel:
                groups[subset] = sel
        result = {'label': call['label'], 'streams': len(rows), 'records': records}
        for p in PAIRINGS:
            result[p] = {}
            for group, sel in groups.items():
                v = [r['seconds'][f'{new}_{p}'] / r['seconds'][f'{old}_{p}'] for r in sel]
                if not v:
                    raise Refused(f'call {name}: group {group} has no stream')
                low, high = interval(v, spec['seed'])
                result[p][group] = {'n': len(v), 'median': statistics.median(v), 'ci95': [low, high],
                                    'faster': sum(t < 1 for t in v)}
            result[p]['per_stream'] = {r['stream']: r['seconds'][f'{new}_{p}'] / r['seconds'][f'{old}_{p}']
                                       for r in rows}
        result['vs_ffmpeg'] = {
            build: {p: {group: {'n': len(sel), 'median': statistics.median([r['ratio'][f'{build}:{p}'] for r in sel]),
                                'faster': sum(r['ratio'][f'{build}:{p}'] < 1 for r in sel)}
                        for group, sel in groups.items()}
                    for p in PAIRINGS}
            for build in (new, old)}
        calls[name] = result
    bars = {}
    for bar in spec['bars']:
        verdict = True
        for check in bar['checks']:
            call = calls.get(check['call'])
            if call is None:
                verdict = None
                break
            groups = []
            for group in check.get('groups', ['overall']):
                groups += class_order(spec['calls'][check['call']]['streams'].values()) if group == 'classes' else [group]
            for group in groups:
                cell = call[check['pairing']][group]
                if 'median_max' in check and not cell['median'] <= check['median_max']:
                    verdict = False
                if 'ci_high_below' in check and not cell['ci95'][1] < check['ci_high_below']:
                    verdict = False
        bars[bar['name']] = verdict
    return {'new': new, 'old': old, 'calls': calls, 'bars': bars}


def show(summary):
    new, old = summary['new'], summary['old']
    for name, call in summary['calls'].items():
        print(f"== {name}: {call['label']}, {call['streams']} streams")
        for p in PAIRINGS:
            for group, v in call[p].items():
                if group == 'per_stream':
                    continue
                print(f"  {new}/{old} {p:7} {group:8} n={v['n']:2} median {v['median']:.4f} "
                      f"[{v['ci95'][0]:.4f},{v['ci95'][1]:.4f}] faster {v['faster']}/{v['n']}")
        for build, table in call['vs_ffmpeg'].items():
            for p, cells in table.items():
                print(f'  {build} vs FFmpeg {p:7} ' + ' '.join(
                    f"{group} {v['median']:.3f} ({v['faster']}/{v['n']} faster)" for group, v in cells.items()))
        for tag, r in call['records'].items():
            print(f"  {tag}: unresolved {len(r['unresolved'])} {r['unresolved']}, retried {r['retried']}, "
                  f"gate drift {r['gate_drift_streams']}, largest null deviation {r['null_max']}")
    words = {True: 'pass', False: 'FAIL', None: 'pending'}
    print('bars:', ', '.join(f'{name} {words[v]}' for name, v in summary['bars'].items()))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('bars', help="the round's bars (JSON)")
    ap.add_argument('dir', help='the directory of the reports, <call>.<class>.json')
    ap.add_argument('--summary', metavar='PATH', help='write the summary as JSON')
    args = ap.parse_args()
    try:
        summary = analyze(load_spec(args.bars), args.dir)
    except Refused as error:
        print(f'refused, no bar is evaluated:\n{error}', file=sys.stderr)
        return REFUSED
    show(summary)
    if args.summary:
        with open(args.summary, 'w') as f:
            json.dump(summary, f, indent=1)
    return 0 if all(v is True for v in summary['bars'].values()) else 1


if __name__ == '__main__':
    sys.exit(main())
