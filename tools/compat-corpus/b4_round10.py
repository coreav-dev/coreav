#!/usr/bin/env python3
"""Freeze, execute and report B4 round 10. Freeze reads metadata, never media.

The run command is Linux-only. Its default is development calibration only.
Product execution requires --mode product and a passing complete calibration.
Keep the frozen manifest private. Results contain only opaque file aliases.
"""
import argparse
import collections
import hashlib
import json
import math
import os
import pathlib
import platform
import resource
import signal
import statistics
import subprocess
import sys
import time

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import b4_protocol as protocol
import release_round
import round_files
import speed

ROOT = HERE.parent.parent
PLAN = 'docs/measurements/compat-ffprobe/round10-plan.md'
DESIGN = 'docs/design/b4-calibration.md'
TOOL_FILES = (PLAN, DESIGN, 'tools/compat-corpus/b4_round10.py',
              'tools/compat-corpus/b4_protocol.py', 'tools/compat-corpus/speed.py',
              'tools/compat-corpus/round_files.py', 'tools/compat-corpus/release_round.py',
              'tools/stage3-report/r2pool.py', 'scripts/oracle_pin.py', 'scripts/oracle-ffmpeg.env',
              'scripts/gcp_vm_job.py')
COUNTERS = ('ru_utime', 'ru_stime', 'ru_minflt', 'ru_majflt', 'ru_nvcsw', 'ru_nivcsw')
PRODUCT_COUNTS = {'isobmff': 20, 'matroska': 20, 'mpegts': 17}
MANIFESTS = ('v1', 'v1-ts-audio', 'v2-heldout')
PLAN_COMMIT = '3c79ef4979aa56bc7b667dc9130833b88f2be634'


def encoded(data):
    return json.dumps(data, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()


def digest(path):
    return round_files.digest(path)


def identity(value, length=64):
    return isinstance(value, str) and len(value) == length and all(c in '0123456789abcdef' for c in value)


def records(path):
    data = json.loads(pathlib.Path(path).read_text())
    rows = data.get('files', data.get('entries')) if isinstance(data, dict) else data
    if not isinstance(rows, list):
        raise ValueError('invalid roster metadata')
    return rows


def select_rosters(root, extra_rosters=()):
    root = pathlib.Path(root)
    paths = sorted((root / 'corpus').glob('*/manifest.json'))
    required = {root / 'corpus' / name / 'manifest.json' for name in MANIFESTS}
    if not required.issubset(paths):
        raise ValueError('all registered corpus manifests are required')
    metadata = {path: records(path) for path in paths}
    heldout = set()
    for rows in metadata.values():
        for row in rows:
            if row.get('holdout') is True:
                if not identity(row.get('sha256')):
                    raise ValueError('invalid registered held-out digest')
                heldout.add(row['sha256'])
    for path in extra_rosters:
        rows = records(path)
        metadata[pathlib.Path(path)] = rows
        for row in rows:
            if not identity(row.get('sha256')):
                raise ValueError('invalid additional held-out digest')
            heldout.add(row['sha256'])
    selected = {'calibration': [], 'product': []}
    for family, (corpus, extensions) in speed.CLASSES.items():
        rows = metadata[root / 'corpus' / corpus / 'manifest.json']
        for phase, partition, salt, count in (
                ('calibration', False, 'coreav-b4-calibration-v2:', 8),
                ('product', True, speed.SALT, PRODUCT_COUNTS[family])):
            pool = [row for row in rows if row.get('set') == 'real'
                    and row.get('holdout') is partition and row.get('ext') in extensions
                    and type(row.get('size')) is int and 0 < row['size'] <= speed.LIMIT]
            if any(not identity(row.get('sha256')) for row in pool):
                raise ValueError('invalid selection digest')
            pool.sort(key=lambda row: hashlib.sha256((salt + row['sha256']).encode()).hexdigest())
            if len(pool) < count:
                raise ValueError('unsupported: too few registered files in a container')
            if phase == 'product' and count < speed.PER_CONTAINER and len(pool) != count:
                raise ValueError('historical product roster changed')
            for rank, row in enumerate(pool[:count]):
                key = row['key'] if '/' in row['key'] else f"corpus/{corpus}/{row['key']}"
                selected[phase].append({'class': family, 'file': f'{phase}:{family}:{rank}',
                                        'rank': rank, 'sha256': row['sha256'], 'size': row['size'],
                                        'ext': row['ext'], 'key': key})
    dev = [row['sha256'] for row in selected['calibration']]
    product = [row['sha256'] for row in selected['product']]
    if len(set(dev)) != len(dev) or len(set(product)) != len(product) or set(dev) & heldout:
        raise ValueError('selection is duplicated or calibration overlaps a held-out roster')
    return selected, [{'sha256': digest(path), 'files': len(rows)} for path, rows in metadata.items()]


def checked(command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def verify_sources(sources):
    if sources.get('plan') != PLAN_COMMIT:
        raise ValueError('the merged round-10 plan commit is required')
    for name in ('plan', 'tooling', 'candidate', 'baseline'):
        if not identity(sources.get(name), 40):
            raise ValueError('sources require immutable public commits')
        subprocess.run(['git', 'merge-base', '--is-ancestor', sources[name], 'origin/main'], cwd=ROOT, check=True)
    if sources['baseline'] != checked(['git', 'rev-parse', 'origin/main']):
        raise ValueError('baseline must be fetched main')
    if sources['tooling'] != checked(['git', 'rev-parse', 'HEAD']):
        raise ValueError('tooling must match this checkout')
    if checked(['git', 'status', '--porcelain', '--untracked-files=no']):
        raise ValueError('tooling checkout has tracked changes')
    rules = json.loads(checked(['gh', 'api', 'repos/coreav-dev/coreav/rules/branches/main']))
    required = {check['context'] for rule in rules if rule['type'] == 'required_status_checks'
                for check in rule['parameters']['required_status_checks']}
    if not required:
        raise ValueError('required CI checks are unavailable')
    for commit in {sources['candidate'], sources['tooling'], sources['baseline']}:
        pages = json.loads(checked(['gh', 'api', '--paginate', '--slurp',
                                   f'repos/coreav-dev/coreav/commits/{commit}/check-runs?per_page=100']))
        checks = {}
        for page in pages:
            for check in page['check_runs']:
                if check['name'] not in checks or check['id'] > checks[check['name']]['id']:
                    checks[check['name']] = check
        if any(checks.get(name, {}).get('conclusion') != 'success'
               or checks.get(name, {}).get('status') != 'completed' for name in required):
            raise ValueError('required CI is not green at a frozen revision')
    builds = sources.get('builds', {})
    for name in ('candidate', 'baseline'):
        build = builds.get(name, {})
        if (build.get('commit') != sources[name] or build.get('profile') != 'release'
                or build.get('features') != 'default' or not identity(build.get('sha256'))):
            raise ValueError('default-feature release build provenance is required')


def freeze(opts):
    selected, manifests = select_rosters(opts.manifests, opts.heldout_roster)
    historical = records(opts.product_roster)
    identities = [{key: row[key] for key in ('class', 'sha256', 'ext', 'size')} for row in selected['product']]
    if identities != historical:
        raise ValueError('product selection differs from the historical frozen roster')
    sources = json.loads(pathlib.Path(opts.sources).read_text())
    checked(['git', 'fetch', 'origin', 'main'])
    verify_sources(sources)
    subprocess.run([sys.executable, str(ROOT / 'scripts/oracle_pin.py'), '--verify-bin', opts.oracle_bin], check=True)
    binaries = {name: digest(path) for name, path in (
        ('candidate', opts.coreav), ('baseline', opts.baseline_coreav),
        ('ffprobe', pathlib.Path(opts.oracle_bin) / 'ffprobe'), ('ffmpeg', pathlib.Path(opts.oracle_bin) / 'ffmpeg'))}
    if any(sources['builds'][name]['sha256'] != binaries[name] for name in ('candidate', 'baseline')):
        raise ValueError('binary differs from its build provenance')
    commands = speed.native_commands()
    if len(commands) != 93:
        raise ValueError('registered native command roster changed')
    data = {'method': protocol.METHOD, 'sources': sources, 'binaries': binaries,
            'metadata': manifests, 'product_roster_sha256': digest(opts.product_roster),
            'selection': selected, 'commands': commands,
            'command_sha256': digest(speed.COMMANDS),
            'tools': {name: digest(ROOT / name) for name in TOOL_FILES},
            'schedule': protocol.calibration_schedule() + protocol.product_schedule(selected['product'], commands)}
    with pathlib.Path(opts.out).open('x') as out:
        out.write(json.dumps(data, indent=1) + '\n')
    return 0


def load_manifest(path):
    manifest = json.loads(pathlib.Path(path).read_text())
    if manifest.get('method') != protocol.METHOD:
        raise ValueError('historical B4 schemas cannot be re-scored')
    selections = manifest['selection']
    if public_selection(selections) != registered_selection():
        raise ValueError('frozen roster aliases differ from the plan')
    seen = set()
    for phase, expected in (('calibration', dict.fromkeys(protocol.FAMILIES, 8)), ('product', PRODUCT_COUNTS)):
        entries = selections[phase]
        if collections.Counter(row['class'] for row in entries) != expected:
            raise ValueError('frozen roster counts differ from the plan')
        for family in protocol.FAMILIES:
            rows = [row for row in entries if row['class'] == family]
            for rank, row in enumerate(rows):
                if (row['rank'] != rank or row['file'] != f'{phase}:{family}:{rank}'
                        or not identity(row['sha256']) or row['sha256'] in seen
                        or row['ext'] not in speed.CLASSES[family][1]
                        or type(row['size']) is not int or not 0 < row['size'] <= speed.LIMIT):
                    raise ValueError('invalid frozen roster identity')
                seen.add(row['sha256'])
    if manifest['commands'] != speed.native_commands() or digest(speed.COMMANDS) != manifest['command_sha256']:
        raise ValueError('command roster differs from the freeze')
    expected = protocol.calibration_schedule() + protocol.product_schedule(selections['product'], manifest['commands'])
    if manifest['schedule'] != expected:
        raise ValueError('call schedule differs from the freeze')
    if manifest['tools'] != {name: digest(ROOT / name) for name in TOOL_FILES}:
        raise ValueError('judging tools differ from the freeze')
    return manifest


def public_selection(selection):
    return {phase: [{key: row[key] for key in ('class', 'file', 'rank')} for row in selection[phase]]
            for phase in ('calibration', 'product')}


def registered_selection():
    return {phase: [{'class': family, 'file': f'{phase}:{family}:{rank}', 'rank': rank}
                    for family in protocol.FAMILIES for rank in range(counts[family])]
            for phase, counts in (('calibration', dict.fromkeys(protocol.FAMILIES, 8)), ('product', PRODUCT_COUNTS))}


def optional_read(path, parse):
    try:
        return {'status': 'available', 'value': parse(pathlib.Path(path).read_text())}
    except OSError:
        return {'status': 'unavailable', 'reason': 'not exposed'}
    except (ValueError, IndexError):
        return {'status': 'invalid', 'reason': 'malformed counter'}


def snapshot(cpu):
    usage = resource.getrusage(resource.RUSAGE_CHILDREN)
    child = {name[3:]: getattr(usage, name) for name in COUNTERS}
    if any(not math.isfinite(value) or value < 0 for value in child.values()):
        raise ValueError('mandatory child resource capture is invalid')
    def steal(text):
        row = next((line.split() for line in text.splitlines() if line.startswith(f'cpu{cpu} ')), [])
        value = int(row[8])
        if value < 0:
            raise ValueError('negative steal')
        return value
    def frequency(text):
        value = int(text.strip())
        if value <= 0:
            raise ValueError('invalid frequency')
        return value
    base = pathlib.Path(f'/sys/devices/system/cpu/cpu{cpu}/cpufreq')
    return {'child': child, 'steal_ticks': optional_read('/proc/stat', steal),
            'frequency_khz': {name: optional_read(base / name, frequency)
                              for name in ('scaling_cur_freq', 'cpuinfo_cur_freq')}}


def telemetry(before, after):
    child = {name: after['child'][name] - value for name, value in before['child'].items()}
    if any(not math.isfinite(value) or value < 0 for value in child.values()):
        raise ValueError('mandatory child resource delta is invalid')
    first, last = before['steal_ticks'], after['steal_ticks']
    steal = {'before': first, 'after': last}
    if first['status'] == last['status'] == 'available':
        delta = last['value'] - first['value']
        steal.update(status='available' if delta >= 0 else 'invalid', delta=delta if delta >= 0 else None)
        if delta < 0:
            steal['reason'] = 'counter moved backwards'
    else:
        steal.update(status='invalid' if 'invalid' in (first['status'], last['status']) else 'unavailable',
                     reason='counter is not available at both boundaries')
    return {'child': child, 'steal_ticks': steal,
            'frequency_khz': {'before': before['frequency_khz'], 'after': after['frequency_khz']}}


def host_gate(instance, cpu):
    features = instance.get('advanced_machine_features') or {}
    smt = pathlib.Path('/sys/devices/system/cpu/smt/active')
    if (platform.system() != 'Linux' or instance.get('machine_type') != 'c4-standard-8'
            or instance.get('provisioning') != 'STANDARD' or features.get('threadsPerCore') != 1
            or os.cpu_count() != 4 or (smt.exists() and smt.read_text().strip() != '0')):
        raise ValueError('host gate requires the registered on-demand SMT-off class')
    if cpu not in os.sched_getaffinity(0):
        raise ValueError('pinned CPU is not available')
    topology = {}
    for name in ('physical_package_id', 'core_id', 'thread_siblings_list'):
        topology[name] = pathlib.Path(f'/sys/devices/system/cpu/cpu{cpu}/topology/{name}').read_text().strip()
    if set(speed.cpu_list(topology['thread_siblings_list'])) != {cpu}:
        raise ValueError('pinned CPU has an SMT sibling')
    os.sched_setaffinity(0, {cpu})
    if os.sched_getaffinity(0) != {cpu}:
        raise ValueError('parent and children must inherit one pinned core')
    model = next((line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines()
                  if line.startswith('model name')), None)
    if not model:
        raise ValueError('CPU model is unavailable')
    policy = {name: optional_read(f'/sys/devices/system/cpu/cpu{cpu}/cpufreq/{name}', str.strip)
              for name in ('scaling_governor', 'scaling_driver', 'scaling_min_freq', 'scaling_max_freq')}
    return {'machine_type': 'c4-standard-8', 'provisioning': 'STANDARD', 'smt': False,
            'cpu_model': model, 'cpu': cpu, 'topology': topology, 'clock_policy': policy,
            'os': platform.platform(), 'timer': vars(time.get_clock_info('perf_counter'))}


class Incomplete(Exception):
    pass


def payload_pool(entries):
    sys.path.insert(0, str(ROOT / 'tools/stage3-report'))
    import r2pool
    variables = ('R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY', 'R2_ENDPOINT', 'R2_BUCKET')
    if any(not os.environ.get(name) for name in variables):
        raise ValueError('R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY, R2_ENDPOINT and R2_BUCKET are required')
    limits = r2pool.Limits(requests=sum(1 + math.ceil(row['size'] / round_files.PART) for row in entries),
                           bytes_=sum(row['size'] + 1 for row in entries))
    return r2pool.Pool({'access': os.environ['R2_ACCESS_KEY_ID'], 'secret': os.environ['R2_SECRET_ACCESS_KEY'],
                        'endpoint': os.environ['R2_ENDPOINT'], 'bucket': os.environ['R2_BUCKET']},
                       1, limits, timeout=300)


def fallback(log):
    try:
        return speed.b4_fallback(log)
    except OSError:
        return {'lines': None, 'refused': None, 'executed': None, 'invalid': 1,
                'void': True, 'capture_error': 'unreadable log'}


def valid_fallback(counts):
    return (not counts.get('capture_error') and all(type(counts.get(name)) is int and counts[name] >= 0
                for name in ('lines', 'refused', 'executed', 'invalid'))
            and counts['lines'] == counts['refused'] + counts['executed'] + counts['invalid']
            and counts.get('void') is bool(counts['executed'] or counts['invalid']))


def observe(argv, env, cpu, origin):
    try:
        before = snapshot(cpu)
    except (OSError, ValueError, AttributeError):
        return {'executed': False, 'error': 'mandatory resource capture failed before launch', 'exit': None}
    start = time.perf_counter() - origin
    error = None
    try:
        elapsed, code = speed.timed(argv, env, raise_oserror=True)
    except (OSError, TimeoutError):
        elapsed, code, error = time.perf_counter() - origin - start, -1, 'process infrastructure failure'
    end = time.perf_counter() - origin
    try:
        after = snapshot(cpu)
        usage = telemetry(before, after)
    except (OSError, ValueError, AttributeError):
        usage, error = {}, 'mandatory resource capture failed after launch'
    if not math.isfinite(elapsed) or elapsed <= 0:
        error = 'invalid elapsed time'
    return {'executed': True, 'start': start, 'end': end, 'elapsed': elapsed, 'exit': code,
            'telemetry': usage, **({'error': error} if error else {})}


def execute_phase(phase, manifest, directory, coreav, ffprobe, env, log, cpu, origin, safety, receipts):
    files = {row['file']: row for row in manifest['selection'][phase]}
    outcomes = {}
    for pair in manifest['schedule']:
        if pair['phase'] != phase:
            continue
        key = (pair['arm'], pair['file'], pair['command'])
        if outcomes.get(key) == 'refused':
            continue
        if time.perf_counter() >= safety:
            raise Incomplete('safety deadline reached')
        entry = files[pair['file']]
        path = str(directory / f"{entry['sha256']}.{entry['ext']}")
        args = protocol.CALIBRATION_COMMANDS[pair['command']] + [path] if phase == 'calibration' else [
            path if arg == 'INPUT' else arg for arg in manifest['commands'][pair['command']]]
        a = [ffprobe] if pair['arm'] == 'aa' else [coreav, 'ffprobe']
        b = [coreav, 'ffprobe'] if pair['arm'] == 'null' else [ffprobe]
        statuses = {}
        failed = False
        for position, side in enumerate(pair['order']):
            if time.perf_counter() >= safety:
                raise Incomplete('safety deadline reached')
            try:
                call = observe((a if side == 'A' else b) + args, env, cpu, origin)
            except (OSError, ValueError):
                receipts.append({**pair, 'side': side, 'position': position,
                                 'error': 'mandatory resource capture failed', 'exit': None})
                raise Incomplete('mandatory resource capture failed') from None
            receipts.append({**pair, 'side': side, 'position': position, **call})
            statuses[side] = call['exit']
            failed |= bool(call.get('error')) or call['exit'] != 0
            if call.get('error'):
                raise Incomplete('process or capture failure')
        counts = fallback(log)
        if counts['void'] or phase == 'calibration' and counts['lines']:
            raise Incomplete('fallback records invalidate measurement')
        if failed:
            if (phase == 'product' and not pair['timed'] and pair['pair'] == 0
                    and statuses == {'A': 4, 'B': 0} and all(
                        captured_call(row) for row in receipts[-2:])):
                outcomes[key] = 'refused'
            else:
                raise Incomplete('child failure or timeout')
    return outcomes


def run(opts):
    if platform.system() != 'Linux' or not hasattr(os, 'pidfd_open'):
        raise ValueError('measurement runs require Linux process-exit descriptors')
    for value in (opts.max_seconds, opts.cap_usd, opts.usd_per_hour, opts.reserve_usd):
        if not math.isfinite(value) or value <= 0:
            raise ValueError('safety limits must be finite and positive')
    if opts.cap_usd > 10 or opts.max_seconds / 3600 * opts.usd_per_hour + opts.reserve_usd > opts.cap_usd:
        raise ValueError('infrastructure bound exceeds the task cap')
    manifest = load_manifest(opts.manifest)
    if any(digest(path) != manifest['binaries'][name] for name, path in (
            ('candidate', opts.coreav), ('ffprobe', pathlib.Path(opts.oracle_bin) / 'ffprobe'),
            ('ffmpeg', pathlib.Path(opts.oracle_bin) / 'ffmpeg'))):
        raise ValueError('executable differs from the freeze')
    subprocess.run([sys.executable, str(ROOT / 'scripts/oracle_pin.py'), '--verify-bin', opts.oracle_bin], check=True)
    host = host_gate(json.loads(pathlib.Path(opts.instance).read_text()), opts.cpu)
    snapshot(opts.cpu)
    out = pathlib.Path(opts.out)
    if out.exists() or out.with_suffix('.fallback.jsonl').exists():
        raise ValueError('measurement output must be new; do not overwrite a prior attempt')
    out.parent.mkdir(parents=True, exist_ok=True)
    log = out.with_suffix('.fallback.jsonl')
    log.touch(exist_ok=False)
    env = release_round.environment(log.resolve())
    directory = pathlib.Path(opts.dir)
    directory.mkdir(parents=True, exist_ok=True)
    origin = time.perf_counter()
    safety = origin + opts.max_seconds
    def deadline(signum, frame):
        raise TimeoutError('total safety deadline reached')
    data = {'method': protocol.METHOD, 'mode': opts.mode, 'host': host,
            'sources': {name: manifest['sources'][name] for name in ('plan', 'tooling', 'candidate', 'baseline')},
            'binaries': manifest['binaries'], 'command_sha256': manifest['command_sha256'],
            'manifest_sha256': hashlib.sha256(encoded(manifest)).hexdigest(),
            'commands': manifest['commands'],
            'selection': public_selection(manifest['selection']),
            'receipts': [], 'aborted': None}
    ffprobe = str(pathlib.Path(opts.oracle_bin).resolve() / 'ffprobe')
    coreav = str(pathlib.Path(opts.coreav).resolve())
    previous_alarm = signal.signal(signal.SIGALRM, deadline)
    signal.setitimer(signal.ITIMER_REAL, opts.max_seconds)
    try:
        for phase in ('calibration', 'product'):
            if phase == 'product' and (opts.mode != 'product' or not calibration_summary(data)['passes']):
                break
            p = payload_pool(manifest['selection'][phase])
            try:
                for entry in manifest['selection'][phase]:
                    if time.perf_counter() >= safety:
                        raise Incomplete('safety deadline reached during preparation')
                    round_files.fetch_one(p, entry, directory)
            finally:
                p.close()
            for entry in manifest['selection'][phase]:
                path = directory / f"{entry['sha256']}.{entry['ext']}"
                if path.stat().st_size != entry['size'] or digest(path) != entry['sha256']:
                    raise Incomplete('file differs from the frozen roster')
            execute_phase(phase, manifest, directory, coreav, ffprobe, env, log, opts.cpu,
                          origin, safety, data['receipts'])
            data['fallback_log'] = fallback(log)
            if phase == 'calibration':
                data['calibration_fallback_log'] = data['fallback_log']
    except (Incomplete, OSError, ValueError, RuntimeError, SystemExit, KeyboardInterrupt):
        data['aborted'] = 'process, capture, integrity, fallback or safety failure'
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous_alarm)
        data['fallback_log'] = fallback(log)
        data.setdefault('calibration_fallback_log', data['fallback_log'])
        data['calibration'] = calibration_summary(data)
        out.write_text(json.dumps(data, allow_nan=False) + '\n')
    return 0 if not data['aborted'] and summary(data)['passes'] else 1


def calibration_summary(data):
    calls = [row for row in data['receipts'] if row['phase'] == 'calibration']
    expected = [(pair, side, position) for pair in protocol.calibration_schedule()
                for position, side in enumerate(pair['order'])]
    complete = len(calls) == len(expected)
    for actual, (pair, side, position) in zip(calls, expected):
        if any(actual.get(k) != v for k, v in {**pair, 'side': side, 'position': position}.items()):
            raise ValueError('calibration receipts differ from the frozen schedule')
        complete &= valid_call(actual)
    logs = {arm: {family: [[[] for _ in range(2)] for _ in range(8)]
                  for family in protocol.FAMILIES} for arm in protocol.ARMS}
    timed = collections.defaultdict(dict)
    for row in calls:
        if row['timed'] and valid_call(row):
            key = (row['arm'], row['class'], row['rank'], row['command'], row['pair'])
            timed[key][row['side']] = row['elapsed']
    if complete:
        for (arm, family, rank, command, pair), sides in timed.items():
            logs[arm][family][rank][command].append(math.log(sides['A'] / sides['B']))
        arms = protocol.calibration_statistics(logs)
    else:
        arms = {arm: {'estimate': None, 'ci95_diagnostic': None, 'ci995_gate': None,
                      'point_in_band': False, 'gate_contains_one': False} for arm in protocol.ARMS}
    counts = data.get('calibration_fallback_log', data.get('fallback_log', {}))
    no_fallback = valid_fallback(counts) and counts['lines'] == 0
    for arm in arms.values():
        arm.update(complete=bool(complete), no_fallback=no_fallback,
                   passes=bool(complete and no_fallback and arm['point_in_band'] and arm['gate_contains_one']))
    diagnostics = {}
    for name, key in (('case', lambda row: f"{row['arm']}:{row['file']}:{row['command']}"),
                      ('position', lambda row: f"{row['arm']}:{row['command']}:{row['side']}:{row['position']}"),
                      ('early_late', lambda row: f"{row['arm']}:{row['command']}:{row['side']}:{'early' if row['block'] < 2 else 'late'}")):
        grouped = collections.defaultdict(list)
        for row in calls:
            if row['timed'] and valid_call(row):
                grouped[key(row)].append(row['elapsed'])
        diagnostics[name] = {group: {'calls': len(values), 'median_seconds': statistics.median(values)}
                             for group, values in grouped.items()}
    case_logs = collections.defaultdict(dict)
    for (arm, family, rank, command, pair), sides in timed.items():
        if set(sides) == {'A', 'B'}:
            case_logs[f'{arm}:calibration:{family}:{rank}:{command}'][pair] = math.log(sides['A'] / sides['B'])
    for case, ratios in case_logs.items():
        contrasts = [(ratios[pair] + ratios[pair + 1]) / 2 for pair in range(0, 16, 2)
                     if pair in ratios and pair + 1 in ratios]
        diagnostics['case'][case].update(timed_pairs=len(ratios), crossovers=len(contrasts),
                                         estimate=math.exp(statistics.median(contrasts)) if contrasts else None)
    return {'arms': arms, 'gate_confidence': 0.995, 'diagnostic_confidence': 0.95,
            'coverage': {'planned': 4992, 'executed': sum(row.get('executed', False) for row in calls),
                         'missing': 4992 - sum(row.get('executed', False) for row in calls),
                         'timed': sum(row['timed'] for row in calls),
                         'failed': sum(not valid_call(row) for row in calls)},
            'diagnostics': diagnostics, 'passes': all(arm['passes'] for arm in arms.values())}


def captured_call(row):
    child = row.get('telemetry', {}).get('child', {})
    return (not row.get('error') and row.get('executed') is True
            and all(type(row.get(name)) in (int, float) and math.isfinite(row[name]) for name in ('start', 'end', 'elapsed'))
            and row['elapsed'] > 0 and 0 <= row['start'] < row['end']
            and set(child) == {name[3:] for name in COUNTERS}
            and all(type(value) in (int, float) and math.isfinite(value) and value >= 0 for value in child.values()))


def valid_call(row):
    return captured_call(row) and type(row.get('exit')) is int and row['exit'] == 0


def summary(data):
    if data.get('method') != protocol.METHOD:
        raise ValueError('historical B4 schemas cannot be re-scored')
    if (data['mode'] not in ('calibration', 'product') or data['selection'] != registered_selection()
            or data['commands'] != speed.native_commands() or data['command_sha256'] != digest(speed.COMMANDS)):
        raise ValueError('result roster differs from the registered method')
    if any(row['phase'] not in ('calibration', 'product') for row in data['receipts']):
        raise ValueError('unknown observation phase')
    if not valid_fallback(data['fallback_log']):
        raise ValueError('fallback capture is missing or inconsistent')
    if (data['mode'] == 'calibration' and data.get('calibration_fallback_log', data['fallback_log']) != data['fallback_log']
            or data['mode'] == 'product' and data['fallback_log']['lines']
            and 'calibration_fallback_log' not in data):
        raise ValueError('fallback capture does not identify its measurement phase')
    calibration = calibration_summary(data)
    result = {'method': protocol.METHOD, 'mode': data['mode'], 'calibration': calibration,
              'aborted': data['aborted'], 'fallback_log': data['fallback_log'], 'product': None,
              'passes': calibration['passes'] and not data['aborted'],
              'b4_passes': None, 'b4_verdict': 'not executed'}
    if data['mode'] == 'calibration':
        if any(row['phase'] != 'calibration' for row in data['receipts']):
            raise ValueError('development-only mode contains product observations')
        return result
    rows = []
    calls = collections.defaultdict(list)
    for row in data['receipts']:
        if row['phase'] == 'product':
            calls[(row['command'], row['file'])].append(row)
    for command, args in enumerate(data['commands']):
        for entry in data['selection']['product']:
            row = {'command': command, 'class': entry['class'], 'file': entry['file']}
            observed = calls.pop((command, entry['file']), [])
            if not speed.eligible(args, entry['class']):
                if observed:
                    raise ValueError('ineligible product case has observations')
                rows.append({**row, 'ineligible': True})
                continue
            expected = protocol.product_schedule([entry], [args])
            start = (command + entry['rank']) % 2
            expected = [{**pair, 'command': command,
                         'order': ('BA' if (start + pair['pair']) % 2 else 'AB') if pair['timed'] else pair['order']}
                        for pair in expected]
            planned = [{**pair, 'side': side, 'position': position} for pair in expected
                       for position, side in enumerate(pair['order'])]
            for actual, planned_call in zip(observed, planned):
                if any(actual.get(k) != v for k, v in planned_call.items()):
                    raise ValueError('product receipts differ from the frozen schedule')
            if len(observed) > len(planned):
                raise ValueError('duplicate product observations')
            statuses = {call['side']: call.get('exit') for call in observed[:2]}
            if len(observed) == 2 and statuses == {'A': 4, 'B': 0} and all(captured_call(call) for call in observed):
                rows.append({**row, 'refused': True})
            elif len(observed) == 18 and all(valid_call(call) for call in observed):
                sides = {side: [call['elapsed'] for call in observed if call['timed'] and call['side'] == side]
                         for side in 'AB'}
                rows.append({**row, 'ratio': statistics.median(sides['A']) / statistics.median(sides['B'])})
            elif observed:
                rows.append({**row, 'failed': {'phase': 'timed' if any(call['timed'] for call in observed) else 'warmup'}})
    if calls:
        raise ValueError('unexpected product observations')
    expected_count = len(data['commands']) * len(data['selection']['product'])
    coverage = {'total': expected_count, 'ineligible': sum(row.get('ineligible', False) for row in rows),
                'refused': sum(row.get('refused', False) for row in rows),
                'failed': sum('failed' in row for row in rows), 'timed': sum('ratio' in row for row in rows),
                'missing': expected_count - len(rows)}
    coverage['eligible'] = expected_count - coverage['ineligible']
    groups = {}
    for command, args in enumerate(data['commands']):
        for entry in data['selection']['product']:
            if speed.eligible(args, entry['class']):
                for key in (('all', 'all'), ('class', entry['class']), ('command', command),
                            ('command-class', f"{command}:{entry['class']}")):
                    groups.setdefault(key, [])
    for row in rows:
        if 'ratio' in row:
            for key in (('all', 'all'), ('class', row['class']), ('command', row['command']),
                        ('command-class', f"{row['command']}:{row['class']}")):
                groups[key].append(row)
    estimates = []
    for (kind, name), values in groups.items():
        ratios = [row['ratio'] for row in values]
        estimates.append({'group': kind, 'name': name, 'n': len(values),
                          'files': len({row['file'] for row in values}),
                          'median': statistics.median(ratios) if ratios else None,
                          'ci95': speed.interval(values),
                          'faster_share': sum(value < 1 for value in ratios) / len(ratios) if ratios else None})
    passed = (bool(estimates) and all(row['median'] is not None and row['median'] <= 1 for row in estimates)
              and not coverage['failed'] and not coverage['missing'] and not data['fallback_log']['void'])
    result['product'] = {'rows': estimates, 'coverage': coverage, 'passes': bool(passed)}
    result['passes'] &= passed
    result['b4_passes'] = bool(result['passes'])
    result['b4_verdict'] = 'passes' if result['passes'] else 'does not pass'
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    p = sub.add_parser('freeze', help='metadata and executable identities only; no payload access')
    p.add_argument('--manifests', required=True)
    p.add_argument('--heldout-roster', action='append', default=[])
    p.add_argument('--product-roster', required=True)
    p.add_argument('--sources', required=True)
    p.add_argument('--coreav', required=True)
    p.add_argument('--baseline-coreav', required=True)
    p.add_argument('--oracle-bin', required=True)
    p.add_argument('--out', required=True)
    p = sub.add_parser('run')
    for name in ('manifest', 'coreav', 'oracle-bin', 'instance', 'dir', 'out'):
        p.add_argument('--' + name, required=True)
    p.add_argument('--cpu', type=int, required=True)
    p.add_argument('--mode', choices=('calibration', 'product'), default='calibration')
    p.add_argument('--max-seconds', type=float, required=True)
    p.add_argument('--cap-usd', type=float, required=True)
    p.add_argument('--usd-per-hour', type=float, required=True)
    p.add_argument('--reserve-usd', type=float, required=True)
    p = sub.add_parser('report')
    p.add_argument('--results', required=True)
    p.add_argument('--out')
    opts = parser.parse_args()
    try:
        if opts.action == 'freeze':
            return freeze(opts)
        if opts.action == 'run':
            return run(opts)
        result = summary(json.loads(pathlib.Path(opts.results).read_text()))
        text = json.dumps(result, indent=1, allow_nan=False) + '\n'
        if opts.out:
            pathlib.Path(opts.out).write_text(text)
        else:
            print(text, end='')
        return 0 if result['passes'] else 1
    except (ValueError, OSError, KeyError, TypeError, subprocess.CalledProcessError):
        print('B4 input, provenance or infrastructure validation failed', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
