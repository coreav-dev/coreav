#!/usr/bin/env python3
"""Bounded, replayable acquisition for the registered H.264 PGO allocation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parent))
import h264_pgo_round as driver

SOURCE_MAX = 256 << 20
TOTAL_MAX = 14 << 30
CHUNK = 32768
META_KEYS = ('codec_name', 'pix_fmt', 'field_order', 'profile', 'width', 'height', 'frames')
POLICY = Path(__file__).with_name('h264_pgo_acquisition.json')
CAPTURE_FAILURES = ('nonzero-exit', 'stderr', 'invalid-metadata', 'empty-extraction', 'invalid-extraction')
CAPTURE_STAGES = ('source-probe', 'packet-copy', 'es-probe')
INFERENCE_SCOPE = 'capture-clean-eligible-cohort'
STREAM_DIAGNOSTIC = re.compile(
    rb'(?:\[[a-z0-9_,#.-]+ @ 0x[0-9a-f]+\][ \t]*)*'
    rb'(?:invalid nal unit size(?: \(-?[0-9]+ > [0-9]+\)\.)?|'
    rb'error splitting the input into nal units\.?|invalid data found when processing input\.?|'
    rb'missing picture in access unit with size [0-9]+|non-existing pps [0-9]+ referenced|'
    rb'decode_slice_header error|no frame!|moov atom not found)')
RESOURCE_DIAGNOSTIC = re.compile(
    rb'cannot allocate memory|out of memory|memory allocation (?:failed|error)|failed to allocate|'
    rb'no space left on device|disk quota exceeded|too many open files|resource temporarily unavailable|'
    rb'file size limit exceeded|permission denied|operation not permitted|input/output error|broken pipe|'
    rb'interrupted system call')


class Incomplete(RuntimeError):
    pass


class CaptureFailure(ValueError):
    def __init__(self, reason):
        self.reason = reason
        super().__init__(reason)


class CaptureFault(ValueError):
    def __init__(self, reason):
        if reason not in ('external-protocol', 'resource-or-infrastructure', 'capture-timeout',
                          'capture-output-limit', 'unrecognized-oracle-diagnostic', 'unexplained-nonzero-exit'):
            raise ValueError('unknown fatal capture reason')
        self.reason = reason
        super().__init__(reason)


require = driver.require


def digest(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def atomic(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.tmp')
    with temporary.open('w') as output:
        json.dump(value, output, indent=2, allow_nan=False)
        output.write('\n')
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def publish_file(temporary, final):
    with temporary.open('rb') as source:
        os.fsync(source.fileno())
    final.parent.mkdir(parents=True, exist_ok=True)
    temporary.replace(final)
    fd = os.open(final.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def check_deadline(deadline, clock):
    if clock() >= deadline:
        raise Incomplete('operator deadline exhausted')


def remaining(deadline, clock, maximum):
    check_deadline(deadline, clock)
    return min(maximum, deadline - clock())


def metadata(stdout):
    document = json.loads(stdout)
    require(isinstance(document, dict) and isinstance(document.get('streams'), list), 'ffprobe stream capture is incomplete')
    streams = document['streams']
    require(all(isinstance(s, dict) and type(s.get('index')) is int and s['index'] >= 0 for s in streams) and
            len({s['index'] for s in streams}) == len(streams), 'ffprobe stream indexes are invalid')
    choices = []
    for stream in sorted(streams, key=lambda s: s['index']):
        if stream.get('codec_type') != 'video':
            continue
        value = {k: stream.get(k) if isinstance(stream.get(k), str) else None for k in META_KEYS[:4]}
        value.update({k: stream.get(k) if type(stream.get(k)) is int and stream[k] > 0 else None for k in ('width', 'height')})
        frames = stream.get('nb_read_frames')
        value['frames'] = int(frames) if isinstance(frames, str) and frames.isascii() and frames.isdigit() and int(frames) > 0 else None
        choices.append((value, stream['index']))
    return next((v for v in choices if eligible(v[0])), choices[0] if choices else (dict.fromkeys(META_KEYS), None))


def eligible(value):
    return (value['codec_name'] == 'h264' and value['pix_fmt'] == 'yuv420p' and value['field_order'] == 'progressive' and
            value['profile'] in ('Constrained Baseline', 'Main', 'High') and
            all(type(value[k]) is int and value[k] > 0 for k in ('width', 'height', 'frames')))


def probe_command(root, source):
    return [str(root / 'ffprobe'), '-v', 'error', '-threads', '1', '-protocol_whitelist', 'file,pipe',
            '-select_streams', 'v', '-count_frames',
            '-show_entries', 'stream=index,codec_type,codec_name,pix_fmt,field_order,profile,width,height,nb_read_frames',
            '-of', 'json', str(source)]


def extraction_command(root, source, output, stream, frames):
    return [str(root / 'ffmpeg'), '-nostdin', '-v', 'error', '-y', '-threads', '1',
            '-protocol_whitelist', 'file,pipe', '-i', str(source),
            '-map', '0:' + str(stream), '-an', '-sn', '-dn', '-frames:v', str(frames), '-c:v', 'copy',
            '-bsf:v', 'h264_mp4toannexb', '-f', 'h264', str(output)]


def reason(candidate, value, source_hash, used, rows, counts, driver, plan):
    if source_hash in used['hashes'] or source_hash[:12] in used['hashes'] or source_hash in {r['source_sha256'] for r in rows}:
        return 'used-bytes'
    if set(candidate['groups']) & {g for r in rows for g in r['groups']}:
        return 'group-used'
    if not eligible(value):
        return 'unsupported'
    if counts[driver.resolution(value['width'], value['height'])] == plan['count_per_class']:
        return 'class-full'
    return None


def _original(root, row, index, state, save, opener, clock, before_fetch):
    final = root / 'operator/originals' / f'o{index:05}.source'
    final.parent.mkdir(parents=True, exist_ok=True)
    def verify():
        require(final.stat().st_size == row['size'], 'original size differs from frozen metadata')
        for name, expected in row['checksums'].items():
            with final.open('rb') as source:
                require(hashlib.file_digest(source, name).hexdigest() == expected, 'original differs from declared checksum')
        return digest(final)
    attempts = state['downloads'].setdefault(str(index), [])
    if final.exists():
        require(attempts, 'original has no download receipt')
        value = verify()
        require(attempts[-1].get('sha256', value) == value, 'retained original changed')
        attempts[-1].update(status='complete', sha256=value, received_bytes=row['size'])
        save()
        return final, value
    require(not any(a['status'] == 'complete' for a in attempts), 'checkpointed original is missing')
    check_deadline(state['deadline'], clock)
    charge = row['size'] + 1
    if sum(a['charged_bytes'] for values in state['downloads'].values() for a in values) + charge > state['byte_cap']:
        raise Incomplete('source byte cap exhausted')
    for prior in attempts:
        if prior['status'] == 'started':
            prior['status'] = 'interrupted'
    attempt = {'status': 'started', 'charged_bytes': charge, 'received_bytes': 0}
    attempts.append(attempt)
    partial = final.with_name(final.name + f'.a{len(attempts)}.part')
    save()
    try:
        before_fetch(index, len(attempts))
        with opener(row['identity'], timeout=remaining(state['deadline'], clock, 180)) as response, partial.open('wb') as output:
            require(getattr(response, 'status', 200) == 200, 'source HTTP status differs')
            while True:
                check_deadline(state['deadline'], clock)
                count = min(CHUNK, row['size'] + 1 - attempt['received_bytes'])
                block = response.read(count)
                require(isinstance(block, bytes) and len(block) <= count, 'source reader contract differs')
                if not block:
                    break
                attempt['received_bytes'] += len(block)
                output.write(block)
                if attempt['received_bytes'] > row['size']:
                    raise Incomplete('source exceeds its frozen size')
            output.flush()
            os.fsync(output.fileno())
        require(attempt['received_bytes'] == row['size'], 'source is truncated against frozen size')
        for name, expected in row['checksums'].items():
            with partial.open('rb') as source:
                require(hashlib.file_digest(source, name).hexdigest() == expected, 'source differs from declared checksum')
        publish_file(partial, final)
        value = verify()
        attempt.update(status='complete', sha256=value)
        save()
        return final, value
    except BaseException as error:
        attempt.update(status='failed', error=type(error).__name__)
        save()
        raise


def capture_failure(root, record):
    require(record['status'] in ('complete', 'failed'), 'capture has no terminal receipt')
    require((record['status'] == 'complete' and 'error' not in record) or
            (record['status'] == 'failed' and record.get('error') is not None), 'contradictory capture receipt')
    require(set(record['files']) == {'stdout', 'stderr'}, 'capture files missing')
    for key in ('stdout', 'stderr'):
        require(re.fullmatch(r'operator/captures/c[0-9]{5}\.' + key, record[key]) is not None and
                (root / record[key]).resolve().is_relative_to(root.resolve()), 'capture path escapes acquisition')
        require(digest(root / record[key]) == record['files'][key], 'capture bytes changed')
    for name, expected in record.get('artifacts', {}).items():
        require(re.fullmatch(r'operator/extractions/e[0-9]{5}\.h264', name) is not None and
                (root / name).resolve().is_relative_to(root.resolve()), 'artifact path escapes acquisition')
        require(digest(root / name) == expected, 'capture artifact changed')
    if any((root / record[key]).stat().st_size > 1 << 20 for key in ('stdout', 'stderr')):
        raise CaptureFault('capture-output-limit')
    error = (root / record['stderr']).read_bytes().lower()
    if re.search(rb'not on whitelist|protocol not found|protocol_whitelist', error):
        raise CaptureFault('external-protocol')
    if RESOURCE_DIAGNOSTIC.search(error):
        raise CaptureFault('resource-or-infrastructure')
    require(record.get('error') in (None, 'CalledProcessError', 'TimeoutExpired', 'ValueError'),
            'unknown capture exception')
    require(record.get('error') != 'ValueError' or 'root' not in record, 'unknown capture exception')
    if record.get('error') == 'TimeoutExpired':
        require('returncode' not in record, 'timeout has a conflicting exit receipt')
        raise CaptureFault('capture-timeout')
    code = record['returncode']
    require(type(code) is int and code >= 0, 'signal or resource failure is not replaceable')
    if error:
        lines = [line.strip() for line in error.splitlines() if line.strip()]
        if not lines or not all(STREAM_DIAGNOSTIC.fullmatch(line) for line in lines):
            raise CaptureFault('unrecognized-oracle-diagnostic')
        return 'nonzero-exit' if code else 'stderr'
    if code:
        raise CaptureFault('unexplained-nonzero-exit')
    require(record['status'] == 'complete' and 'error' not in record, 'unclassified capture failure')
    return None


def command_matches(root, record, argv):
    if 'root' in record:
        origin = record['root']
    else:
        initial = json.loads((root / 'preparation/state.json').read_text())
        origin = str(Path(initial['captures'][0]['argv'][0]).parent)
    def normalized(values, base):
        return ['$ROOT/' + value[len(base) + 1:] if value.startswith(base + '/') else value for value in values]
    return normalized(record['argv'], origin) == normalized(argv, str(root))


def _capture(root, state, save, kind, argv, runner, clock, replay=False):
    matches = [i for i, r in enumerate(state['captures']) if r['kind'] == kind and command_matches(root, r, argv)]
    require(len(matches) <= 1, 'duplicate capture command')
    if matches:
        capture_failure(root, state['captures'][matches[0]])
        return matches[0]
    require(not replay, 'replay lacks a retained capture')
    number = len(state['captures'])
    base = root / 'operator/captures' / f'c{number:05}'
    base.parent.mkdir(parents=True, exist_ok=True)
    record = {'kind': kind, 'argv': argv, 'root': str(root), 'status': 'started',
              'stdout': str(base.relative_to(root)) + '.stdout', 'stderr': str(base.relative_to(root)) + '.stderr'}
    state['captures'].append(record)
    save()
    try:
        with (root / record['stdout']).open('wb') as stdout, (root / record['stderr']).open('wb') as stderr:
            try:
                result = runner(argv, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                timeout=remaining(state['deadline'], clock, 180), check=False)
                record['returncode'] = result.returncode
                record['status'] = 'complete'
            except subprocess.TimeoutExpired:
                record.update(status='failed', error='TimeoutExpired')
        check_deadline(state['deadline'], clock)
    except BaseException as error:
        record.update(status='failed', error=type(error).__name__)
        raise
    finally:
        for key in ('stdout', 'stderr'):
            if (root / record[key]).exists():
                with (root / record[key]).open('rb') as captured:
                    os.fsync(captured.fileno())
        record['files'] = {k: digest(root / record[k]) for k in ('stdout', 'stderr') if (root / record[k]).exists()}
        if kind == 'packet-copy' and Path(argv[-1]).is_file():
            output = Path(argv[-1])
            with output.open('rb') as captured:
                os.fsync(captured.fileno())
            record['artifacts'] = {str(output.relative_to(root)): digest(output)}
        save()
    capture_failure(root, record)
    return number


def _captured(root, state, number, argv):
    record = state['captures'][number]
    require(command_matches(root, record, argv), 'capture command changed')
    why = capture_failure(root, record)
    if why:
        raise CaptureFailure(why)
    return (root / record['stdout']).read_bytes()


def charges(metadata_value, counts, plan):
    if metadata_value is None:
        return [c for c in driver.bars.CLASSES if counts[c] < plan['count_per_class']]
    return [driver.resolution(metadata_value['width'], metadata_value['height'])]


def failure_counts(decisions, plan):
    counts = {c: 0 for c in driver.bars.CLASSES}
    failures = dict(counts)
    for decision in decisions:
        require(not any(failures[c] >= 16 and counts[c] < plan['count_per_class'] for c in counts),
                'collection continued after replacement limit')
        if decision['capture_status'] == 'failed':
            require(decision['status'] == 'excluded' and decision['reason'] in
                    CAPTURE_FAILURES, 'unregistered capture exclusion')
            expected = charges(decision['metadata'], counts, plan)
            require(decision['failure_classes'] == expected and expected, 'failure class charge changed')
            for cls in expected:
                failures[cls] += 1
        elif decision['status'] == 'selected':
            counts[driver.resolution(decision['metadata']['width'], decision['metadata']['height'])] += 1
    return counts, failures


def public_report(state, plan):
    counts, failures = failure_counts(state['decisions'], plan)
    classes = driver.bars.CLASSES
    pending = {cls: [] for cls in classes}
    cells = {cls: {} for cls in classes}
    charges_by_class = {cls: {'known': 0, 'unknown': 0} for cls in classes}
    replacements = {cls: {'known': 0, 'unknown': 0} for cls in classes}
    sources = {'known': 0, 'unknown': 0}
    source_reasons = {}
    for decision in state['decisions']:
        if decision['capture_status'] == 'failed':
            kind = 'unknown' if decision['metadata'] is None else 'known'
            reason, stage = decision['reason'], decision['failure_stage']
            require(stage in CAPTURE_STAGES, 'unregistered capture stage')
            key = kind, reason, stage
            sources[kind] += 1
            source_reasons[key] = source_reasons.get(key, 0) + 1
            for cls in decision['failure_classes']:
                charges_by_class[cls][kind] += 1
                pending[cls].append(key)
                cell = cells[cls].setdefault(key, {'failure_charges': 0, 'replacements_selected': 0})
                cell['failure_charges'] += 1
        elif decision['status'] == 'selected':
            value = decision['metadata']
            cls = driver.resolution(value['width'], value['height'])
            if pending[cls]:
                key = pending[cls].pop(0)
                replacements[cls][key[0]] += 1
                cells[cls][key]['replacements_selected'] += 1
    stop = state.get('stop_reason')
    require(stop in (None, 'external-protocol', 'resource-or-infrastructure', 'capture-timeout',
                     'capture-output-limit', 'unrecognized-oracle-diagnostic', 'unexplained-nonzero-exit',
                     'operational-limit', 'evidence-or-infrastructure'), 'unregistered stop reason')
    require(state['status'] != 'complete' or stop is None, 'completed acquisition has a terminal fault')
    return {'schema': 1, 'status': 'complete' if state['status'] == 'complete' else 'incomplete',
            'inference_scope': INFERENCE_SCOPE, 'failure_limit_per_class': 16, 'stop_reason': stop,
            'failed_sources': sources,
            'source_failure_reasons': [
                {'classification': kind, 'reason': reason, 'stage': stage, 'sources': number}
                for (kind, reason, stage), number in sorted(source_reasons.items())],
            'per_class': {cls: {
                'selected': counts[cls], 'failure_charges': charges_by_class[cls],
                'replacements_selected': replacements[cls],
                'unmatched_failure_charges': {kind: charges_by_class[cls][kind] - replacements[cls][kind]
                                              for kind in ('known', 'unknown')},
                'failure_limit_reached': failures[cls] >= 16 and counts[cls] < plan['count_per_class'],
                'reasons': [{'classification': kind, 'reason': reason, 'stage': stage, **cell}
                            for (kind, reason, stage), cell in sorted(cells[cls].items())]}
                for cls in classes}}



def prepare(root, index, candidate, original, source_hash, state, save, used, rows, counts,
            plan, runner, clock, replay=False):
    item = state['items'].setdefault(str(index), {})
    value = None
    stage = 'source-probe'
    def captured(key, kind, argv):
        item[key] = _capture(root, state, save, kind, argv, runner, clock, replay)
        save()
        return _captured(root, state, item[key], argv)
    def parsed(raw):
        try:
            return metadata(raw)
        except (ValueError, TypeError, KeyError, UnicodeError):
            raise CaptureFailure('invalid-metadata') from None
    decision = {'identity': candidate['identity'], 'source_sha256': source_hash}
    try:
        value, stream = parsed(captured('probe', stage, probe_command(root, original)))
        why = reason(candidate, value, source_hash, used, rows, counts, driver, plan)
        decision.update(metadata=value, capture_status='ok', status='excluded' if why else 'selected', reason=why)
        if why:
            return decision, None
        cls = driver.resolution(value['width'], value['height'])
        label = f'h{len(rows):03}'
        es = root / 'media' / (label + '.h264')
        legacy = index < len(json.loads((root / 'preparation/collection.json').read_text()))
        output = es if legacy else root / 'operator/extractions' / f'e{index:05}.h264'
        output.parent.mkdir(parents=True, exist_ok=True)
        partial = es.with_suffix('.part') if legacy else output
        stage = 'packet-copy'
        captured('extraction', stage, extraction_command(root, original, partial, stream, plan['frames'][cls]))
        output = es if legacy else output
        if not output.is_file() or output.stat().st_size == 0:
            raise CaptureFailure('empty-extraction')
        stage = 'es-probe'
        es_meta, _ = parsed(captured('es_probe', stage, probe_command(root, output)))
        if not (eligible(es_meta) and all(es_meta[k] == value[k] for k in META_KEYS if k != 'frames') and
                es_meta['frames'] == min(value['frames'], plan['frames'][cls])):
            raise CaptureFailure('invalid-extraction')
        es_hash = digest(output)
        require(es_hash not in used['hashes'] and es_hash[:12] not in used['hashes'] and
                es_hash not in {r['sha256'] for r in rows}, 'extracted bytes overlap')
        if not legacy and not replay:
            es.parent.mkdir(parents=True, exist_ok=True)
            temporary = es.with_suffix('.part')
            shutil.copyfile(output, temporary)
            publish_file(temporary, es)
        require(digest(es) == es_hash, 'selected elementary stream changed')
        selected = {'label': label, 'identity': candidate['identity'], 'groups': candidate['groups'],
                    'source_sha256': source_hash, 'sha256': es_hash, 'class': cls,
                    'width': value['width'], 'height': value['height'], 'frames': es_meta['frames'],
                    'source_frames': value['frames'], 'stream_index': stream}
        return decision, selected
    except CaptureFailure as error:
        decision.update(metadata=value, capture_status='failed', status='excluded', reason=error.reason,
                        failure_stage=stage, failure_classes=charges(value, counts, plan))
        return decision, None


def policy():
    config = json.loads(POLICY.read_text())
    require(config['schema'] == 1 and config['plan_sha256'] == digest(driver.PLAN) and
            config['replacement_limit_per_class'] == 16 and config['source_attempt_bytes'] == TOTAL_MAX and
            config['continuation_seconds'] == 6600, 'acquisition registration changed')
    return config


def verify_receipt(root, expected, tooling_revision=None):
    config = policy()
    require(expected is not None and digest(root / 'acquisition-receipt.json') == expected,
            'acquisition receipt differs from separately recorded digest')
    receipt = json.loads((root / 'acquisition-receipt.json').read_text())
    require(set(receipt) == {'schema', 'policy', 'allocation', 'tooling_revision', 'deadline'} and
            receipt['schema'] == 1 and receipt['policy'] == digest(POLICY) and
            receipt['allocation'] == config['allocation_sha256'] and
            re.fullmatch('[0-9a-f]{40}', receipt['tooling_revision']) and
            type(receipt['deadline']) in (int, float) and 0 < receipt['deadline'] < float('inf'),
            'invalid acquisition receipt')
    require(tooling_revision is None or receipt['tooling_revision'] == tooling_revision, 'acquisition tooling changed')
    for name, sha256 in config['preparation'].items():
        require(digest(root / ('preparation/' + name)) == sha256, 'stopped preparation changed')
    require(digest(root / 'operator/acquisition.json') == config['source_inventory_sha256'],
            'source inventory changed')
    return receipt


def freeze(root):
    require(sys.platform == 'linux', 'acquisition requires the isolated Linux host')
    config = policy()
    revision = driver.merged()
    driver.verify_allocation(root, json.loads(driver.PLAN.read_text()), config['allocation_sha256'],
                             config['artifact_revision'])
    require(not (root / 'events.jsonl').exists(), 'evaluation already started')
    for name, sha256 in config['preparation'].items():
        require(digest(root / ('preparation/' + name)) == sha256, 'stopped preparation changed')
    require(digest(root / 'operator/acquisition.json') == config['source_inventory_sha256'], 'source inventory changed')
    value = {'schema': 1, 'policy': digest(POLICY), 'allocation': config['allocation_sha256'],
             'tooling_revision': revision, 'deadline': time.time() + 6600}
    with (root / 'acquisition-receipt.json').open('x') as output:
        json.dump(value, output, indent=2, allow_nan=False)
        output.flush()
        os.fsync(output.fileno())
    return digest(root / 'acquisition-receipt.json')


def replay(root, state, candidates, plan, runner, clock):
    used = driver.exclusions(json.loads((root / 'exclusions.json').read_text()))
    rows, decisions = [], []
    require(len(state['decisions']) <= len(candidates), 'collection extends frozen allocation')
    for index, recorded in enumerate(state['decisions']):
        counts, _ = failure_counts(decisions, plan)
        original = root / 'operator/originals' / f'o{index:05}.source'
        require(digest(original) == recorded['source_sha256'], 'retained original changed')
        decision, selected = prepare(root, index, candidates[index], original, recorded['source_sha256'],
                                     state, lambda: None, used, rows, counts, plan, runner, clock, True)
        require(decision == recorded, 'retained acquisition decision changed')
        decisions.append(decision)
        if selected:
            rows.append(selected)
    require(rows == state['rows'], 'retained roster changed')
    failure_counts(decisions, plan)
    return rows


def bounded_runner(argv, **kwargs):
    import resource
    def limits():
        resource.setrlimit(resource.RLIMIT_AS, (8 << 30, 8 << 30))
        resource.setrlimit(resource.RLIMIT_FSIZE, (600 << 20, 600 << 20))
        resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    env = {key: os.environ[key] for key in ('PATH', 'HOME') if key in os.environ}
    env.update(LANG='C', LC_ALL='C')
    return subprocess.run(argv, env=env, preexec_fn=limits, **kwargs)


def verify_prefix(root, state):
    initial = json.loads((root / 'preparation/state.json').read_text())
    require(initial['decisions'] == json.loads((root / 'preparation/collection.json').read_text()) and
            initial['rows'] == json.loads((root / 'preparation/roster.json').read_text()), 'preparation prefix differs')
    require(state['decisions'][:len(initial['decisions'])] == initial['decisions'] and
            state['rows'][:len(initial['rows'])] == initial['rows'] and
            state['captures'][:len(initial['captures'])] == initial['captures'], 'preparation prefix changed')
    for key, attempts in initial['downloads'].items():
        require(state['downloads'][key][:len(attempts)] == attempts, 'prior source attempts changed')


def _acquire(root, allocation_sha256, acquisition_sha256, *, opener, checkpoint, clock=time.time):
    root = Path(root)
    require(sys.platform == 'linux', 'acquisition requires the isolated Linux host')
    require(not (root / 'events.jsonl').exists(), 'evaluation started; replacement forbidden')
    receipt = verify_receipt(root, acquisition_sha256, driver.merged())
    config = policy()
    require(allocation_sha256 == config['allocation_sha256'], 'original allocation changed')
    plan = json.loads(driver.PLAN.read_text())
    candidates = driver.verify_allocation(root, plan, allocation_sha256, config['artifact_revision'])
    inventory = json.loads((root / 'operator/acquisition.json').read_text())
    require(inventory['population_sha256'] == digest(root / 'population.json'), 'source population changed')
    path = root / 'operator/amended-state.json'
    if path.exists():
        state = json.loads(path.read_text())
        require(state['acquisition_sha256'] == acquisition_sha256 and state['deadline'] == receipt['deadline'] and
                state['byte_cap'] == TOTAL_MAX, 'acquisition binding or limits changed')
    else:
        state = json.loads((root / 'preparation/state.json').read_text())
        require(state['decisions'] == json.loads((root / 'preparation/collection.json').read_text()) and
                state['rows'] == json.loads((root / 'preparation/roster.json').read_text()), 'preparation prefix differs')
        state.update(acquisition_sha256=acquisition_sha256, deadline=receipt['deadline'], byte_cap=TOTAL_MAX,
                     status='incomplete')
    save = lambda: atomic(path, state)
    verify_prefix(root, state)
    save()
    require(sum(a['charged_bytes'] for attempts in state['downloads'].values() for a in attempts) <= TOTAL_MAX,
            'source-attempt cap exceeded')
    replay(root, state, candidates, plan, None, clock)
    atomic(root / 'collection.json', state['decisions'])
    atomic(root / 'roster.json', state['rows'])
    atomic(root / 'acquisition-report.json', public_report(state, plan))
    import oracle_pin
    require(not list(oracle_pin.verify_bin(root)), 'oracle installation differs')
    def notify(kind, index, paths):
        key = f'amended:{kind}:{index}'
        if key in state['notifications']:
            return
        checkpoint({'key': key, 'kind': kind, 'index': index}, tuple(paths) + (path,))
        state['notifications'].append(key)
        save()
    for index in range(len(state['decisions'])):
        notify('decision', index, evidence_paths(root, state, index))
    if state['status'] == 'complete':
        counts, _ = failure_counts(state['decisions'], plan)
        require(all(n == plan['count_per_class'] for n in counts.values()), 'completed acquisition lacks quota')
        return state
    try:
        for index in range(len(state['decisions']), len(candidates)):
            counts, failures = failure_counts(state['decisions'], plan)
            if all(n == plan['count_per_class'] for n in counts.values()):
                break
            if any(failures[c] >= 16 and counts[c] < plan['count_per_class'] for c in counts):
                raise Incomplete('replacement limit exhausted')
            check_deadline(state['deadline'], clock)
            candidate = candidates[index]
            row = inventory['files'][candidate['identity']]
            require(row['identity'] == candidate['identity'] and row['groups'] == candidate['groups'] and
                    type(row['size']) is int and 0 < row['size'] <= SOURCE_MAX, 'source inventory row differs')
            original, source_hash = _original(root, row, index, state, save, opener, clock,
                lambda i, n: checkpoint({'key': f'download-attempt:{i}:{n}', 'kind': 'download-attempt',
                                        'index': i, 'charged_bytes': row['size'] + 1}, (path,)))
            notify('original', index, [original])
            decision, selected = prepare(root, index, candidate, original, source_hash, state, save,
                driver.exclusions(json.loads((root / 'exclusions.json').read_text())), state['rows'], counts,
                plan, bounded_runner, clock)
            state['decisions'].append(decision)
            if selected:
                state['rows'].append(selected)
            save()
            atomic(root / 'collection.json', state['decisions'])
            atomic(root / 'roster.json', state['rows'])
            atomic(root / 'acquisition-report.json', public_report(state, plan))
            notify('decision', index, evidence_paths(root, state, index))
        counts, _ = failure_counts(state['decisions'], plan)
        require(all(n == plan['count_per_class'] for n in counts.values()), 'population exhausted')
        driver.roster(state['rows'], json.loads((root / 'exclusions.json').read_text()), plan)
        check_deadline(state['deadline'], clock)
        state.pop('error', None)
        state.pop('stop_reason', None)
        state.update(status='complete', completed_at=clock())
        save()
        atomic(root / 'acquisition-report.json', public_report(state, plan))
        checkpoint({'key': 'acquisition-complete', 'kind': 'complete'},
                   (path, root / 'acquisition-report.json'))
        return state
    except BaseException as error:
        stop = error.reason if isinstance(error, CaptureFault) else (
            'operational-limit' if isinstance(error, Incomplete) else 'evidence-or-infrastructure')
        state.update(status='incomplete', error=type(error).__name__, stop_reason=stop)
        save()
        atomic(root / 'acquisition-report.json', public_report(state, plan))
        checkpoint({'key': 'acquisition-stop', 'kind': 'stop'}, (path, root / 'acquisition-report.json'))
        raise


def acquire(root, allocation_sha256, acquisition_sha256, *, opener, checkpoint, clock=time.time):
    import fcntl
    root = Path(root).resolve()
    require(sys.platform == 'linux', 'acquisition requires the isolated Linux host')
    with (root / 'operator/amended.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        return _acquire(root, allocation_sha256, acquisition_sha256, opener=opener, checkpoint=checkpoint, clock=clock)


def evidence_paths(root, state, index):
    paths = [root / 'operator/originals' / f'o{index:05}.source', root / 'collection.json', root / 'roster.json',
             root / 'acquisition-report.json']
    for number in state['items'].get(str(index), {}).values():
        paths.extend(root / state['captures'][number][k] for k in ('stdout', 'stderr'))
        paths.extend(root / name for name in state['captures'][number].get('artifacts', {}))
    selected = next((r for r in state['rows'] if r['identity'] == state['decisions'][index]['identity']), None)
    if selected:
        paths.append(root / 'media' / (selected['label'] + '.h264'))
    return paths


def audit_collection(root, candidates, plan, expected):
    receipt = verify_receipt(root, expected)
    state = json.loads((root / 'operator/amended-state.json').read_text())
    require(state['acquisition_sha256'] == expected and state['status'] == 'complete' and
            state['byte_cap'] == TOTAL_MAX and state['deadline'] == receipt['deadline'] and
            type(state['completed_at']) in (int, float) and 0 < state['completed_at'] <= receipt['deadline'],
            'acquisition did not complete')
    verify_prefix(root, state)
    inventory = json.loads((root / 'operator/acquisition.json').read_text())
    require(set(state['downloads']) == {str(i) for i in range(len(state['decisions']))}, 'source attempt roster differs')
    for index, decision in enumerate(state['decisions']):
        row = inventory['files'][candidates[index]['identity']]
        attempts = state['downloads'][str(index)]
        original = root / 'operator/originals' / f'o{index:05}.source'
        require(attempts and all(a['charged_bytes'] == row['size'] + 1 for a in attempts) and
                attempts[-1]['status'] == 'complete' and attempts[-1]['sha256'] == decision['source_sha256'] and
                original.stat().st_size == row['size'], 'source attempt accounting differs')
        for name, checksum in row['checksums'].items():
            with original.open('rb') as source:
                require(hashlib.file_digest(source, name).hexdigest() == checksum, 'original checksum differs')
    require(state['decisions'] == json.loads((root / 'collection.json').read_text()) and
            state['rows'] == json.loads((root / 'roster.json').read_text()), 'collection differs from state')
    require(set(state['items']) == {str(i) for i in range(len(state['decisions']))}, 'capture item roster differs')
    indexes = []
    for index, decision in enumerate(state['decisions']):
        item = state['items'][str(index)]
        require(set(item) <= {'probe', 'extraction', 'es_probe'} and 'probe' in item, 'capture stages differ')
        indexes.extend(item.values())
        require(f'amended:decision:{index}' in state['notifications'], 'decision checkpoint was not acknowledged')
    require(all(type(i) is int for i in indexes) and len(indexes) == len(set(indexes)) and
            set(indexes) == set(range(len(state['captures']))), 'incomplete capture roster')
    require(sum(a['charged_bytes'] for attempts in state['downloads'].values() for a in attempts) <= TOTAL_MAX,
            'source-attempt cap exceeded')
    replay(root, state, candidates, plan, None, time.time)
    require(json.loads((root / 'acquisition-report.json').read_text()) == public_report(state, plan),
            'public acquisition accounting differs from replay')
    return state


def bound_files(root, state):
    names = {'acquisition-receipt.json', 'operator/amended-state.json', 'operator/acquisition.json',
             'collection.json', 'roster.json', 'acquisition-report.json'}
    names.update('preparation/' + name for name in policy()['preparation'])
    for index in range(len(state['decisions'])):
        names.update(str(path.relative_to(root)) for path in evidence_paths(root, state, index))
    return sorted(names)


def tool_files():
    return ['scripts/h264_pgo_acquire.py', 'scripts/h264_pgo_acquisition.json', 'scripts/h264_pgo_round.py']


def bind(root, expected):
    state = json.loads((root / 'operator/amended-state.json').read_text())
    value = {'schema': 1, 'acquisition': expected,
             'tools': {name: digest(driver.ROOT / name) for name in tool_files()},
             'files': {name: digest(root / name) for name in bound_files(root, state)}}
    with (root / 'acquisition-binding.json').open('x') as output:
        json.dump(value, output, indent=2, allow_nan=False)


def audit_bound(root, plan, allocation_sha256, expected):
    require(allocation_sha256 == policy()['allocation_sha256'], 'allocation changed')
    candidates = driver.verify_allocation(root, plan, allocation_sha256, policy()['artifact_revision'])
    state = audit_collection(root, candidates, plan, expected)
    binding = json.loads((root / 'acquisition-binding.json').read_text())
    require(set(binding) == {'schema', 'acquisition', 'tools', 'files'} and binding['schema'] == 1 and
            binding['acquisition'] == expected and set(binding['tools']) == set(tool_files()) and
            set(binding['files']) == set(bound_files(root, state)), 'incomplete acquisition binding')
    require(all(digest(driver.ROOT / name) == sha256 for name, sha256 in binding['tools'].items()) and
            all(digest(root / name) == sha256 for name, sha256 in binding['files'].items()),
            'acquisition binding changed')
    return public_report(state, plan)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('freeze',))
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    try:
        print(freeze(args.directory.resolve()))
        return 0
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        print('Acquisition receipt incomplete; retain private preparation evidence', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
