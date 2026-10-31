#!/usr/bin/env python3
"""Explicit round-9 corpus comparison, cutoff inventory and mutation audit.

The production registry is empty until eligible records land. Synthetic
rules are exercised by tests, never loaded from a run's input files.
See docs/design/corpus-key-exclusions.md for the capture and reporting contract.
"""
import argparse
import collections
import copy
import dataclasses
import decimal
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))
import run

SCHEMA = 'coreav.corpus-kd/1'
PLAN = 'docs/measurements/compat-ffprobe/round9-plan.md'
AMENDMENT = '## Corpus key-row exclusions'
CONTAINERS = ('isobmff', 'matroska', 'mpegts')
ROUND_FILES = 1315
POOLED_TAGS = '(tag keys in fewer than 5 files, together)'
BLIND_SPOTS = [
    'Only reference-listed leaves are gated; extra candidate leaves and trailing packets are not.',
    'Compact packet parsing overwrites repeated same-name side-data keys.',
    'The historical raw comparator treats a missing candidate leaf like null.',
]


@dataclasses.dataclass(frozen=True)
class Pair:
    reference: object
    candidate: object
    present: bool


@dataclasses.dataclass(frozen=True)
class Context:
    reference: dict
    candidate: dict
    reference_packet: dict = dataclasses.field(default_factory=dict)
    candidate_packet: dict = dataclasses.field(default_factory=dict)


@dataclasses.dataclass(frozen=True)
class Occurrence:
    path: str
    key: str
    reference: object
    present: bool
    candidate: object
    context: Context
    successful: bool

    @property
    def different(self):
        return self.reference != self.candidate


@dataclasses.dataclass(frozen=True)
class Rule:
    identifier: str
    document: str
    record: dict
    keys: tuple
    expected: object


def record_hash(record):
    return hashlib.sha256(json.dumps(record, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def file_hash(path):
    with pathlib.Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


class Registry:
    def __init__(self, rules, read_evidence):
        self.rules = tuple(rules)
        self.read_evidence = read_evidence
        if len({r.identifier for r in self.rules}) != len(self.rules):
            raise ValueError('duplicate implemented KD identifier')

    def records(self):
        return [{'id': r.identifier, 'path': r.document, 'sha256': record_hash(r.record),
                 'basis': r.record['basis'], 'record': r.record, 'keys': list(r.keys)} for r in self.rules]

    def validate(self, inventory):
        if inventory['records'] != self.records():
            raise ValueError('inventory and implemented frozen records/scopes differ')

    def evidence(self, path):
        if not self.rules:
            return None
        try:
            return self.read_evidence(path)
        except (OSError, ValueError, EOFError):
            return None

    def covers(self, occurrence, facts):
        if not occurrence.different or not occurrence.successful or facts is None:
            return frozenset()
        found = set()
        for rule in self.rules:
            if occurrence.key not in rule.keys:
                continue
            if rule.identifier.startswith('KD-TS-') and not same_pid(occurrence):
                continue
            pair = rule.expected(facts, occurrence)
            if pair is not None and pair.present == occurrence.present \
                    and type(pair.reference) is type(occurrence.reference) \
                    and pair.reference == occurrence.reference \
                    and type(pair.candidate) is type(occurrence.candidate) \
                    and pair.candidate == occurrence.candidate:
                found.add(rule.identifier)
        return frozenset(found)


def production_registry():
    return Registry((), lambda path: None)


def same_pid(occurrence):
    """A TS prerequisite at the existing array/packet position, never a search."""
    context = occurrence.context
    if occurrence.path.startswith('packets['):
        ref_index = context.reference_packet.get('stream_index')
        got_index = context.candidate_packet.get('stream_index')
        if ref_index != got_index:
            return False
        try:
            index = int(ref_index)
        except (TypeError, ValueError):
            return False
        ref_streams, got_streams = context.reference.get('streams', []), context.candidate.get('streams', [])
        if not 0 <= index < min(len(ref_streams), len(got_streams)):
            return False
        reference, candidate = ref_streams[index], got_streams[index]
    else:
        match = re.match(r'((?:programs\[\d+\]\.)?streams\[\d+\])\.', occurrence.path)
        if match is None:
            return False
        reference, candidate = context.reference, context.candidate
        try:
            for part in match[1].split('.'):
                key, index = part.split('[')
                reference, candidate = reference[key][int(index[:-1])], candidate[key][int(index[:-1])]
        except (KeyError, IndexError, TypeError):
            return False
    try:
        a, b = int(str(reference['id']), 0), int(str(candidate['id']), 0)
    except (KeyError, TypeError, ValueError):
        return False
    return 0 <= a < 8192 and a == b


@dataclasses.dataclass
class Capture:
    reference: dict
    candidate: dict
    reference_packets: pathlib.Path
    candidate_packets: pathlib.Path
    section_code: int = 0
    packet_code: int = 0
    strict_keys: set = dataclasses.field(default_factory=set)


@dataclasses.dataclass(frozen=True)
class Attack:
    label: str
    sections: object = None
    packets: object = None


def packet_rows(path, transform=None):
    with open(path, errors='replace') as source:
        for i, line in enumerate(source):
            fields = run.packet_fields(line)
            yield from transform(i, fields) if transform else (fields,)


def occurrences(capture, attack=None):
    candidate = capture.candidate if attack is None or attack.sections is None else attack.sections
    context = Context(capture.reference, candidate)
    for path, key, ref, present, got in run.key_pairs(run.flatten(capture.reference), run.flatten(candidate)):
        yield Occurrence(path, key, ref, present, got, context, capture.section_code == 0)
    transform = attack.packets if attack else None
    got = iter(packet_rows(capture.candidate_packets, transform)) if capture.packet_code == 0 else iter(())
    for i, ref in enumerate(packet_rows(capture.reference_packets)):
        other = next(got, {})
        context = Context(capture.reference, candidate, ref, other)
        for path, key, value, present, actual in run.key_pairs(ref, other, f'packets[{i}].'):
            yield Occurrence(path, key, value, present, actual, context, capture.packet_code == 0)


def compare(capture, media, registry):
    facts = registry.evidence(media)
    keys, differences, validated, by_kd, examples = {}, collections.Counter(), collections.Counter(), {}, {}
    for item in occurrences(capture):
        keys[item.key] = keys.get(item.key, True) and not item.different
        if not item.different:
            continue
        differences[item.key] += 1
        examples.setdefault(item.key, [item.path, item.reference, item.candidate])
        if item.key in capture.strict_keys:
            continue
        ids = registry.covers(item, facts)
        if ids:
            validated[item.key] += 1
            by_kd.setdefault(item.key, set()).update(ids)
    excluded = {key: sorted(by_kd[key]) for key, n in differences.items() if validated[key] == n}
    return {'keys': keys, 'covered_keys': sorted(capture.strict_keys), 'excluded': excluded,
            'differences': dict(differences), 'validated': dict(validated), 'examples': examples}


def alter(value, delta=1):
    if isinstance(value, bool):
        return not value
    if isinstance(value, (int, float)):
        return value + delta
    if isinstance(value, str):
        try:
            return format(decimal.Decimal(value) + delta, 'f')
        except decimal.InvalidOperation:
            return 'changed' if value != 'changed' else 'other'
    return 'changed'


def section_nodes(document):
    if isinstance(document, dict):
        yield document
        for value in document.values():
            yield from section_nodes(value)
    elif isinstance(document, list):
        for value in document:
            yield from section_nodes(value)


def mutations(capture):
    for node_index, node in enumerate(section_nodes(capture.candidate)):
        for key, value in node.items():
            if isinstance(value, (list, dict)):
                continue
            changed = copy.deepcopy(capture.candidate)
            target = list(section_nodes(changed))[node_index]
            target[key] = alter(value)
            identity = 'identity ' if key in ('id', 'index', 'stream_index') else ''
            yield Attack(f'section {node_index}: {identity}{key} changed', sections=copy.deepcopy(changed))
            if key in ('duration_ts', 'start_pts', 'duration', 'start_time'):
                target['claimed_' + key] = target[key]
                yield Attack(f'section {node_index}: {key} coupled with claimed evidence', sections=copy.deepcopy(changed))
        for key, value in node.items():
            if key != 'streams' or not isinstance(value, list):
                continue
            for i in range(len(value)):
                for action in ('drop', 'duplicate'):
                    changed = copy.deepcopy(capture.candidate)
                    streams = list(section_nodes(changed))[node_index][key]
                    if action == 'drop':
                        del streams[i]
                    else:
                        streams.insert(i, copy.deepcopy(streams[i]))
                    yield Attack(f'section {node_index}: {action} stream {i}', sections=changed)
    counts = collections.Counter(row.get('stream_index') for row in packet_rows(capture.candidate_packets))
    seen = collections.Counter()
    for i, row in enumerate(packet_rows(capture.candidate_packets)):
        stream = row.get('stream_index')
        ordinal = seen[stream]
        seen[stream] += 1
        if ordinal not in {0, counts[stream] // 2, counts[stream] - 1}:
            continue
        for key in row:
            def change(n, fields, i=i, key=key):
                return ({**fields, key: alter(fields.get(key))},) if n == i else (fields,)
            yield Attack(f'packet {i}: {key} changed', packets=change)
        for action in ('drop', 'duplicate'):
            def structure(n, fields, i=i, action=action):
                return (() if action == 'drop' else (fields, fields)) if n == i else (fields,)
            yield Attack(f'{action} packet {i}', packets=structure)


def audit(capture, media, registry, attacks=None):
    facts = registry.evidence(media)
    results = []
    if attacks is None:
        attacks = mutations(capture) if registry.rules else ()
    for attack in attacks:
        caught, covered, changed = 0, 0, 0
        for before, after in zip(occurrences(capture), occurrences(capture, attack), strict=True):
            old_ids, new_ids = registry.covers(before, facts), registry.covers(after, facts)
            new_difference = after.different and (not before.different or
                              (before.present, before.candidate) != (after.present, after.candidate))
            if new_difference:
                changed += 1
                if new_ids or after.key in capture.strict_keys:
                    covered += 1
                else:
                    caught += 1
            elif after.different and not old_ids and new_ids:
                covered += 1
        verdict = 'covered' if covered else 'caught' if caught else 'no effect'
        results.append({'label': attack.label, 'verdict': verdict, 'changed': changed,
                        'caught': caught, 'covered': covered})
    return {'passed': all(r['verdict'] != 'covered' for r in results), 'attacks': results,
            'blind_spots': BLIND_SPOTS}


def report(records):
    groups, aliases, excluded = {}, set(), []
    passed = bool(records)
    for row in records:
        alias = row['alias']
        if not re.fullmatch(r'f[0-9]{6}', alias) or alias in aliases:
            raise ValueError('each row needs a unique opaque fNNNNNN alias')
        aliases.add(alias)
        container = row['container']
        if container not in CONTAINERS or type(row['holdout']) is not bool:
            raise ValueError('unknown container or invalid held-out flag')
        if row['status'] not in ('compared', 'strict_covered') or not row.get('audit', {}).get('passed'):
            passed = False
        keys, covered, exclusions = row.get('keys', {}), set(row.get('covered_keys', ())), row.get('excluded', {})
        if not set(exclusions) <= set(keys) or not covered <= set(keys):
            raise ValueError('coverage names a key not in the raw comparison')
        for key, ok in keys.items():
            if type(ok) is not bool or key in exclusions and (ok or key in covered or not exclusions[key]):
                raise ValueError('raw agreement, existing coverage and new exclusions must not overlap')
            outcome = 'direct' if ok else 'covered' if key in covered else 'excluded' if key in exclusions else 'uncovered'
            for group in ('all', container, container + (':holdout' if row['holdout'] else ':development')):
                c = groups.setdefault(group, {}).setdefault(key, dict.fromkeys(
                    ('files', 'direct', 'covered', 'excluded', 'uncovered'), 0))
                c['files'] += 1
                c[outcome] += 1
            if key in exclusions:
                excluded.append({'alias': alias, 'container': container, 'holdout': row['holdout'],
                                 'key': key, 'kds': sorted(set(exclusions[key]))})
    for table in groups.values():
        rare_tags = [c for k, c in table.items() if '.tags.' in k and c['files'] < 5]
        if rare_tags:
            table[POOLED_TAGS] = {name: sum(c[name] for c in rare_tags)
                                  for name in ('files', 'direct', 'covered', 'excluded', 'uncovered')}
        for key, c in table.items():
            c['headline'] = c['files'] >= 5 and '.tags.' not in key and key != POOLED_TAGS
        for key, c in table.items():
            denominator = c['files'] - c['excluded']
            c.update(raw_agree=c['direct'], raw_files=c['files'],
                     gated_agree=c['direct'] + c['covered'], gated_files=denominator,
                     raw_rate=100 * c['direct'] / c['files'],
                     gated_rate=100 * (c['direct'] + c['covered']) / denominator if denominator else None,
                     eligible=c['files'] >= 5 or key == POOLED_TAGS)
            c['passed'] = bool(denominator and 100 * (c['direct'] + c['covered']) >= 98 * denominator)
            if key == POOLED_TAGS:
                c['memberships'], c['raw_memberships'], c['gated_memberships'] = (
                    c.pop('files'), c.pop('raw_files'), c.pop('gated_files'))
    gated = [c for c in groups.get('all', {}).values() if c['eligible']]
    attribution = collections.Counter()
    detailed = collections.Counter()
    for row in excluded:
        for identifier in row['kds']:
            attribution[identifier] += 1
            detailed[identifier, row['key'], row['container'], row['holdout']] += 1
    return {'schema': SCHEMA, 'passed': passed and bool(gated) and all(c['passed'] for c in gated),
            'files': len(records), 'groups': groups, 'excluded_rows': excluded,
            'audit': {'files_audited': sum('audit' in r for r in records),
                      'failed_aliases': [r['alias'] for r in records if not r.get('audit', {}).get('passed')],
                      'verdicts': dict(collections.Counter(a['verdict'] for r in records
                                      for a in r.get('audit', {}).get('attacks', [])))},
            'by_status': dict(collections.Counter(r['status'] for r in records)),
            'exclusion_counts': [{'kd': kd, 'key': key, 'container': container, 'holdout': holdout, 'rows': n}
                                 for (kd, key, container, holdout), n in sorted(detailed.items())],
            'exclusions_by_kd': dict(attribution), 'unique_excluded_rows': len(excluded),
            'attribution_overlaps': sum(len(r['kds']) > 1 for r in excluded), 'blind_spots': BLIND_SPOTS}


def git(root, *args):
    return subprocess.run(['git', '-C', str(root), *args], check=True, capture_output=True).stdout


def inventory(root, cutoff, registry):
    if not re.fullmatch(r'[0-9a-f]{40}', cutoff):
        raise ValueError('plan commit must be a full commit SHA')
    git(root, 'merge-base', '--is-ancestor', cutoff, 'origin/main')
    if AMENDMENT not in git(root, 'show', f'{cutoff}:{PLAN}').decode():
        raise ValueError('cutoff does not contain the corpus exclusion amendment')
    if AMENDMENT in git(root, 'show', f'{cutoff}^:{PLAN}').decode():
        raise ValueError('cutoff is not the main commit introducing the amendment')
    pin = (root / 'scripts/oracle-ffmpeg.env').read_bytes()
    if pin != git(root, 'show', f'{cutoff}:scripts/oracle-ffmpeg.env'):
        raise ValueError('the oracle pin differs from the plan cutoff')
    for rule in registry.rules:
        entries = json.loads(git(root, 'show', f'{cutoff}:{rule.document}'))['entries']
        matches = [e for e in entries if e['id'] == rule.identifier]
        if matches != [rule.record] or not rule.record.get('basis'):
            raise ValueError(f'{rule.identifier}: absent or changed record at the plan cutoff')
    return {'schema': SCHEMA, 'plan_commit': cutoff, 'records': registry.records(),
            'oracle_pin_sha256': hashlib.sha256(pin).hexdigest()}


def capture_file(coreav, media, directory):
    directory.mkdir(parents=True, exist_ok=False)
    ref_code, ref_out, _ = run.run(['ffprobe'] + run.SECTION_ARGS + [str(media)])
    got_code, got_out, _ = run.run([coreav, 'ffprobe'] + run.SECTION_ARGS + [str(media)])
    (directory / 'reference.json').write_bytes(ref_out)
    (directory / 'candidate.json').write_bytes(got_out)
    if ref_code != 0:
        raise ValueError('oracle section listing failed')
    reference = json.loads(ref_out)
    if not isinstance(reference, dict):
        raise ValueError('oracle section listing is not an object')
    try:
        candidate = json.loads(got_out) if got_code == 0 else {}
        if not isinstance(candidate, dict):
            candidate, got_code = {}, -1
    except ValueError:
        candidate, got_code = {}, -1
    ref_path, got_path = directory / 'reference.packets', directory / 'candidate.packets'
    ref_packets = run.run_to_file(['ffprobe'] + run.PACKET_ARGS + [str(media)], ref_path)
    got_packets = run.run_to_file([coreav, 'ffprobe'] + run.PACKET_ARGS + [str(media)], got_path)
    if ref_packets != 0:
        raise ValueError('oracle packet listing failed')
    capture = Capture(reference, candidate, ref_path, got_path, got_code, got_packets)
    if 1 in (got_code, got_packets) and run.strict_rejection(coreav, media):
        capture.strict_keys = {o.key for o in occurrences(capture)
                               if (got_packets if o.path.startswith('packets[') else got_code) == 1}
    (directory / 'status.json').write_text(json.dumps({'section_code': got_code, 'packet_code': got_packets,
                                                     'strict_keys': sorted(capture.strict_keys)}) + '\n')
    return capture


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    freeze = sub.add_parser('freeze')
    freeze.add_argument('--plan-commit', required=True)
    freeze.add_argument('--out', type=pathlib.Path, required=True)
    compare_parser = sub.add_parser('compare')
    compare_parser.add_argument('--inventory', type=pathlib.Path, required=True)
    compare_parser.add_argument('--files', type=pathlib.Path, required=True)
    compare_parser.add_argument('--coreav', required=True)
    compare_parser.add_argument('--oracle-dir', type=pathlib.Path, required=True)
    compare_parser.add_argument('--out', type=pathlib.Path, required=True)
    args = parser.parse_args()
    registry = production_registry()
    if args.command == 'freeze':
        args.out.write_text(json.dumps(inventory(ROOT, args.plan_commit, registry), indent=2) + '\n')
        return 0
    frozen = json.loads(args.inventory.read_text())
    registry.validate(frozen)
    if frozen != inventory(ROOT, frozen['plan_commit'], registry):
        raise ValueError('inventory no longer matches its cutoff or oracle pin')
    sys.path.insert(0, str(ROOT / 'scripts'))
    import oracle_pin
    problems = list(oracle_pin.verify_bin(args.oracle_dir))
    if problems:
        raise ValueError('; '.join(problems))
    os.environ['PATH'] = str(args.oracle_dir.resolve()) + os.pathsep + os.environ['PATH']
    files = json.loads(args.files.read_text())
    if not isinstance(files, list) or not files:
        raise ValueError('files must be the nonempty fixed manifest')
    aliases, media_hashes = set(), set()
    for row in files:
        if not re.fullmatch(r'f[0-9]{6}', row['alias']) or row['alias'] in aliases \
                or row['container'] not in CONTAINERS or type(row['holdout']) is not bool \
                or not re.fullmatch(r'[0-9a-f]{64}', row['sha256']) or row['sha256'] in media_hashes:
            raise ValueError('invalid or duplicate manifest row')
        aliases.add(row['alias'])
        media_hashes.add(row['sha256'])
    args.coreav = str(pathlib.Path(args.coreav).resolve(strict=True))
    identities = {'binary_sha256': file_hash(args.coreav),
                  'oracle_sha256': file_hash(args.oracle_dir / 'ffprobe'),
                  'selection_sha256': file_hash(args.files),
                  'tooling_commit': git(ROOT, 'rev-parse', 'HEAD').decode().strip(),
                  'tool_sha256': file_hash(__file__), 'comparator_sha256': file_hash(HERE / 'run.py')}
    args.out.mkdir(parents=True, exist_ok=False)
    (args.out / 'private').mkdir(mode=0o700)
    (args.out / 'private/files.json').write_text(args.files.read_text())
    (args.out / 'inventory.json').write_text(json.dumps(frozen, indent=2) + '\n')
    (args.out / 'identities.json').write_text(json.dumps(identities, indent=2) + '\n')
    run.ENV = run.release_round.environment(args.out / 'private/fallback.jsonl')
    results = [{'alias': r['alias'], 'container': r['container'], 'holdout': r['holdout'],
                'status': 'unprocessed'} for r in files]
    aborted, active = False, None
    try:
        for i, row in enumerate(files):
            active = i
            media = pathlib.Path(row['path'])
            if file_hash(media) != row['sha256']:
                raise ValueError('manifest media checksum differs')
            cap = capture_file(args.coreav, media, args.out / 'private' / row['alias'])
            result = compare(cap, media, registry)
            audited = audit(cap, media, registry)
            failed = any(code != 0 and not (code == 1 and cap.strict_keys)
                         for code in (cap.section_code, cap.packet_code))
            results[i] = {'alias': row['alias'], 'container': row['container'], 'holdout': row['holdout'],
                            'status': 'coreav_failed' if failed else 'strict_covered' if cap.strict_keys else 'compared',
                            'audit': audited, **result}
            active = None
    except BaseException:
        aborted = True
        if active is not None:
            results[active]['status'] = 'error'
        raise
    finally:
        (args.out / 'private/rows.json').write_text(json.dumps(results, indent=2) + '\n')
        public = report(results)
        processed = sum(r['status'] not in ('unprocessed', 'error') for r in results)
        complete = len(files) == ROUND_FILES and {r['container'] for r in files} == set(CONTAINERS)
        public.update(aborted=aborted, expected_files=len(files), processed_files=processed,
                      registered_population=complete, inventory=frozen, **identities,
                      fallback=run.release_round.fallback_verdict(run.ENV['COREAV_FALLBACK_LOG']))
        public['passed'] = public['passed'] and complete and not aborted and processed == len(files) \
            and not public['fallback']['void']
        (args.out / 'report.json').write_text(json.dumps(public, indent=2) + '\n')
    return 0 if public['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
