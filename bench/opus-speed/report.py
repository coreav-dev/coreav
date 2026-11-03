#!/usr/bin/env python3
"""Recompute a registered Opus judgment from complete checked shards."""
import collections
import json
import pathlib
import random
import statistics

import speed


def summarize(plan, binding, expected, indexed, reports, plan_sha256, roster_sha256):
    sources, shards = speed.roster(expected, plan)
    forecasts = speed.allowance.admit(indexed, expected, plan)
    errors, all_rows, accepted, seen = [], [], [], set()
    host = {'cpu': plan['host']['cpu'], 'smt': False, 'outputs_tmpfs': True,
            'memory_as_bytes': binding['limits']['memory_bytes'],
            'tmpfs_budget_bytes': binding['limits']['tmpfs_bytes'],
            'tmpfs_capacity_bytes': binding['limits']['tmpfs_bytes'],
            'machine': plan['host']['machine'], 'provisioning': 'STANDARD', 'affinity_cpu': plan['host']['affinity_cpu']}
    for report in reports:
        if not isinstance(report, dict):
            errors.append({'reason': 'invalid_shard'})
            continue
        shard = report.get('shard')
        if not isinstance(shard, str) or shard not in shards or shard in seen:
            errors.append({'reason': 'unexpected_or_duplicate_shard'})
            continue
        seen.add(shard)
        rows = report.get('results')
        if not isinstance(rows, list):
            rows = []
        all_rows.extend(rows)
        wanted = {(source, cls) for source in shards[shard] for cls in speed.CLASSES}
        found = []
        invalid = []
        for row in rows:
            if (not isinstance(row, dict) or not speed.valid_sha(row.get('source_sha256'))
                    or row.get('class') not in speed.CLASSES):
                invalid.append(row)
                continue
            found.append(speed.key(row))
            registered = indexed.get(speed.key(row))
            matches = (registered is not None and
                ((registered['status'] in speed.DERIVATION_FAILURES and row.get('status') == 'derivation_failed')
                 or (registered['status'] == 'ok' and row.get('status') != 'derivation_failed'
                     and all(row.get(name) == registered[name]
                             for name in ('sha256', 'channels', 'kbps', 'frame_ms', 'packets', 'raw_frames', 'file_bytes')))))
            if speed.key(row) not in wanted or not matches or not speed.record_valid(row, plan):
                invalid.append(row)
        for source, cls in sorted(wanted - set(found)):
            errors.append({'reason': 'omitted_input', 'shard': shard, 'source_sha256': source, 'class': cls})
        for row in invalid:
            identity = ({'source_sha256': row.get('source_sha256'), 'class': row.get('class')}
                        if isinstance(row, dict) and speed.valid_sha(row.get('source_sha256'))
                        and row.get('class') in speed.CLASSES else {})
            errors.append({'reason': 'failed_or_incomplete_input', 'shard': shard, **identity})
        fields = {'schema', 'round', 'plan_sha256', 'provenance_commit', 'binding_sha256',
                  'roster_sha256', 'shard', 'host', 'results', 'errors', 'complete', 'allowance', 'run_seconds'}
        header = (set(report) == fields and report.get('schema') == 'opus-speed/1'
                  and report.get('round') == plan['round'] and report.get('plan_sha256') == plan_sha256
                  and report.get('provenance_commit') == plan['provenance_commit']
                  and report.get('binding_sha256') == speed.capacity.canonical(binding)
                  and report.get('roster_sha256') == roster_sha256 and report.get('host') == host
                  and report.get('allowance') == forecasts[shard]
                  and speed.allowance.number(report.get('run_seconds'))
                  and report['run_seconds'] <= plan['shard_seconds']
                  and report.get('errors') == [] and report.get('complete') is True)
        coverage = len(found) == len(wanted) and set(found) == wanted
        if not header or not coverage or invalid:
            errors.append({'reason': 'unusable_shard', 'shard': shard})
        else:
            accepted.extend(rows)
    for shard in sorted(set(shards) - seen):
        errors.append({'reason': 'missing_shard', 'shard': shard})
    present = {speed.key(row) for row in all_rows if isinstance(row, dict)
               and speed.valid_sha(row.get('source_sha256')) and row.get('class') in speed.CLASSES}
    for source in sources:
        for cls in speed.CLASSES:
            if (source, cls) not in present:
                all_rows.append(speed.blank(indexed[source, cls]))
    timed = sorted((row for row in accepted if row['status'] == 'timed'), key=speed.key)
    modes = collections.Counter(row['mode'] for row in timed)
    celt_classes = collections.Counter(row['class'] for row in timed if row['mode'] == 'celt')
    minimums = {'real_sources': len(sources), 'celt': modes['celt'], 'celt_mp4': celt_classes['mp4'],
                'celt_matroska': celt_classes['matroska'], 'silk': modes['silk'], 'hybrid': modes['hybrid']}
    floors = {'real_sources': plan['draw']['minimum_sources'], 'celt': plan['minimums']['celt'],
              'celt_mp4': plan['minimums']['celt_per_class'], 'celt_matroska': plan['minimums']['celt_per_class'],
              'silk': plan['minimums']['silk'], 'hybrid': plan['minimums']['hybrid']}
    for group, floor in floors.items():
        if minimums[group] < floor:
            errors.append({'reason': 'minimum_not_met', 'group': group, 'actual': minimums[group], 'minimum': floor})
    untimed = [speed.public_untimed(row) for row in all_rows
               if isinstance(row, dict) and row.get('status') != 'timed']
    untimed = [row for row in untimed if row is not None]
    summary = {'schema': 'opus-speed-summary/1', 'round': plan['round'], 'plan_sha256': plan_sha256,
        'roster_sha256': roster_sha256, 'binding_sha256': speed.capacity.canonical(binding),
        'provenance_commit': plan['provenance_commit'], 'bootstrap': plan['bootstrap'],
        'passed': False, 'minimums': minimums, 'errors': errors, 'timed': 0, 'stats': [],
        'gates': {name: dict(collections.Counter(speed.gate_status(row, name) for row in all_rows))
                  for name in speed.GATES},
        'untimed': untimed}
    if errors:
        return summary
    rng = random.Random(plan['bootstrap']['seed'])

    def pair(label, group, subset, numerator, denominator, threshold=None, strict=False):
        ratios = [row['ns'][numerator] / row['ns'][denominator] for row in subset]
        if not ratios:
            if threshold is not None:
                errors.append({'reason': 'empty_gated_group', 'pair': label, 'group': group})
            return
        median = statistics.median(ratios)
        boot = sorted(statistics.median(rng.choices(ratios, k=len(ratios)))
                      for _ in range(plan['bootstrap']['resamples']))
        lo, hi = boot[249], boot[9749]
        summary['stats'].append({'pair': label, 'group': group, 'inputs': len(ratios), 'median': median,
            'ci95': [lo, hi], 'share_faster': sum(value < 1.0 for value in ratios) / len(ratios),
            'max': max(ratios), 'faster_claim_supported': hi < 1.0, 'threshold': threshold, 'strict': strict})
        if threshold is not None and not (median < threshold if strict else median <= threshold):
            errors.append({'reason': 'bar_missed', 'pair': label, 'group': group,
                           'median': median, 'threshold': threshold})

    celt = [row for row in timed if row['mode'] == 'celt']
    for cls in ('all', *speed.CLASSES):
        pair('decoder_vs_lav', 'celt/' + cls, [row for row in celt if cls == 'all' or row['class'] == cls],
             'head_dec', 'lav_dec', plan['bars']['celt_decoder_lav_below'], True)
    for mode in ('silk', 'hybrid'):
        pair('decoder_vs_base', mode, [row for row in timed if row['mode'] == mode],
             'head_dec', 'base_dec', plan['bars'][mode + '_decoder_base_at_most'])
    for cls in ('all', *speed.CLASSES):
        pair('library_vs_lav', cls, [row for row in timed if cls == 'all' or row['class'] == cls],
             'head_lib', 'lav_lib', plan['bars']['library_lav_at_most'])
    for mode in ('all', 'silk', 'hybrid'):
        pair('decoder_vs_lav', mode, [row for row in timed if mode == 'all' or row['mode'] == mode],
             'head_dec', 'lav_dec')
    for field in ('toc_frame_ms', 'kbps'):
        for value in sorted({row[field] for row in celt}, key=float):
            pair('decoder_vs_lav', 'celt/' + field + '/' + str(value),
                 [row for row in celt if row[field] == value], 'head_dec', 'lav_dec')
    for cls in ('all', *speed.CLASSES):
        subset = [row for row in timed if cls == 'all' or row['class'] == cls]
        pair('library_vs_base', cls, subset, 'head_lib', 'base_lib')
        pair('command_vs_ffmpeg', cls, subset, 'head_cli', 'ffmpeg_cli')
    summary['timed'] = len(timed)
    summary['passed'] = not errors
    return summary


def main(argv=None):
    parser = speed.capacity.Parser(description=__doc__)
    parser.add_argument('--plan', type=pathlib.Path, required=True)
    parser.add_argument('--roster', type=pathlib.Path, required=True)
    parser.add_argument('--inputs', type=pathlib.Path, required=True)
    parser.add_argument('--exclusions', type=pathlib.Path, required=True)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    parser.add_argument('reports', nargs='+', type=pathlib.Path)
    args = None
    try:
        args = parser.parse_args(argv)
        plan, binding = speed.load_plan(args.plan)
        expected = speed.capacity.load(args.roster)
        if (speed.capacity.digest(args.exclusions) != plan['exclusions_sha256']
                or speed.capacity.digest(args.inputs) != expected['inputs_sha256']):
            raise speed.Failure('changed_registered_inputs')
        indexed = speed.input_records(speed.capacity.load(args.inputs), expected,
                                      speed.capacity.load(args.exclusions), plan)
        result = summarize(plan, binding, expected, indexed,
            [speed.capacity.load(path) for path in args.reports],
            speed.capacity.digest(args.plan), speed.capacity.digest(args.roster))
    except SystemExit as error:
        return 0 if error.code == 0 else 1
    except BaseException as error:
        result = {'schema': 'opus-speed-summary/1', 'passed': False, 'timed': 0, 'stats': [],
                  'errors': [{'reason': 'invalid_report_input', 'error': speed.receipts.exception_receipt(error)}]}
    if args is not None:
        speed.receipts.atomic_write(args.out, result)
    print(json.dumps({'passed': result['passed'], 'errors': len(result['errors'])}))
    return 0 if result['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
