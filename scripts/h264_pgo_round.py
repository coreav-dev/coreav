#!/usr/bin/env python3
"""Execute and audit the registered PGO round; see docs/design/h264-pgo-release.md."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import re
import selectors
import signal
import statistics
import subprocess
import sys
import time
import urllib.request
import urllib.parse

sys.path.insert(0, str(Path(__file__).resolve().parent))
import h264_parity_bars as bars

ROOT = Path(__file__).resolve().parents[1]
PLAN = ROOT / 'scripts/h264_pgo_registration.json'
ARMS = ('base', 'pgo', 'ffmpeg')
COVERAGE = {'training', 'decoder-rounds', 'encoder-sets', 'other-draws'}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def digest(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + '\n')


def merged(checkout=ROOT):
    subprocess.run(['git', 'fetch', '--quiet', 'origin', 'main'], cwd=checkout, check=True, timeout=120)
    require(not subprocess.check_output(['git', 'status', '--porcelain'], cwd=checkout), 'source must be clean')
    subprocess.run(['git', 'merge-base', '--is-ancestor', 'HEAD', 'origin/main'], cwd=checkout, check=True, timeout=30)
    return subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=checkout, text=True).strip()


def reservation(root, plan):
    config = json.loads((root / 'reservation-checkout.json').read_text())
    checkout = Path(config['checkout']).resolve()
    commit = merged(checkout)
    path = checkout / config['manifest']
    allocation = json.loads(path.read_text())['allocations'][plan['allocation']]
    require(allocation['operation'] == 'h264-decode' and allocation['status'] == 'reserved' and
            allocation['holdout'] is True and allocation['spent'] is False and
            allocation['seed'] == plan['draw_seed'] and allocation['count_per_class'] == plan['count_per_class'],
            'allocation reservation differs from the public plan')
    return {'allocation': plan['allocation'], 'commit': commit, 'manifest_sha256': digest(path),
            'count_per_class': plan['count_per_class']}


def reservation_receipt(root, plan):
    value = json.loads((root / 'reservation-receipt.json').read_text())
    require(value['allocation'] == plan['allocation'] and value['count_per_class'] == plan['count_per_class'] and
            re.fullmatch('[0-9a-f]{40}', value['commit']) is not None and
            re.fullmatch('[0-9a-f]{64}', value['manifest_sha256']) is not None, 'missing merged allocation receipt')


def exclusions(registry):
    require(set(registry) == COVERAGE, 'exclusion registry must cover all four input owners')
    result = {key: set() for key in ('hashes', 'identities', 'groups')}
    for record in registry.values():
        require(set(record) == {'commitment', *result}, 'incomplete exclusion component')
        require(re.fullmatch('[a-f0-9]{64}', record['commitment']) is not None, 'invalid registry commitment')
        for key in result:
            require(isinstance(record[key], list) and bool(record[key]), 'empty exclusion component')
            result[key].update(record[key])
    return result


def source_identity(url):
    value = urllib.parse.urlsplit(url)
    return urllib.parse.urlunsplit((value.scheme.lower(), value.netloc.lower(), value.path, value.query, ''))


def exclusion_registry(owners):
    require(set(owners) == COVERAGE, 'exclusion owners are incomplete')
    result = {}
    for owner, paths in owners.items():
        found = {key: set() for key in ('hashes', 'identities', 'groups')}
        inventory = []
        for name in paths:
            root = Path(name)
            require(root.exists(), 'exclusion root missing')
            files = sorted(root.rglob('*')) if root.is_dir() else [root]
            for file in files:
                if not file.is_file() or '.git' in file.parts:
                    continue
                if file.suffix not in ('.json', '.jsonl', '.md', '.txt', '.csv'):
                    continue
                raw = file.read_bytes()
                inventory.append([str(file), hashlib.sha256(raw).hexdigest()])
                text = raw.decode('utf-8', errors='replace')
                found['hashes'].update(re.findall(r'(?<![0-9a-f])[0-9a-f]{64}(?![0-9a-f])', text))
                found['hashes'].update(re.findall(r'(?<![0-9a-f])[0-9a-f]{12}(?![0-9a-f])', text))
                found['identities'].update(source_identity(u.rstrip('.,;')) for u in re.findall(r'https?://[^\s\"<>\)]+', text))
                records = []
                try:
                    records = [json.loads(line) for line in text.splitlines()] if file.suffix == '.jsonl' else [json.loads(text)]
                except ValueError:
                    pass
                while records:
                    value = records.pop()
                    if isinstance(value, list):
                        records.extend(value)
                    elif isinstance(value, dict):
                        records.extend(value.values())
                        if isinstance(value.get('groups'), list):
                            found['groups'].update(str(g) for g in value['groups'])
                        for key in ('source', 'device_key'):
                            if isinstance(value.get(key), str) and value[key]:
                                found['groups'].add(value[key])
        require(inventory and all(found.values()), 'exclusion owner has no complete inventory')
        commitment = hashlib.sha256(json.dumps(inventory, separators=(',', ':')).encode()).hexdigest()
        result[owner] = {'commitment': commitment, **{key: sorted(v) for key, v in found.items()}}
    return result


def allocate(candidates, registry, plan):
    used = exclusions(registry)
    require(len({r['identity'] for r in candidates}) == len(candidates), 'duplicate candidate identity')
    selected = []
    for row in candidates:
        require(row['groups'] and row['identity'], 'candidate requires identity and source groups')
        if row['identity'] in used['identities'] or set(row['groups']) & used['groups']:
            continue
        if row.get('sha256') in used['hashes'] or row.get('sha256', '')[:12] in used['hashes']:
            continue
        selected.append(row)
    return sorted(selected, key=lambda r: hashlib.sha256(
        (plan['draw_seed'] + '\0' + r['identity']).encode()).hexdigest())


def roster(rows, registry, plan):
    used = exclusions(registry)
    require(len(rows) == plan['count_per_class'] * 4, 'incomplete held-out roster')
    require(len({r['label'] for r in rows}) == len(rows), 'duplicate label')
    require(len({r['source_sha256'] for r in rows}) == len(rows), 'duplicate source bytes')
    require(len({r['sha256'] for r in rows}) == len(rows), 'duplicate extracted bytes')
    require(len({r['identity'] for r in rows}) == len(rows), 'duplicate source identity')
    groups = [g for r in rows for g in r['groups']]
    require(len(groups) == len(set(groups)), 'source group contributes more than one file')
    for cls in bars.CLASSES:
        require(sum(r['class'] == cls for r in rows) == plan['count_per_class'], 'incomplete class')
    for row in rows:
        require(re.fullmatch('h[0-9]{3}', row['label']) is not None, 'use neutral input labels')
        require(row['identity'] not in used['identities'] and not set(row['groups']) & used['groups'], 'used source')
        require(row['source_sha256'] not in used['hashes'] and row['sha256'] not in used['hashes'], 'used input bytes')
        require(row['source_sha256'][:12] not in used['hashes'] and row['sha256'][:12] not in used['hashes'], 'used input prefix')
        require(row['groups'] and row['class'] in bars.CLASSES, 'invalid input metadata')
        require(all(type(row[k]) is int and row[k] > 0 for k in ('width', 'height')), 'invalid geometry')
        cls = next((c for c, w, h in (('sd', 640, 480), ('720p', 1280, 720), ('1080p', 1920, 1088))
                    if row['width'] <= w and row['height'] <= h), '4k')
        require(row['class'] == cls, 'class disagrees with displayed geometry')
        require(all(re.fullmatch('[a-f0-9]{64}', row[k]) is not None for k in ('sha256', 'source_sha256')), 'invalid content digest')
    return {r['label']: r['class'] for r in rows}


def host(instance):
    require(instance['machine_type'].endswith('/c4-standard-16') or instance['machine_type'] == 'c4-standard-16', 'wrong machine')
    require(instance['provisioning'] == 'STANDARD', 'timing requires on-demand')
    features = instance['advanced_machine_features']
    require(features['threadsPerCore'] == 1 and features['turboMode'] == 'ALL_CORE_MAX', 'wrong provider CPU settings')
    require('8581C' in Path('/proc/cpuinfo').read_text(), 'wrong CPU model')
    require(os.sched_getaffinity(0) == set(range(8)), 'require eight dedicated physical cores')
    topology = [Path(f'/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list').read_text().strip() for c in range(8)]
    require(topology == [str(c) for c in range(8)], 'SMT must be off')
    req = urllib.request.Request('http://metadata.google.internal/computeMetadata/v1/instance/scheduling/preemptible',
                                 headers={'Metadata-Flavor': 'Google'})
    with urllib.request.urlopen(req, timeout=5) as response:
        require(response.read().strip().upper() == b'FALSE', 'metadata refuses timed host')
    return {'topology': topology, 'affinity': sorted(os.sched_getaffinity(0)),
            'cpu': '8581C', 'load': list(os.getloadavg()), 'instance': instance}


def execute(argv, maximum=0, timeout=180):
    start = time.perf_counter()
    h = hashlib.sha256()
    size, error = 0, bytearray()
    with subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE if maximum else subprocess.DEVNULL,
                          stderr=subprocess.PIPE, start_new_session=True) as process:
        try:
            with selectors.DefaultSelector() as poll:
                fd = os.pidfd_open(process.pid)
                try:
                    poll.register(fd, selectors.EVENT_READ, 'exit')
                    poll.register(process.stderr, selectors.EVENT_READ, 'stderr')
                    if maximum:
                        poll.register(process.stdout, selectors.EVENT_READ, 'stdout')
                    elapsed = None
                    while poll.get_map():
                        remaining = timeout - (time.perf_counter() - start)
                        require(remaining > 0, 'child deadline')
                        events = poll.select(remaining)
                        require(events, 'child deadline')
                        for key, _ in events:
                            if key.data == 'exit':
                                elapsed = time.perf_counter() - start
                                poll.unregister(fd)
                                continue
                            data = os.read(key.fd, 1 << 20)
                            if not data:
                                poll.unregister(key.fileobj)
                            elif key.data == 'stdout':
                                size += len(data)
                                require(size <= maximum, 'raw output exceeds frame budget')
                                h.update(data)
                            else:
                                error.extend(data)
                                require(len(error) <= 65536, 'diagnostic output exceeds budget')
                    require(process.wait() == 0 and not error, 'decoder exit or unexpected diagnostic')
                    return {'wall': elapsed, 'bytes': size, 'sha256': h.hexdigest()}
                finally:
                    os.close(fd)
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


def command(root, row, arm, threads, raw, plan):
    path = str(root / 'media' / (row['label'] + '.h264'))
    prefix = ['taskset', '-c', '2' if threads == 1 else '0-7', str(root / arm)]
    count = str(plan['frames'][row['class']])
    if arm != 'ffmpeg':
        return prefix + ['--threads', str(threads), '--frames', count] + ([] if raw else ['--discard']) + [path]
    return prefix + ['-nostdin', '-v', 'error', '-threads', str(threads), '-i', path,
                     '-frames:v', count, '-fps_mode', 'passthrough'] + (
        ['-f', 'rawvideo', '-pix_fmt', 'yuv420p', '-'] if raw else ['-f', 'null', '-'])


def schedule(rows, plan):
    rng = random.Random(plan['seed'])
    cells = []
    for threads in (1, 8):
        for n in range(plan['rounds']):
            order = list(rows)
            rng.shuffle(order)
            for row in order:
                arms = list(ARMS)
                rng.shuffle(arms)
                cells.extend({'label': row['label'], 'arm': a, 'threads': threads, 'round': n} for a in arms)
    return cells


def verdict(one, eight, plan):
    return (one['median'] <= plan['one_thread_median_max'] and
            one['ci95'][1] < plan['one_thread_ci_high_below'] and
            all(eight[c]['ci95'][1] <= plan['eight_thread_ci_high_max'] for c in bars.CLASSES))


def audit(root, plan):
    binding = json.loads((root / 'binding.json').read_text())
    require(binding['plan'] == digest(PLAN), 'registration changed')
    for name, expected in binding['files'].items():
        require(digest(root / name) == expected, 'bound file changed')
    for name, expected in binding['tools'].items():
        require(digest(ROOT / name) == expected, 'executor or estimator changed')
    reservation_receipt(root, plan)
    rows = json.loads((root / 'roster.json').read_text())
    classes = roster(rows, json.loads((root / 'exclusions.json').read_text()), plan)
    events = [json.loads(line) for line in (root / 'events.jsonl').read_text().splitlines()]
    require(events and events[-1] == {'kind': 'complete'}, 'incomplete or aborted run')
    require(not any(r['kind'] == 'abort' for r in events), 'aborted run')
    require(all(r['kind'] in {'host', 'qualification', 'calibration-call', 'calibration', 'health', 'warmup', 'timed', 'complete'}
                for r in events), 'unknown event kind')
    hosts = [r for r in events if r['kind'] == 'host']
    require(len(hosts) == 1 and events[0]['kind'] == 'host', 'missing host evidence')
    provider = hosts[0]['instance']
    require(provider['provisioning'] == 'STANDARD' and provider['advanced_machine_features']['threadsPerCore'] == 1
            and provider['advanced_machine_features']['turboMode'] == 'ALL_CORE_MAX', 'invalid provider host')
    require(provider['machine_type'].split('/')[-1] == 'c4-standard-16' and hosts[0]['affinity'] == list(range(8))
            and hosts[0]['topology'] == [str(c) for c in range(8)] and hosts[0]['cpu'] == '8581C', 'wrong host class')
    checks = [r for r in events if r['kind'] == 'qualification']
    wanted = {(r['label'], a, t) for r in rows for a in ARMS for t in (1, 8)}
    require(len(checks) == len(wanted) and {(r['label'], r['arm'], r['threads']) for r in checks} == wanted,
            'incomplete qualification')
    for row in rows:
        cells = [r for r in checks if r['label'] == row['label']]
        require(len({(r['bytes'], r['sha256'], r['frames']) for r in cells}) == 1, 'raw output mismatch')
        require(0 < cells[0]['frames'] <= plan['frames'][row['class']], 'invalid work')
        pixels = row['width'] * row['height'] + 2 * ((row['width'] + 1) // 2) * ((row['height'] + 1) // 2)
        require(cells[0]['bytes'] == cells[0]['frames'] * pixels and
                re.fullmatch('[a-f0-9]{64}', cells[0]['sha256']) is not None, 'raw geometry or digest mismatch')
    timed = [r for r in events if r['kind'] == 'timed']
    expected = schedule(rows, plan)
    require(len(timed) == len(expected), 'incomplete timings')
    for got, want in zip(timed, expected):
        require(all(got[k] == v for k, v in want.items()) and bars.positive(got['wall']), 'bad timing cell')
    first_timing = next(i for i, r in enumerate(events) if r['kind'] == 'timed')
    require(all(i < first_timing for i, r in enumerate(events) if r['kind'] in
                ('qualification', 'warmup', 'calibration', 'calibration-call')), 'preflight occurs after timing')
    calibration = [r for r in events if r['kind'] == 'calibration']
    require(len(calibration) == 4 and {(r['threads'], r['comparison']) for r in calibration} ==
            {(t, c) for t in (1, 8) for c in ('aa', 'null')}, 'missing calibration')
    for row in calibration:
        require(bars.positive(row['ratio']) and plan['calibration_min'] <= row['ratio'] <= plan['calibration_max'],
                'failed calibration')
        calls = [r for r in events if r['kind'] == 'calibration-call' and
                 r['comparison'] == row['comparison'] and r['threads'] == row['threads']]
        expected_calls = {(c, a, n) for c in bars.CLASSES for a in ('base', row['comparison']) for n in range(plan['rounds'])}
        require(len(calls) == len(expected_calls) and {(r['class'], r['arm'], r['round']) for r in calls} == expected_calls,
                'incomplete calibration calls')
        ratios = []
        for cls in bars.CLASSES:
            samples = {a: [r['wall'] for r in calls if r['class'] == cls and r['arm'] == a]
                       for a in ('base', row['comparison'])}
            require(all(bars.positive(v) for values in samples.values() for v in values), 'invalid calibration time')
            ratios.append(statistics.median(samples[row['comparison']]) / statistics.median(samples['base']))
        require(statistics.median(ratios) == row['ratio'], 'calibration summary changed')
    warms = [r for r in events if r['kind'] == 'warmup']
    require(len(warms) == len(wanted) and {(r['label'], r['arm'], r['threads']) for r in warms} == wanted,
            'missing warmup')
    health = [r for r in events if r['kind'] == 'health']
    require(len(health) == 2 and [r['phase'] for r in health] == ['before', 'after'], 'missing health checks')
    for threads in (1, 8):
        for cls in bars.CLASSES:
            before = health[0]['references'][str(threads)][cls]
            after = health[1]['references'][str(threads)][cls]
            require(bars.positive(before) and bars.positive(after) and
                    abs(after / before - 1) <= plan['reference_drift_max'], 'reference drift')
    result = {}
    for threads in (1, 8):
        times = {(r['label'], a): {} for r in rows for a in ARMS}
        for r in timed:
            if r['threads'] == threads:
                times[r['label'], r['arm']][r['round']] = r['wall']
        comparisons = {f'{a}/{b}': bars.paired_round_summary(times, classes, a, b, plan['rounds'],
                        plan['seed'], plan['bootstrap_replicates']) for a, b in
                       (('pgo', 'base'), ('pgo', 'ffmpeg'), ('base', 'ffmpeg'))}
        calls = {f'{s}/{a}': {'median': statistics.median(v.values()), 'min': min(v.values()),
                  'max': max(v.values()), 'range': max(v.values()) - min(v.values())} for (s, a), v in times.items()}
        result[str(threads)] = {'comparisons': comparisons, 'calls': calls}
    result['pass'] = verdict(result['1']['comparisons']['pgo/base']['groups']['overall'],
                             result['8']['comparisons']['pgo/base']['groups'], plan)
    return result


def run(root, plan):
    revision = merged()
    out = root / 'events.jsonl'
    with out.open('x', buffering=1) as log:
        def emit(row):
            log.write(json.dumps(row, allow_nan=False) + '\n')
        try:
            reservation_receipt(root, plan)
            instance = json.loads((root / 'instance.json').read_text())
            emit({'kind': 'host', **host(instance)})
            rows = json.loads((root / 'roster.json').read_text())
            roster(rows, json.loads((root / 'exclusions.json').read_text()), plan)
            subprocess.run([sys.executable, '-I', str(ROOT / 'scripts/oracle_pin.py'), '--verify-bin', str(root)],
                           check=True, timeout=30)
            refs = json.loads((root / 'references.json').read_text())
            require(len(refs) == 4 and {r['class'] for r in refs} == set(bars.CLASSES), 'four training references required')
            from h264_pgo_build import training_roster
            approved = training_roster(ROOT)
            require(all(any(r['sha256'] == a['es_sha256'] and r['class'] == a['class'] for a in approved.values())
                        for r in refs), 'calibration reference is not approved training data')
            build = json.loads((root / 'build.json').read_text())
            require(build['source']['commit'] == revision and build['independent_match'] and
                    build['recipe_sha256'] == digest(ROOT / 'scripts/h264_pgo_build.py'), 'build provenance changed')
            require(build['pgo']['sha256'] == build['independent']['sha256'] == digest(root / 'pgo') and
                    build['base']['sha256'] == digest(root / 'base'), 'build binaries differ')
            names = ['roster.json', 'exclusions.json', 'references.json', 'instance.json', 'build.json',
                     'reservation-receipt.json', 'base', 'pgo', 'ffmpeg', 'ffprobe', 'aa', 'null']
            names += ['media/' + r['label'] + '.h264' for r in rows + refs]
            require(digest(root / 'aa') == digest(root / 'base') == digest(root / 'null'), 'null binaries differ')
            for row in rows + refs:
                require(digest(root / 'media' / (row['label'] + '.h264')) == row['sha256'], 'input changed')
            write(root / 'binding.json', {'plan': digest(PLAN), 'revision': revision,
                  'tools': {str(p.relative_to(ROOT)): digest(p) for p in (Path(__file__), ROOT / 'scripts/h264_parity_bars.py')},
                  'files': {n: digest(root / n) for n in names}})
            for threads in (1, 8):
                for comparison, arm in (('aa', 'aa'), ('null', 'null')):
                    ratios = []
                    for row in refs:
                        samples = {'base': [], arm: []}
                        for n in range(plan['rounds']):
                            for a in (('base', arm) if n % 2 == 0 else (arm, 'base')):
                                value = execute(command(root, row, a, threads, False, plan))
                                samples[a].append(value['wall'])
                                emit({'kind': 'calibration-call', 'class': row['class'], 'comparison': comparison,
                                      'arm': a, 'threads': threads, 'round': n, **value})
                        ratios.append(statistics.median(samples[arm]) / statistics.median(samples['base']))
                    ratio = statistics.median(ratios)
                    emit({'kind': 'calibration', 'threads': threads, 'comparison': comparison, 'ratio': ratio})
                    require(plan['calibration_min'] <= ratio <= plan['calibration_max'], 'failed calibration')
            def health(phase):
                references = {str(t): {r['class']: statistics.median(execute(command(root, r, 'base', t, False, plan))['wall']
                              for _ in range(3)) for r in refs} for t in (1, 8)}
                emit({'kind': 'health', 'phase': phase, 'references': references})
            health('before')
            for row in rows:
                pixels = row['width'] * row['height'] + 2 * ((row['width'] + 1) // 2) * ((row['height'] + 1) // 2)
                outputs = set()
                for threads in (1, 8):
                    for arm in ARMS:
                        value = execute(command(root, row, arm, threads, True, plan), pixels * plan['frames'][row['class']])
                        require(value['bytes'] > 0 and value['bytes'] % pixels == 0, 'invalid raw frame length')
                        outputs.add((value['bytes'], value['sha256']))
                        emit({'kind': 'qualification', 'label': row['label'], 'arm': arm, 'threads': threads,
                              **value, 'frames': value['bytes'] // pixels})
                require(len(outputs) == 1, 'raw output mismatch before timing')
            for row in rows:
                for threads in (1, 8):
                    for arm in ARMS:
                        execute(command(root, row, arm, threads, False, plan))
                        emit({'kind': 'warmup', 'label': row['label'], 'arm': arm, 'threads': threads})
            lookup = {r['label']: r for r in rows}
            for cell in schedule(rows, plan):
                value = execute(command(root, lookup[cell['label']], cell['arm'], cell['threads'], False, plan))
                emit({'kind': 'timed', **cell, **value})
            health('after')
            require(host(instance)['affinity'] == list(range(8)), 'host changed')
            emit({'kind': 'complete'})
        except BaseException:
            emit({'kind': 'abort'})
            raise


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('action', choices=('registry', 'allocate', 'run', 'audit'))
    ap.add_argument('directory', type=Path)
    args = ap.parse_args()
    plan = json.loads(PLAN.read_text())
    root = args.directory.resolve()
    try:
        if args.action == 'registry':
            owners = json.loads((root / 'registry-inputs.json').read_text())
            write(root / 'exclusions.json', exclusion_registry(owners))
            return 0
        if args.action == 'allocate':
            merged()
            write(root / 'reservation-receipt.json', reservation(root, plan))
            candidates = json.loads((root / 'population.json').read_text())
            write(root / 'allocation.json', allocate(candidates, json.loads((root / 'exclusions.json').read_text()), plan))
            return 0
        if args.action == 'run':
            run(root, plan)
        result = audit(root, plan)
        write(root / 'summary.json', result)
        return 0 if result['pass'] else 1
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError, bars.Refused):
        write(root / 'failure.json', {'status': 'incomplete', 'speed_verdict': None})
        print('PGO round incomplete; retain events and inspect private diagnostics', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
