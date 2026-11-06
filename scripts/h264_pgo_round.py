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
import tempfile
import time
import urllib.request
import urllib.parse

sys.path.insert(0, str(Path(__file__).resolve().parent))
import h264_parity_bars as bars
import test_oracle

ROOT = Path(__file__).resolve().parents[1]
PLAN = ROOT / 'scripts/h264_pgo_registration.json'
EVALUATION = ROOT / 'scripts/h264_pgo_evaluation.json'
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
    require(not subprocess.check_output(['git', 'status', '--porcelain'], cwd=checkout, timeout=30), 'source must be clean')
    subprocess.run(['git', 'merge-base', '--is-ancestor', 'HEAD', 'origin/main'], cwd=checkout, check=True, timeout=30)
    return subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=checkout, text=True, timeout=30).strip()


def reservation(root, plan, commit=None):
    config = json.loads((root / 'reservation-checkout.json').read_text())
    checkout = Path(config['checkout']).resolve()
    relative = Path(config['manifest'])
    require(not relative.is_absolute() and '..' not in relative.parts and relative.parts,
            'reservation manifest must stay inside its checkout')
    head = merged(checkout)
    current = subprocess.check_output(['git', 'rev-parse', 'origin/main'], cwd=checkout, text=True, timeout=30).strip()
    require(commit is not None or head == current, 'update allocation checkout to fetched main before freezing')
    commit = current if commit is None else commit
    require(re.fullmatch('[0-9a-f]{40}', commit) is not None, 'invalid reservation revision')
    subprocess.run(['git', 'merge-base', '--is-ancestor', commit, 'origin/main'], cwd=checkout, check=True, timeout=30)
    entry = subprocess.check_output(['git', 'ls-tree', commit, '--', relative.as_posix()], cwd=checkout, text=True, timeout=30)
    require(entry.startswith('100644 blob ') and len(entry.splitlines()) == 1, 'reservation manifest must be a tracked regular file')
    raw = subprocess.check_output(['git', 'cat-file', 'blob', f'{commit}:{relative.as_posix()}'], cwd=checkout, timeout=30)
    allocation = json.loads(raw)['allocations'][plan['allocation']]
    require(allocation['operation'] == 'h264-decode' and allocation['status'] == 'reserved' and
            allocation['holdout'] is True and allocation['spent'] is False and
            allocation['seed'] == plan['draw_seed'] and allocation['count_per_class'] == plan['count_per_class'],
            'allocation reservation differs from the public plan')
    checker = Path(allocation['exclusion_checker'])
    require(not checker.is_absolute() and '..' not in checker.parts and checker.suffix == '.py',
            'allocation checker must stay inside its checkout')
    entry = subprocess.check_output(['git', 'ls-tree', commit, '--', checker.as_posix()], cwd=checkout, text=True, timeout=30)
    require(entry.startswith('100644 blob ') and len(entry.splitlines()) == 1, 'allocation checker must be tracked')
    code = subprocess.check_output(['git', 'cat-file', 'blob', f'{commit}:{checker.as_posix()}'], cwd=checkout, timeout=30)
    return {'allocation': plan['allocation'], 'commit': commit, 'manifest_sha256': hashlib.sha256(raw).hexdigest(),
            'population': allocation['population'], 'count_per_class': plan['count_per_class'],
            'checker': {'path': checker.as_posix(), 'sha256': hashlib.sha256(code).hexdigest()}}


def reservation_receipt(root, plan):
    value = json.loads((root / 'reservation-receipt.json').read_text())
    require(set(value) == {'allocation', 'commit', 'manifest_sha256', 'population', 'count_per_class', 'checker'} and
            isinstance(value['population'], str) and value['population'] and
            value['allocation'] == plan['allocation'] and value['count_per_class'] == plan['count_per_class'] and
            re.fullmatch('[0-9a-f]{40}', value['commit']) is not None and
            re.fullmatch('[0-9a-f]{64}', value['manifest_sha256']) is not None, 'missing merged allocation receipt')
    checker = value['checker']
    require(set(checker) == {'path', 'sha256'} and isinstance(checker['path'], str) and
            re.fullmatch('[0-9a-f]{64}', checker['sha256']) is not None, 'missing allocation checker identity')


def reserved_inputs(root, plan, receipt):
    config = json.loads((root / 'reservation-checkout.json').read_text())
    checkout = Path(config['checkout']).resolve()
    code = subprocess.check_output(['git', 'cat-file', 'blob', f"{receipt['commit']}:{receipt['checker']['path']}"], cwd=checkout, timeout=30)
    require(hashlib.sha256(code).hexdigest() == receipt['checker']['sha256'], 'allocation checker changed')
    with tempfile.TemporaryDirectory() as directory:
        checker = Path(directory) / 'check.py'
        checker.write_bytes(code)
        raw = subprocess.check_output([sys.executable, '-I', str(checker), '--repo', str(checkout),
                                       '--commit', receipt['commit'], '--allocation', plan['allocation'],
                                       '--frozen', str(Path(config['frozen']).resolve())], timeout=120)
    return json.loads(raw)


def reserved_evidence(root, plan, receipt):
    value = json.loads((root / 'reserved-inputs.json').read_text())
    require(set(value) == {'schema', 'allocation', 'commit', 'manifest_sha256', 'frozen_sha256',
                           'marked_rows', 'frozen_rows', 'hashes'} and value['schema'] == 1 and
            value['allocation'] == plan['allocation'] and value['commit'] == receipt['commit'] and
            value['manifest_sha256'] == receipt['manifest_sha256'], 'standing exclusion evidence differs')
    require(re.fullmatch('[0-9a-f]{64}', value['frozen_sha256']) is not None and
            all(type(value[k]) is int and value[k] > 0 for k in ('marked_rows', 'frozen_rows')) and
            isinstance(value['hashes'], list) and bool(value['hashes']) and
            all(re.fullmatch('[0-9a-f]{64}', h) is not None for h in value['hashes']) and
            value['hashes'] == sorted(set(value['hashes'])), 'standing exclusion inventory is incomplete')
    require(set(value['hashes']) <= exclusions(json.loads((root / 'exclusions.json').read_text()))['hashes'],
            'registry omits standing spent or reserved inputs')
    return value


def build_evidence(root, revision):
    from h264_pgo_build import training_roster, TOOLCHAIN, LLVM, TARGET
    build = json.loads((root / 'build.json').read_text())
    acquisition = json.loads((ROOT / 'scripts/h264_pgo_acquisition.json').read_text())
    lock = acquisition['artifact_lock_sha256'] if revision == acquisition['artifact_revision'] else digest(ROOT / 'Cargo.lock')
    require(build['schema'] == 1 and build['source']['commit'] == revision and
            build['source']['lock_sha256'] == lock and build['independent_match'] is True and
            build['recipe_sha256'] == digest(ROOT / 'scripts/h264_pgo_build.py'), 'build provenance changed')
    require(build['pgo']['sha256'] == build['independent']['sha256'] == digest(root / 'pgo') == digest(root / 'independent')
            and build['base']['sha256'] == digest(root / 'base') and
            build['profile_sha256'] == digest(root / 'profile.profdata'), 'build artifacts differ')
    require(f'release: {TOOLCHAIN}' in build['tools']['rustc'] and LLVM in build['tools']['profdata'], 'build compiler differs')
    approved = training_roster(ROOT)
    training = build['training']
    require(len(training) == 32 and {(r['input'], r['threads']) for r in training} ==
            {(s, t) for s in approved for t in (1, 8)}, 'incomplete profile training')
    require(all(r['input_sha256'] == approved[r['input']]['es_sha256'] and r['bytes'] > 0 for r in training), 'training bytes differ')
    cli = build['cli']
    features = json.loads(PLAN.read_text())['diagnostics']['cli_features']
    require(cli['features'] == features and cli['independent_match'] is True, 'diagnostic CLI configuration differs')
    require(cli['pgo']['sha256'] == cli['independent']['sha256'] == digest(root / 'cli-pgo') ==
            digest(root / 'cli-independent') and cli['base']['sha256'] == digest(root / 'cli-base'), 'diagnostic CLI artifacts differ')
    command = ['cargo', '+' + TOOLCHAIN, 'build', '--locked', '--profile', 'dist', '-p', 'coreav-cli',
               '--bin', 'coreav', '--features', ','.join(features), '--target', TARGET]
    require(all(cli[a]['command'] == command for a in ('base', 'pgo', 'independent')) and
            cli['base']['flags'] == build['base']['flags'] and
            cli['pgo']['flags'] == cli['independent']['flags'] == build['pgo']['flags'], 'diagnostic CLI build flags differ')
    return build


def evidence_inventory(rows, refs):
    from h264_pgo_diagnostics import FILES
    names = ['roster.json', 'exclusions.json', 'references.json', 'instance.json', 'build.json',
             'reservation-receipt.json', 'reserved-inputs.json', 'allocation-contract.json', 'collection.json', 'base', 'pgo', 'independent',
             'profile.profdata', 'ffmpeg', 'ffprobe', 'aa', 'null']
    names += ['cli-base', 'cli-pgo', 'cli-independent', *FILES]
    names += ['media/' + r['label'] + '.h264' for r in rows + refs]
    require(len(names) == len(set(names)), 'input label overlaps a reference')
    return names


def population_files(root, receipt, plan):
    population = json.loads((root / 'population.json').read_text())
    require(set(population) == {'allocation', 'population', 'snapshots', 'candidates'} and
            population['allocation'] == plan['allocation'] and population['population'] == receipt['population'],
            'population differs from reservation')
    require(isinstance(population['snapshots'], dict) and population['snapshots'], 'population has no frozen metadata snapshots')
    paths = []
    for name, expected in population['snapshots'].items():
        require(re.fullmatch('s[0-9]{4}\\.json', name) is not None, 'snapshot needs a neutral local name')
        path = 'snapshots/' + name
        require(digest(root / path) == expected, 'population snapshot changed')
        paths.append(path)
    return population, paths


def allocation_files(snapshots):
    from h264_pgo_diagnostics import FILES
    return ['population.json', 'allocation.json', 'exclusions.json', 'build.json', 'reservation-receipt.json', 'reserved-inputs.json',
            'base', 'pgo', 'independent', 'profile.profdata', 'cli-base', 'cli-pgo', 'cli-independent', *FILES, *snapshots]


def verify_allocation(root, plan, expected, revision=None):
    from h264_pgo_diagnostics import verify
    require(expected is not None and re.fullmatch('[0-9a-f]{64}', expected) is not None and
            digest(root / 'allocation-contract.json') == expected, 'allocation contract differs from registered digest')
    contract = json.loads((root / 'allocation-contract.json').read_text())
    require(set(contract) == {'schema', 'plan', 'revision', 'files'} and contract['schema'] == 1 and
            contract['plan'] == digest(PLAN), 'allocation contract schema or plan differs')
    require(revision is None or contract['revision'] == revision, 'allocation source changed')
    receipt = json.loads((root / 'reservation-receipt.json').read_text())
    reservation_receipt(root, plan)
    reserved_evidence(root, plan, receipt)
    population, snapshots = population_files(root, receipt, plan)
    require(isinstance(contract['files'], dict) and set(contract['files']) == set(allocation_files(snapshots)),
            'incomplete allocation inventory')
    require(all(digest(root / n) == sha for n, sha in contract['files'].items()), 'frozen allocation artifact changed')
    selected = json.loads((root / 'allocation.json').read_text())
    require(selected == allocate(population['candidates'], json.loads((root / 'exclusions.json').read_text()), plan),
            'allocation differs from deterministic exclusion and order')
    build_evidence(root, contract['revision'])
    verify(root, plan)
    return selected


def resolution(width, height):
    return next((c for c, w, h in (('sd', 640, 480), ('720p', 1280, 720), ('1080p', 1920, 1088))
                 if width <= w and height <= h), '4k')


def evaluation_cohort(root, plan, allocation_sha256):
    cohort = json.loads(EVALUATION.read_text())
    if allocation_sha256 != cohort['allocation_sha256']:
        return None
    require(set(cohort) == {'schema', 'plan_sha256', 'allocation_sha256', 'acquisition_sha256',
                            'snapshot', 'counts', 'inputs'} and cohort['schema'] == 1 and
            cohort['plan_sha256'] == digest(PLAN), 'evaluation registration changed')
    require(digest(root / 'allocation-contract.json') == allocation_sha256 and
            digest(root / 'acquisition-receipt.json') == cohort['acquisition_sha256'],
            'evaluation allocation or acquisition receipt changed')
    require(set(cohort['snapshot']) == {'operator/amended-state.json', 'collection.json', 'roster.json',
                                       'acquisition-report.json'} and
            all(digest(root / name) == sha for name, sha in cohort['snapshot'].items()),
            'registered evaluation snapshot changed')
    fields = {'label', 'class', 'source_sha256', 'sha256', 'width', 'height', 'source_frames', 'frames'}
    require(len(cohort['inputs']) == 57 and all(set(row) == fields for row in cohort['inputs']) and
            set(cohort['counts']) == set(bars.CLASSES) and
            all(type(n) is int and 0 < n <= plan['count_per_class'] for n in cohort['counts'].values()) and
            cohort['counts'] == {c: sum(r['class'] == c for r in cohort['inputs']) for c in bars.CLASSES},
            'registered evaluation counts differ')
    rows = json.loads((root / 'roster.json').read_text())
    require([{k: row[k] for k in fields} for row in rows] == cohort['inputs'],
            'roster differs from registered evaluation inputs or order')
    require({p.name for p in (root / 'media').glob('h*.h264')} == {r['label'] + '.h264' for r in rows},
            'registered evaluation media roster changed')
    require(all(digest(root / 'media' / (r['label'] + '.h264')) == r['sha256'] for r in rows),
            'registered evaluation media changed')
    return cohort


def selection(rows, candidates, decisions, registry, plan, *, expected_counts=None):
    from h264_pgo_acquire import failure_counts
    failure_counts(decisions, plan)
    used = exclusions(registry)
    require(len(decisions) <= len(candidates), 'collection extends population')
    counts = {c: 0 for c in bars.CLASSES}
    chosen, groups, hashes = [], set(), set()
    for candidate, decision in zip(candidates, decisions):
        require(source_identity(decision['identity']) == candidate['identity'], 'collection is not an ordered population prefix')
        require(not all(n == plan['count_per_class'] for n in counts.values()), 'collection continued after quotas filled')
        if decision['capture_status'] == 'failed':
            continue
        metadata = decision['metadata']
        require(set(metadata) == {'codec_name', 'pix_fmt', 'field_order', 'profile', 'width', 'height', 'frames'} and
                decision['capture_status'] == 'ok', 'collection lacks a complete metadata capture')
        eligible = (metadata['codec_name'] == 'h264' and metadata['pix_fmt'] == 'yuv420p' and
                    metadata['field_order'] == 'progressive' and metadata['profile'] in ('Constrained Baseline', 'Main', 'High') and
                    all(type(metadata[k]) is int and metadata[k] > 0 for k in ('width', 'height', 'frames')))
        sha = decision['source_sha256']
        require(re.fullmatch('[a-f0-9]{64}', sha) is not None, 'invalid acquired content digest')
        if sha in used['hashes'] or sha[:12] in used['hashes'] or sha in hashes:
            reason = 'used-bytes'
        elif set(candidate['groups']) & groups:
            reason = 'group-used'
        elif not eligible:
            reason = 'unsupported'
        elif counts[resolution(metadata['width'], metadata['height'])] == plan['count_per_class']:
            reason = 'class-full'
        else:
            reason = None
        if reason is not None:
            require(decision['status'] == 'excluded' and decision['reason'] == reason, 'unsupported collection exclusion')
            continue
        require(decision['status'] == 'selected', 'eligible input was skipped')
        cls = resolution(metadata['width'], metadata['height'])
        chosen.append({'identity': candidate['identity'], 'groups': candidate['groups'], 'source_sha256': sha,
                       'class': cls, 'width': metadata['width'], 'height': metadata['height']})
        groups.update(candidate['groups'])
        hashes.add(sha)
        counts[cls] += 1
    expected_counts = expected_counts or {c: plan['count_per_class'] for c in bars.CLASSES}
    require(counts == expected_counts and len(chosen) == len(rows), 'collection quotas are incomplete')
    for row, expected in zip(rows, chosen):
        require(all(source_identity(row[k]) == v if k == 'identity' else row[k] == v for k, v in expected.items()),
                'roster differs from first eligible independent inputs')


def reference_evidence(refs):
    from h264_pgo_build import training_roster
    approved = training_roster(ROOT)
    require(len(refs) == 4 and {r['class'] for r in refs} == set(bars.CLASSES), 'four training references required')
    require(all(re.fullmatch('r[0-9]{3}', r['label']) is not None and
                any(r['sha256'] == a['es_sha256'] and r['class'] == a['class'] for a in approved.values())
                for r in refs), 'calibration reference is not approved training data')


def exclusions(registry):
    require(isinstance(registry, dict) and set(registry) == COVERAGE, 'exclusion registry must cover all four input owners')
    result = {key: set() for key in ('hashes', 'identities', 'groups')}
    for record in registry.values():
        require(set(record) == {'commitment', *result}, 'incomplete exclusion component')
        require(re.fullmatch('[a-f0-9]{64}', record['commitment']) is not None, 'invalid registry commitment')
        for key in result:
            require(isinstance(record[key], list) and bool(record[key]), 'empty exclusion component')
            result[key].update(source_identity(v) if key == 'identities' else v for v in record[key])
    return result


def source_identity(url):
    require(isinstance(url, str) and bool(url), 'source identity must be a nonempty string')
    value = urllib.parse.urlsplit(url)
    return urllib.parse.urlunsplit((value.scheme.lower(), value.netloc.lower(), value.path, value.query, ''))


def exclusion_registry(owners):
    require(isinstance(owners, dict) and set(owners) == COVERAGE, 'exclusion owners are incomplete')
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
    candidates = [r | {'identity': source_identity(r['identity'])} for r in candidates]
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


def roster(rows, registry, plan, *, counts=None):
    used = exclusions(registry)
    counts = counts or {c: plan['count_per_class'] for c in bars.CLASSES}
    require(len(rows) == sum(counts.values()), 'incomplete held-out roster')
    require(len({r['label'] for r in rows}) == len(rows), 'duplicate label')
    require(len({r['source_sha256'] for r in rows}) == len(rows), 'duplicate source bytes')
    require(len({r['sha256'] for r in rows}) == len(rows), 'duplicate extracted bytes')
    require(len({source_identity(r['identity']) for r in rows}) == len(rows), 'duplicate source identity')
    groups = [g for r in rows for g in r['groups']]
    require(len(groups) == len(set(groups)), 'source group contributes more than one file')
    for cls in bars.CLASSES:
        require(sum(r['class'] == cls for r in rows) == counts[cls], 'incomplete class')
    for row in rows:
        require(re.fullmatch('h[0-9]{3}', row['label']) is not None, 'use neutral input labels')
        require(source_identity(row['identity']) not in used['identities'] and not set(row['groups']) & used['groups'], 'used source')
        require(row['source_sha256'] not in used['hashes'] and row['sha256'] not in used['hashes'], 'used input bytes')
        require(row['source_sha256'][:12] not in used['hashes'] and row['sha256'][:12] not in used['hashes'], 'used input prefix')
        require(row['groups'] and row['class'] in bars.CLASSES, 'invalid input metadata')
        require(all(type(row[k]) is int and row[k] > 0 for k in ('width', 'height')), 'invalid geometry')
        cls = resolution(row['width'], row['height'])
        require(row['class'] == cls, 'class disagrees with displayed geometry')
        require(all(re.fullmatch('[a-f0-9]{64}', row[k]) is not None for k in ('sha256', 'source_sha256')), 'invalid content digest')
    return {r['label']: r['class'] for r in rows}


def provider_host(instance):
    require(isinstance(instance, dict) and isinstance(instance.get('machine_type'), str) and
            (instance['machine_type'].endswith('/c4-standard-16') or instance['machine_type'] == 'c4-standard-16'), 'wrong machine')
    require(instance['provisioning'] == 'STANDARD', 'timing requires on-demand')
    features = instance['advanced_machine_features']
    require(isinstance(features, dict) and type(features['threadsPerCore']) is int and
            features['threadsPerCore'] == 1 and features['turboMode'] == 'ALL_CORE_MAX', 'wrong provider CPU settings')


def host(instance):
    provider_host(instance)
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


def execute(argv, maximum=0, timeout=180, capture=False):
    start = time.perf_counter()
    h = hashlib.sha256()
    size, error = 0, bytearray()
    output = bytearray()
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
                                if capture:
                                    output.extend(data)
                            else:
                                error.extend(data)
                                require(len(error) <= 65536, 'diagnostic output exceeds budget')
                    if process.wait() != 0 or error:
                        raise subprocess.CalledProcessError(process.returncode, argv, stderr=bytes(error))
                    value = {'wall': elapsed, 'bytes': size, 'sha256': h.hexdigest()}
                    return value | {'stdout': output.decode('utf-8')} if capture else value
                finally:
                    os.close(fd)
        finally:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass


def command(root, row, arm, threads, raw, plan, oracle):
    path = str(root / 'media' / (row['label'] + '.h264'))
    prefix = ['taskset', '-c', '2' if threads == 1 else '0-7', str(oracle.ffmpeg if arm == 'ffmpeg' else root / arm)]
    count = str(plan['frames'][row['class']])
    if arm != 'ffmpeg':
        return prefix + ['--threads', str(threads), '--frames', count] + ([] if raw else ['--discard']) + [path]
    return prefix + ['-nostdin', '-v', 'error', '-threads', str(threads), '-i', path,
                     '-frames:v', count, '-fps_mode', 'passthrough'] + (
        ['-f', 'rawvideo', '-pix_fmt', 'yuv420p', '-'] if raw else ['-f', 'null', '-'])


def stream_metadata(root, row, oracle):
    result = execute([str(oracle.ffprobe), '-v', 'error', '-threads', '1', '-select_streams', 'v:0',
                      '-show_entries', 'stream=codec_name,pix_fmt,width,height,field_order,profile', '-of', 'json',
                      str(root / 'media' / (row['label'] + '.h264'))], maximum=65536, timeout=30, capture=True)
    streams = json.loads(result['stdout'])['streams']
    require(len(streams) == 1, 'input must have one selected video stream')
    validate_metadata(streams[0], row)
    return streams[0]


def validate_metadata(metadata, row):
    require(metadata['codec_name'] == 'h264' and metadata['pix_fmt'] == 'yuv420p' and
            metadata['field_order'] == 'progressive' and
            metadata['profile'] in ('Constrained Baseline', 'Main', 'High') and
            metadata['width'] == row['width'] and metadata['height'] == row['height'], 'input metadata differs from admission')


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


def audit(root, plan, allocation_sha256):
    import h264_pgo_diagnostics as diagnostics
    cohort = evaluation_cohort(root, plan, allocation_sha256)
    counts = cohort['counts'] if cohort else None
    binding = json.loads((root / 'binding.json').read_text())
    require(set(binding) == {'plan', 'revision', 'tools', 'files', 'allocation_sha256'} and
            binding['allocation_sha256'] == allocation_sha256, 'incomplete binding schema')
    candidates = verify_allocation(root, plan, allocation_sha256, binding['revision'])
    rows = json.loads((root / 'roster.json').read_text())
    refs = json.loads((root / 'references.json').read_text())
    classes = roster(rows, json.loads((root / 'exclusions.json').read_text()), plan, counts=counts)
    reference_evidence(refs)
    require(isinstance(binding['files'], dict) and set(binding['files']) == set(evidence_inventory(rows, refs)),
            'incomplete artifact inventory')
    require(isinstance(binding['tools'], dict) and set(binding['tools']) == {'scripts/h264_pgo_round.py', 'scripts/h264_pgo_build.py',
                                    'scripts/h264_parity_bars.py', 'scripts/h264_pgo_diagnostics.py'} |
                                    ({'scripts/h264_pgo_evaluation.json'} if cohort else set()),
            'incomplete tool inventory')
    require(binding['plan'] == digest(PLAN), 'registration changed')
    for name, expected in binding['files'].items():
        require(digest(root / name) == expected, 'bound file changed')
    for name, expected in binding['tools'].items():
        require(digest(ROOT / name) == expected, 'executor or estimator changed')
    reservation_receipt(root, plan)
    build_evidence(root, binding['revision'])
    selection(rows, candidates, json.loads((root / 'collection.json').read_text()),
              json.loads((root / 'exclusions.json').read_text()), plan, expected_counts=counts)
    require(all(digest(root / 'media' / (r['label'] + '.h264')) == r['sha256'] for r in rows + refs), 'input digest differs from roster')
    events = [json.loads(line) for line in (root / 'events.jsonl').read_text().splitlines()]
    require(events and events[-1] == {'kind': 'complete'}, 'incomplete or aborted run')
    require(not any(r['kind'] == 'abort' for r in events), 'aborted run')
    require(all(r['kind'] in {'host', 'metadata', 'qualification', 'calibration-call', 'calibration', 'health', 'warmup', 'timed', 'diagnostics', 'complete'}
                for r in events), 'unknown event kind')
    hosts = [r for r in events if r['kind'] == 'host']
    require(len(hosts) == 2 and [r['phase'] for r in hosts] == ['before', 'after'] and events[0]['kind'] == 'host', 'missing host evidence')
    for entry in hosts:
        provider = entry['instance']
        provider_host(provider)
        require(provider == json.loads((root / 'instance.json').read_text()), 'invalid provider host')
        require(entry['affinity'] == list(range(8))
                and entry['topology'] == [str(c) for c in range(8)] and entry['cpu'] == '8581C', 'wrong host class')
    checks = [r for r in events if r['kind'] == 'qualification']
    wanted = {(r['label'], a, t) for r in rows for a in ARMS for t in (1, 8)}
    require(len(checks) == len(wanted) and {(r['label'], r['arm'], r['threads']) for r in checks} == wanted,
            'incomplete qualification')
    metadata = [r for r in events if r['kind'] == 'metadata']
    require(len(metadata) == len(rows) and {r['label'] for r in metadata} == set(classes), 'missing metadata checks')
    for row in rows:
        validate_metadata(next(r['value'] for r in metadata if r['label'] == row['label']), row)
        meta_index = next(i for i, r in enumerate(events) if r['kind'] == 'metadata' and r['label'] == row['label'])
        first_check = next(i for i, r in enumerate(events) if r['kind'] == 'qualification' and r['label'] == row['label'])
        require(meta_index < first_check, 'input metadata followed qualification')
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
    indices = {k: [i for i, r in enumerate(events) if r['kind'] == k] for k in
               ('host', 'health', 'qualification', 'warmup', 'timed', 'calibration', 'complete')}
    require(indices['calibration'][-1] < indices['health'][0] < indices['qualification'][0] and
            indices['qualification'][-1] < indices['warmup'][0] and indices['warmup'][-1] < indices['timed'][0] and
            indices['timed'][-1] < indices['health'][1] < indices['host'][1] < indices['complete'][0], 'event phase order differs')
    diagnostic_markers = [i for i, r in enumerate(events) if r['kind'] == 'diagnostics']
    require(len(diagnostic_markers) == 1 and indices['timed'][-1] < diagnostic_markers[0] < indices['health'][1],
            'missing or misplaced diagnostic phase')
    diagnostic_result = diagnostics.audit(root, plan)
    require(events[diagnostic_markers[0]]['status'] == diagnostic_result['status'], 'diagnostic terminal status differs')
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
    result['diagnostics'] = diagnostic_result
    return result


def run(root, plan, allocation_sha256, acquisition_sha256=None):
    import h264_pgo_diagnostics as diagnostics
    import h264_pgo_acquire as acquisition
    tooling_revision = merged()
    out = root / 'events.jsonl'
    with out.open('x', buffering=1) as log:
        def emit(row):
            log.write(json.dumps(row, allow_nan=False) + '\n')
        try:
            reservation_receipt(root, plan)
            receipt = json.loads((root / 'reservation-receipt.json').read_text())
            require(receipt == reservation(root, plan, receipt['commit']),
                    'allocation is not reserved on its merged main')
            require(reserved_evidence(root, plan, receipt) == reserved_inputs(root, plan, receipt),
                    'standing exclusion source differs from allocation')
            cohort = evaluation_cohort(root, plan, allocation_sha256)
            require(cohort is not None, 'evaluation requires the registered cohort allocation')
            acquisition.verify_receipt(root, acquisition_sha256, tooling_revision)
            revision = json.loads((root / 'allocation-contract.json').read_text())['revision']
            require(revision == acquisition.policy()['artifact_revision'], 'measured artifact revision changed')
            candidates = verify_allocation(root, plan, allocation_sha256, revision)
            acquisition.audit_collection(root, candidates, plan, acquisition_sha256)
            acquisition.bind(root, acquisition_sha256)
            instance = json.loads((root / 'instance.json').read_text())
            emit({'kind': 'host', 'phase': 'before', **host(instance)})
            rows = json.loads((root / 'roster.json').read_text())
            roster(rows, json.loads((root / 'exclusions.json').read_text()), plan, counts=cohort['counts'])
            selection(rows, candidates, json.loads((root / 'collection.json').read_text()),
                      json.loads((root / 'exclusions.json').read_text()), plan, expected_counts=cohort['counts'])
            subprocess.run([sys.executable, '-I', str(ROOT / 'scripts/oracle_pin.py'), '--verify-bin', str(root)],
                           check=True, timeout=30)
            oracle = test_oracle.require(root / 'ffmpeg', root / 'ffprobe')
            refs = json.loads((root / 'references.json').read_text())
            reference_evidence(refs)
            build_evidence(root, revision)
            names = evidence_inventory(rows, refs)
            require(digest(root / 'aa') == digest(root / 'base') == digest(root / 'null'), 'null binaries differ')
            for row in rows + refs:
                require(digest(root / 'media' / (row['label'] + '.h264')) == row['sha256'], 'input changed')
            write(root / 'binding.json', {'plan': digest(PLAN), 'revision': revision, 'allocation_sha256': allocation_sha256,
                  'tools': {str(p.relative_to(ROOT)): digest(p) for p in (Path(__file__), ROOT / 'scripts/h264_parity_bars.py',
                             ROOT / 'scripts/h264_pgo_build.py', ROOT / 'scripts/h264_pgo_diagnostics.py', EVALUATION)},
                  'files': {n: digest(root / n) for n in names}})
            for threads in (1, 8):
                for comparison, arm in (('aa', 'aa'), ('null', 'null')):
                    ratios = []
                    for row in refs:
                        samples = {'base': [], arm: []}
                        for n in range(plan['rounds']):
                            for a in (('base', arm) if n % 2 == 0 else (arm, 'base')):
                                value = execute(command(root, row, a, threads, False, plan, oracle))
                                samples[a].append(value['wall'])
                                emit({'kind': 'calibration-call', 'class': row['class'], 'comparison': comparison,
                                      'arm': a, 'threads': threads, 'round': n, **value})
                        ratios.append(statistics.median(samples[arm]) / statistics.median(samples['base']))
                    ratio = statistics.median(ratios)
                    emit({'kind': 'calibration', 'threads': threads, 'comparison': comparison, 'ratio': ratio})
                    require(plan['calibration_min'] <= ratio <= plan['calibration_max'], 'failed calibration')
            def health(phase):
                references = {str(t): {r['class']: statistics.median(execute(command(root, r, 'base', t, False, plan, oracle))['wall']
                              for _ in range(3)) for r in refs} for t in (1, 8)}
                emit({'kind': 'health', 'phase': phase, 'references': references})
            health('before')
            for row in rows:
                emit({'kind': 'metadata', 'label': row['label'], 'value': stream_metadata(root, row, oracle)})
                pixels = row['width'] * row['height'] + 2 * ((row['width'] + 1) // 2) * ((row['height'] + 1) // 2)
                outputs = set()
                for threads in (1, 8):
                    for arm in ARMS:
                        value = execute(command(root, row, arm, threads, True, plan, oracle), pixels * plan['frames'][row['class']])
                        require(value['bytes'] > 0 and value['bytes'] % pixels == 0, 'invalid raw frame length')
                        outputs.add((value['bytes'], value['sha256']))
                        emit({'kind': 'qualification', 'label': row['label'], 'arm': arm, 'threads': threads,
                              **value, 'frames': value['bytes'] // pixels})
                require(len(outputs) == 1, 'raw output mismatch before timing')
            for row in rows:
                for threads in (1, 8):
                    for arm in ARMS:
                        execute(command(root, row, arm, threads, False, plan, oracle))
                        emit({'kind': 'warmup', 'label': row['label'], 'arm': arm, 'threads': threads})
            lookup = {r['label']: r for r in rows}
            for cell in schedule(rows, plan):
                value = execute(command(root, lookup[cell['label']], cell['arm'], cell['threads'], False, plan, oracle))
                emit({'kind': 'timed', **cell, **value})
            status = diagnostics.run(root, plan, execute)
            emit({'kind': 'diagnostics', 'status': status})
            health('after')
            emit({'kind': 'host', 'phase': 'after', **host(instance)})
            emit({'kind': 'complete'})
        except BaseException:
            emit({'kind': 'abort'})
            raise


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('action', choices=('registry', 'allocate', 'run', 'audit'))
    ap.add_argument('directory', type=Path)
    ap.add_argument('--allocation-sha256', help='the allocation contract digest recorded before media acquisition')
    ap.add_argument('--acquisition-sha256', help='the amendment receipt digest recorded before continuation')
    args = ap.parse_args()
    plan = json.loads(PLAN.read_text())
    root = args.directory.resolve()
    try:
        if args.action == 'registry':
            owners = json.loads((root / 'registry-inputs.json').read_text())
            receipt = reservation(root, plan)
            value = reserved_inputs(root, plan, receipt)
            config = json.loads((root / 'reservation-checkout.json').read_text())
            owners['other-draws'] = [*owners['other-draws'], config['frozen'],
                                    str(Path(config['checkout']) / config['manifest'])]
            registry = exclusion_registry(owners)
            component = registry['other-draws']
            component['hashes'] = sorted(set(component['hashes']) | set(value['hashes']))
            component['commitment'] = hashlib.sha256(json.dumps([component['commitment'], value], sort_keys=True).encode()).hexdigest()
            write(root / 'reservation-receipt.json', receipt)
            write(root / 'reserved-inputs.json', value)
            write(root / 'exclusions.json', registry)
            reserved_evidence(root, plan, receipt)
            return 0
        if args.action == 'allocate':
            from h264_pgo_diagnostics import verify
            revision = merged()
            build_evidence(root, revision)
            verify(root, plan)
            require(not (root / 'allocation-contract.json').exists(), 'allocation is already frozen')
            receipt = reservation(root, plan)
            require(receipt == json.loads((root / 'reservation-receipt.json').read_text()), 'reservation changed; rebuild registry')
            require(reserved_evidence(root, plan, receipt) == reserved_inputs(root, plan, receipt), 'standing exclusions changed')
            population, snapshots = population_files(root, receipt, plan)
            write(root / 'allocation.json', allocate(population['candidates'], json.loads((root / 'exclusions.json').read_text()), plan))
            contract = {'schema': 1, 'plan': digest(PLAN), 'revision': revision,
                        'files': {n: digest(root / n) for n in allocation_files(snapshots)}}
            with (root / 'allocation-contract.json').open('x') as f:
                json.dump(contract, f, indent=2, allow_nan=False)
            print(digest(root / 'allocation-contract.json'))
            return 0
        if args.action == 'run':
            run(root, plan, args.allocation_sha256, args.acquisition_sha256)
        from h264_pgo_acquire import audit_bound
        acquisition_result = audit_bound(root, plan, args.allocation_sha256, args.acquisition_sha256)
        result = audit(root, plan, args.allocation_sha256)
        result['acquisition'] = acquisition_result
        write(root / 'summary.json', result)
        return 0 if result['pass'] else 1
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError, bars.Refused):
        write(root / 'failure.json', {'status': 'incomplete', 'speed_verdict': None})
        print('PGO round incomplete; retain events and inspect private diagnostics', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
