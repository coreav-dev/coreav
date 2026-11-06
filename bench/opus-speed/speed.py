#!/usr/bin/env python3
"""Execute one registered Opus shard without resuming judged work."""
import argparse
import contextlib
import fcntl
import hashlib
import importlib.metadata
import json
import math
import os
import pathlib
import re
import statistics
import sys
import tempfile
import time
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
sys.path.insert(0, str(ROOT / 'bench/audio-decode'))
import capacity
import oracle
import receipts
import run as capacity_run
import allowance
sys.path.insert(0, str(ROOT / 'scripts'))
import test_oracle

CLASSES = ('mp4', 'matroska')
GATES = ('raw_identity', 'cli_identity', 'oracle')
ENGINES = ('head_lib', 'base_lib', 'lav_lib', 'head_dec', 'base_dec', 'lav_dec', 'head_cli', 'ffmpeg_cli')
COMPLETE = ('timed', 'oracle_disagreement', 'derivation_failed')
DERIVATION_FAILURES = ('flac_encode_failed', 'no_audio_stream', 'encode_failed')


class Failure(Exception):
    def __init__(self, code):
        allowed = {'invalid_plan', 'invalid_capacity_reference', 'changed_capacity_reference', 'unqualified_limits',
            'invalid_draw_plan', 'invalid_judgment_plan', 'invalid_roster', 'short_or_duplicate_roster',
            'invalid_shard_coverage', 'invalid_packet_roster', 'invalid_exclusions', 'spent_source',
            'unexpected_or_duplicate_input', 'missing_derivation_outcome', 'invalid_or_spent_prepared_input',
            'omitted_input', 'changed_qualified_runtime', 'host_not_registered_on_demand',
            'missing_checkpoint_credentials', 'incomplete_timed_work', 'changed_input_bytes',
            'invalid_gate_work', 'identity_mismatch', 'inconsistent_gate_work', 'invalid_completed_record',
            'changed_registered_inputs', 'invalid_allowance', 'roster_exceeds_allowance'}
        self.code = code if isinstance(code, str) and code in allowed else 'internal_error'
        super().__init__(self.code)


def error_info(error):
    return {'reason': error.code if isinstance(error, Failure) else None,
            'error': (receipts.receipt('capacity.stage', 'harness', 'protocol_error') if isinstance(error, Failure)
                      else receipts.exception_receipt(error))}


def valid_sha(value, length=64):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{' + str(length) + '}', value) is not None


def load_plan(path):
    plan = capacity.load(path)
    fields = {'schema', 'round', 'provenance_commit', 'exclusions_sha256', 'capacity',
              'base_commit', 'head_commit', 'draw', 'engines', 'warmups', 'rounds', 'bootstrap',
              'bars', 'minimums', 'host', 'call_limits', 'shards', 'shard_seconds', 'vm_seconds', 'allowance'}
    if (not isinstance(plan, dict) or set(plan) != fields or plan['schema'] != 'opus-speed-plan/1'
            or plan['round'] != 4 or plan['engines'] != list(ENGINES)
            or plan['warmups'] != 1 or plan['rounds'] != 5 or plan['shards'] != 4
            or plan['bootstrap'] != {'seed': 206, 'resamples': 10000}
            or not all(valid_sha(plan[k], 40) for k in ('base_commit', 'head_commit', 'provenance_commit'))
            or not valid_sha(plan['exclusions_sha256'])):
        raise Failure('invalid_plan')
    refs = plan['capacity']
    if (not isinstance(refs, dict) or set(refs) != {'receipt', 'receipt_sha256', 'binding', 'binding_sha256'}
            or not all(valid_sha(refs[k]) for k in ('receipt_sha256', 'binding_sha256'))):
        raise Failure('invalid_capacity_reference')
    paths = [(ROOT / refs[k]).resolve() for k in ('receipt', 'binding')]
    if (any(ROOT not in path.parents for path in paths)
            or [capacity.digest(path) for path in paths] != [refs['receipt_sha256'], refs['binding_sha256']]):
        raise Failure('changed_capacity_reference')
    binding = capacity.load(paths[1])
    receipt = capacity.load(paths[0])
    capacity.validate_capacity(receipt, binding)
    limits = plan['call_limits']
    if (limits != {'seconds': binding['limits']['per_call_seconds'],
                   'address_space_bytes': binding['limits']['memory_bytes'],
                   'temporary_output_bytes': binding['limits']['tmpfs_bytes']}
            or not capacity.positive(plan['shard_seconds']) or not capacity.positive(plan['vm_seconds'])
            or plan['shard_seconds'] >= plan['vm_seconds']):
        raise Failure('unqualified_limits')
    draw = plan['draw']
    if (not isinstance(draw, dict) or set(draw) != {'number', 'salt', 'minimum_sources', 'target_sources',
            'class_maximum', 'source_maximum', 'group_maximum', 'bytes_maximum', 'seconds_minimum',
            'seconds_maximum', 'exclude_through_draw'} or draw['number'] != 10
            or draw['exclude_through_draw'] != 9 or draw['salt'] != 'coreav-corpus-v2-heldout-audio-decode-4:'
            or any(not capacity.positive(value) for key, value in draw.items() if key != 'salt')
            or not 100 <= draw['minimum_sources'] <= draw['target_sources'] <= 160
            or draw['seconds_minimum'] > draw['seconds_maximum']):
        raise Failure('invalid_draw_plan')
    if (set(plan['bars']) != {'celt_decoder_lav_below', 'silk_decoder_base_at_most',
            'hybrid_decoder_base_at_most', 'library_lav_at_most'}
            or any(type(value) not in (int, float) or not math.isfinite(value) or value <= 0
                   for value in plan['bars'].values())
            or set(plan['minimums']) != {'celt', 'celt_per_class', 'silk', 'hybrid'}
            or any(not capacity.positive(value) for value in plan['minimums'].values())
            or plan['host'] != {'cpu': 'Intel Xeon Platinum 8581C', 'smt': False,
                                'machine': 'c4-standard-4', 'provisioning': 'STANDARD', 'affinity_cpu': 1}):
        raise Failure('invalid_judgment_plan')
    ref = plan['allowance']
    try:
        if not isinstance(ref, dict) or set(ref) != {'path', 'sha256'} or not valid_sha(ref['sha256']):
            raise ValueError
        path = (ROOT / ref['path']).resolve()
        if ROOT not in path.parents or capacity.digest(path) != ref['sha256']:
            raise ValueError
        previous = capacity.load(ROOT / 'bench/audio-decode/evidence/opus-round4-binding.json')
        plan['_allowance'] = allowance.prepare(capacity.load(path), binding, receipt, previous)
    except (ValueError, KeyError, TypeError, AttributeError, capacity.Invalid):
        raise Failure('invalid_allowance') from None
    return plan, binding


def roster(data, plan):
    if (not isinstance(data, dict) or set(data) != {'draw', 'sources', 'shards', 'inputs_sha256', 'exclusions_sha256'}
            or data['draw'] != plan['draw']['number'] or not valid_sha(data['inputs_sha256'])
            or data['exclusions_sha256'] != plan['exclusions_sha256']):
        raise Failure('invalid_roster')
    sources, shards = data['sources'], data['shards']
    if (not isinstance(sources, list) or not plan['draw']['minimum_sources'] <= len(sources) <= plan['draw']['target_sources']
            or not all(valid_sha(value) for value in sources) or len(set(sources)) != len(sources)
            or not isinstance(shards, dict) or set(shards) != {str(i) for i in range(plan['shards'])}
            or any(not isinstance(part, list) or not part for part in shards.values())):
        raise Failure('short_or_duplicate_roster')
    flat = [value for part in shards.values() for value in part]
    if len(flat) != len(sources) or set(flat) != set(sources):
        raise Failure('invalid_shard_coverage')
    return sources, shards


def key(record):
    return record['source_sha256'], record['class']


def classification(record):
    modes, frames = {}, {}
    allowed = {'celt': (2.5, 5, 10, 20), 'hybrid': (10, 20), 'silk': (10, 20, 40, 60)}
    if not isinstance(record['packets'], dict) or not record['packets']:
        raise Failure('invalid_packet_roster')
    for label, count in record['packets'].items():
        mode, frame = label.split('/')
        if mode not in allowed or float(frame) not in allowed[mode] or not capacity.positive(count):
            raise Failure('invalid_packet_roster')
        modes[mode] = modes.get(mode, 0) + count
        frames[label] = count
    mode = min(modes, key=lambda value: (-modes[value], value))
    frame = min((label for label in frames if label.startswith(mode + '/')), key=lambda label: (-frames[label], label))
    return mode, frame.split('/')[1]


def input_records(document, expected, exclusions, plan):
    sources, _ = roster(expected, plan)
    if (not isinstance(exclusions, dict) or exclusions.get('schema') != 'opus-speed-exclusions/1'
            or exclusions.get('through_draw') != plan['draw']['exclude_through_draw']
            or not isinstance(exclusions.get('hashes'), list)
            or not all(valid_sha(value) for value in exclusions['hashes'])):
        raise Failure('invalid_exclusions')
    known = set(exclusions['hashes'])
    if set(sources) & known:
        raise Failure('spent_source')
    wanted = {(source, cls) for source in sources for cls in CLASSES}
    found = {}
    for record in document['inputs']:
        identity = key(record)
        if identity not in wanted or identity in found or record.get('set') != 'derived':
            raise Failure('unexpected_or_duplicate_input')
        if record.get('status') not in ('ok', *DERIVATION_FAILURES):
            raise Failure('missing_derivation_outcome')
        if record['status'] == 'ok':
            if (not valid_sha(record.get('sha256')) or record['sha256'] in known
                    or type(record.get('channels')) is not int or record['channels'] not in (1, 2)
                    or record.get('kbps') not in (12, 24, 32, 64, 128, 256)
                    or record.get('frame_ms') not in (2.5, 5, 10, 20, 40, 60)
                    or not capacity.positive(record.get('raw_frames')) or not capacity.positive(record.get('file_bytes'))
                    or not isinstance(record.get('file'), str)):
                raise Failure('invalid_or_spent_prepared_input')
            classification(record)
        found[identity] = record
    if set(found) != wanted:
        raise Failure('omitted_input')
    try:
        allowance.admit(found, expected, plan)
    except (KeyError, TypeError, ValueError):
        raise Failure('roster_exceeds_allowance') from None
    return found


def runtime_matches(binding, tools):
    if (set(tools) != set(capacity.TOOLS) | {'lav_archive'}
            or {role: capacity.digest(tools[role]) for role in capacity.TOOLS} != binding['tools']
            or capacity.verify_tools(ROOT, tools) != binding['oracle']
            or capacity.judging(ROOT) != binding['judging']
            or {'python': f'{sys.version_info.major}.{sys.version_info.minor}',
                'numpy': importlib.metadata.version('numpy')} != binding['runtime']):
        raise Failure('changed_qualified_runtime')


def metadata(name):
    request = urllib.request.Request('http://metadata.google.internal/computeMetadata/v1/instance/' + name,
                                     headers={'Metadata-Flavor': 'Google'})
    with urllib.request.urlopen(request, timeout=5) as response:
        return response.read(4096).decode().strip()


def host_check(work, binding, plan):
    host = capacity_run.host_setup(work, binding['limits'])
    machine = metadata('machine-type').rsplit('/', 1)[-1]
    if metadata('scheduling/preemptible').lower() != 'false' or machine != plan['host']['machine']:
        raise Failure('host_not_registered_on_demand')
    os.sched_setaffinity(0, {plan['host']['affinity_cpu']})
    if os.sched_getaffinity(0) != {plan['host']['affinity_cpu']}:
        raise Failure('host_not_registered_on_demand')
    return {**host, 'machine': machine, 'provisioning': 'STANDARD', 'affinity_cpu': plan['host']['affinity_cpu']}


class Store:
    def __init__(self, directory, plan, shard, registration, client=None):
        self.directory = pathlib.Path(directory)
        self.directory.mkdir(parents=True, exist_ok=False)
        if client is None:
            import boto3
            from botocore.config import Config
            names = ('R2_ENDPOINT', 'R2_BUCKET', 'R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY')
            if not all(os.environ.get(name) for name in names):
                raise Failure('missing_checkpoint_credentials')
            client = boto3.client('s3', endpoint_url=os.environ['R2_ENDPOINT'],
                aws_access_key_id=os.environ['R2_ACCESS_KEY_ID'], aws_secret_access_key=os.environ['R2_SECRET_ACCESS_KEY'],
                config=Config(connect_timeout=10, read_timeout=30, retries={'max_attempts': 2}))
        self.client, self.bucket = client, os.environ.get('R2_BUCKET')
        self.prefix = f'measurements/opus-speed/round-{plan["round"]}/shard-{shard}/'
        raw = json.dumps(registration, sort_keys=True, allow_nan=False).encode()
        self.client.put_object(Bucket=self.bucket, Key=self.prefix + 'STARTED.json', Body=raw, IfNoneMatch='*')
        with (self.directory / 'STARTED.json').open('x') as target:
            target.write(raw.decode() + '\n')

    def write(self, value):
        receipts.atomic_write(self.directory / 'results.json', value)
        self.client.put_object(Bucket=self.bucket, Key=self.prefix + 'results.json',
            Body=json.dumps(value, separators=(',', ':'), allow_nan=False).encode())


def blank(record):
    if record.get('status') not in ('ok', *DERIVATION_FAILURES):
        raise Failure('missing_derivation_outcome')
    result = {'source_sha256': record['source_sha256'], 'class': record['class'], 'status': 'pending',
              'gates': {name: {'status': 'not_run', 'result': None, 'error': None} for name in GATES},
              'warmup': {}, 'rounds': [], 'ns': {}, 'failure': None}
    if record['status'] != 'ok':
        result['status'] = 'derivation_failed'
        return result
    result.update({name: record[name] for name in ('sha256', 'channels', 'kbps', 'frame_ms', 'packets', 'raw_frames', 'file_bytes')})
    result['mode'], result['toc_frame_ms'] = classification(record)
    return result


def gate_valid(record):
    gates = record['gates']
    if set(gates) != set(GATES):
        return False
    for name in GATES:
        slot = gates[name]
        if (set(slot) != {'status', 'result', 'error'} or slot['status'] != 'complete'
                or slot['error'] is not None or not capacity.result_valid(name, slot['result'])):
            return False
    raw, cli, judged = (gates[name]['result'] for name in GATES)
    detail = judged['detail']
    samples = detail['samples']
    if record['class'] == 'mp4':
        if not {'applied', 'model', 'ffmpeg_expected', 'full', 'entries_count'} <= set(detail):
            return False
        lengths = samples['ours'] == detail['model'] and samples['ref'] == detail['ffmpeg_expected']
        default_length = samples['default'] == detail['ffmpeg_expected']
    else:
        lengths = samples['ours'] == samples['ref']
        default_length = samples['default'] == samples['ref']
    return (raw['passed'] and cli['passed'] and raw['packets'] == sum(record['packets'].values())
            and raw['frames'] == record['raw_frames']
            and raw['channels'] == cli['channels'] == record['channels']
            and cli['output_bytes'] >= cli['frames'] * record['channels'] * 2 + 44
            and samples['ours'] == cli['frames'] and detail['length_ok'] == lengths
            and detail['default_length_ok'] == default_length)


def call_valid(name, value, record):
    if (not isinstance(value, dict) or value.get('ok') is not True
            or not capacity.positive(value.get('ns')) or not gate_valid(record)):
        return False
    raw = record['gates']['raw_identity']['result']
    cli = record['gates']['cli_identity']['result']
    if name.endswith('_cli'):
        if set(value) != {'ok', 'ns', 'frames', 'channels', 'sample_rate', 'sample_width', 'output_bytes'}:
            return False
        fields = {key: value.get(key) for key in ('frames', 'channels', 'sample_rate', 'sample_width', 'output_bytes')}
        if not capacity.result_valid('ffmpeg_cli', fields) or fields['channels'] != record['channels']:
            return False
        tolerance = 0 if name == 'head_cli' else 5760
        return (abs(fields['frames'] - cli['frames']) <= tolerance
                and fields['output_bytes'] >= fields['frames'] * record['channels'] * 2 + 44)
    fields = {'samples'} | ({'bytes'} if name.endswith('_lib') else set())
    if name.startswith('lav_'):
        fields |= {'errors'} | ({'packets'} if name == 'lav_dec' else set())
    work = {key: value.get(key) for key in fields}
    expected = fields | {'ok', 'ns'}
    if name in ('head_lib', 'base_lib'):
        expected |= {'warnings'}
        if type(value.get('warnings')) is not int or not 0 <= value['warnings'] <= receipts.COUNT_LIMIT:
            return False
    if set(value) != expected or not capacity.result_valid(name, work):
        return False
    target = raw['frames'] if name.endswith('_dec') else cli['frames']
    tolerance = 5760 if name.startswith('lav_') else 0
    return (abs(work['samples'] - target) <= tolerance
            and (not name.endswith('_lib') or work['bytes'] == work['samples'] * record['channels'] * 2)
            and (name != 'lav_dec' or work['packets'] == sum(record['packets'].values())))


def record_valid(record, plan):
    try:
        fields = {'source_sha256', 'class', 'status', 'gates', 'warmup', 'rounds', 'ns', 'failure'}
        if not isinstance(record, dict) or record['failure'] is not None:
            return False
        if record['status'] == 'derivation_failed':
            return (set(record) == fields and not record['warmup'] and not record['rounds'] and not record['ns']
                    and set(record['gates']) == set(GATES)
                    and all(slot == {'status': 'not_run', 'result': None, 'error': None}
                            for slot in record['gates'].values()))
        fields |= {'sha256', 'channels', 'kbps', 'frame_ms', 'packets', 'mode', 'toc_frame_ms', 'stream', 'raw_frames', 'file_bytes'}
        if (set(record) != fields or not valid_sha(record['sha256'])
                or type(record['channels']) is not int or record['channels'] not in (1, 2)
                or record['kbps'] not in (12, 24, 32, 64, 128, 256)
                or record['frame_ms'] not in (2.5, 5, 10, 20, 40, 60)
                or not capacity.positive(record['raw_frames']) or not capacity.positive(record['file_bytes'])
                or (record['mode'], record['toc_frame_ms']) != classification(record) or not gate_valid(record)
                or type(record.get('stream')) is not int or record['stream'] < 0):
            return False
        raw = record['gates']['raw_identity']['result']
        cli = record['gates']['cli_identity']['result']
        simultaneous_bytes = 2 * (8 + raw['packets'] * 32 + raw['frames'] * record['channels'] * 4)
        if max(simultaneous_bytes, 2 * cli['output_bytes']) > plan['call_limits']['temporary_output_bytes']:
            return False
        passed = record['gates']['oracle']['result']['passed']
        if record['status'] == 'oracle_disagreement':
            return not passed and not record['warmup'] and not record['rounds'] and not record['ns']
        if (record['status'] != 'timed' or not passed or set(record['warmup']) != set(ENGINES)
                or len(record['rounds']) != plan['rounds']):
            return False
        calls = [record['warmup']]
        for index, row in enumerate(record['rounds']):
            if (set(row) != {'index', 'order', 'calls'} or row['index'] != index
                    or row['order'] != list(ENGINES[index:] + ENGINES[:index])):
                return False
            calls.append(row['calls'])
        if any(set(row) != set(ENGINES) or any(not call_valid(name, row[name], record) for name in ENGINES)
               for row in calls):
            return False
        return record['ns'] == {name: statistics.median(row['calls'][name]['ns'] for row in record['rounds'])
                                for name in ENGINES}
    except (KeyError, TypeError, ValueError, AttributeError, OverflowError, Failure):
        return False


def gate_status(record, name):
    gates = record.get('gates') if isinstance(record, dict) else None
    slot = gates.get(name) if isinstance(gates, dict) else None
    status = slot.get('status') if isinstance(slot, dict) else None
    return status if status in ('not_run', 'complete', 'failed') else 'invalid'


def public_untimed(record):
    if (not isinstance(record, dict) or not valid_sha(record.get('source_sha256'))
            or record.get('class') not in CLASSES):
        return None
    status = record.get('status')
    if status not in (*COMPLETE, 'pending', 'failed'):
        status = 'invalid'
    result = {'source_sha256': record['source_sha256'], 'class': record['class'],
              'status': status, 'failure': None, 'gates': {}}
    failure = record.get('failure')
    stages = ('input', *GATES, 'warmup', 'measured')
    if isinstance(failure, dict) and failure.get('stage') in stages and receipts.validate(failure.get('error')):
        reason = failure.get('reason')
        result['failure'] = {'stage': failure['stage'], 'error': failure['error'],
                            'reason': reason if reason is None or Failure(reason).code == reason else 'internal_error'}
    gates = record.get('gates')
    for name in GATES:
        slot = gates.get(name) if isinstance(gates, dict) else None
        value = {'status': 'invalid', 'result': None, 'error': None}
        if isinstance(slot, dict):
            try:
                if slot == {'status': 'not_run', 'result': None, 'error': None}:
                    value = slot
                elif slot.get('status') == 'complete' and slot.get('error') is None and capacity.result_valid(name, slot.get('result')):
                    value = {'status': 'complete', 'result': slot['result'], 'error': None}
                elif slot.get('status') == 'failed' and receipts.validate(slot.get('error')):
                    value = {'status': 'failed', 'result': None, 'error': slot['error']}
            except (KeyError, TypeError, ValueError, AttributeError):
                pass
        result['gates'][name] = value
    return result


def timed_call(execute, name, source, stream, work, record):
    role = 'lav' if name.startswith('lav_') else name.split('_')[0] + '_abench'
    if name.endswith('_cli'):
        role = 'head_coreav' if name == 'head_cli' else 'ffmpeg'
        output = work / (name + '.wav')
        output.unlink(missing_ok=True)
        if name == 'head_cli':
            command = [execute.tools[role], 'decode-audio', '--force', '--threads', '1', '--stream', stream,
                       '--format', 's16', source, output]
            operation = 'capacity.cli.head'
        else:
            command = [execute.tools[role], '-nostdin', '-v', 'error', '-y', *capacity_run.AUDIO_DECODE_INPUT,
                       '-i', source, '-map', f'0:{stream}', '-vn', '-sn', '-dn', '-c:a', 'pcm_s16le',
                       '-ar', '48000', '-ac', str(record['channels']), output]
            operation = 'capacity.ffmpeg'
        started = time.perf_counter_ns()
        done = execute.invoke(operation, role, command)
        ns = time.perf_counter_ns() - started
        try:
            info = capacity_run.wav_info(output, record['channels'])
        except receipts.CommandFailure as error:
            raise receipts.CommandFailure({**error.receipt, 'operation': operation, 'tool': role}, stderr=done.stderr) from None
        value = {'ok': True, 'ns': ns, **info}
        output.unlink()
    else:
        kind = 'lib' if name.endswith('_lib') else 'decoder'
        command = ('decode' if kind == 'lib' else 'decoder') if role == 'lav' else kind
        operation = ('capacity.lav.' + ('library' if kind == 'lib' else 'decoder') if role == 'lav'
                     else 'capacity.' + kind + '.' + name.split('_')[0])
        done = execute.invoke(operation, role, [execute.tools[role], command, source, stream])
        try:
            parsed = receipts.decode_json(done.stdout)
        except BaseException as error:
            raise receipts.CommandFailure(receipts.exception_receipt(error, operation, role), stderr=done.stderr) from None
        fields = {'ok', 'ns', 'samples'} | ({'bytes'} if name.endswith('_lib') else set())
        if name in ('head_lib', 'base_lib'):
            fields |= {'warnings'}
        if name.startswith('lav_'):
            fields |= {'errors'} | ({'packets'} if name == 'lav_dec' else set())
        if not isinstance(parsed, dict) or set(parsed) != fields:
            raise receipts.CommandFailure(receipts.receipt(operation, role, 'protocol_error'), stderr=done.stderr)
        value = {field: parsed.get(field) for field in fields}
    if not call_valid(name, value, record):
        raise receipts.CommandFailure(receipts.receipt(operation, role, 'protocol_error'), stderr=done.stderr)
    return value


def one(execute, record, result, work, plan):
    stage = 'input'
    try:
        source = (execute.media / record['file']).resolve()
        suffix = '.mp4' if record['class'] == 'mp4' else '.webm'
        if (execute.media.resolve() not in source.parents or source.suffix != suffix
                or source.stat().st_size != record['file_bytes'] or capacity.digest(source) != record['sha256']):
            raise Failure('changed_input_bytes')
        result['stream'] = stream = execute.selected_stream(record)
        for stage in GATES:
            value = execute(stage, record)
            if not capacity.result_valid(stage, value):
                if stage == 'oracle' and oracle.validate(value) and value['status'] == 'error':
                    raise receipts.CommandFailure(value['error'])
                raise Failure('invalid_gate_work')
            result['gates'][stage] = {'status': 'complete', 'result': value, 'error': None}
            if stage != 'oracle' and value['passed'] is not True:
                raise Failure('identity_mismatch')
        if not gate_valid(result):
            raise Failure('inconsistent_gate_work')
        if not result['gates']['oracle']['result']['passed']:
            if capacity.digest(source) != record['sha256']:
                raise Failure('changed_input_bytes')
            result['status'] = 'oracle_disagreement'
            return
        with source.open('rb') as cached:
            while cached.read(4 << 20):
                pass
        stage = 'warmup'
        for name in ENGINES:
            result['warmup'][name] = timed_call(execute, name, source, stream, work, result)
        for index in range(plan['rounds']):
            stage = 'measured'
            order = list(ENGINES[index:] + ENGINES[:index])
            row = {'index': index, 'order': order, 'calls': {}}
            result['rounds'].append(row)
            for name in order:
                row['calls'][name] = timed_call(execute, name, source, stream, work, result)
        result['ns'] = {name: statistics.median(row['calls'][name]['ns'] for row in result['rounds']) for name in ENGINES}
        if capacity.digest(source) != record['sha256']:
            raise Failure('changed_input_bytes')
        result['status'] = 'timed'
        if not record_valid(result, plan):
            raise Failure('invalid_completed_record')
    except BaseException as error:
        result['status'] = 'failed'
        result['ns'] = {}
        result['failure'] = {'stage': stage, **error_info(error)}
        result['failure']['diagnostics'] = list(getattr(execute, 'failures', []))
        if not result['failure']['diagnostics'] and getattr(execute, 'last_diagnostic', None) is not None:
            result['failure']['diagnostics'].append(execute.last_diagnostic)
        if not result['failure']['diagnostics'] and isinstance(error, receipts.CommandFailure):
            result['failure']['diagnostics'].append(receipts.private_failure(error))
        if stage in GATES and result['gates'][stage]['status'] != 'complete':
            result['gates'][stage] = {'status': 'failed', 'result': None,
                                      'error': receipts.exception_receipt(error)}


def run(args, plan, binding, expected, indexed, tools, store):
    started = time.monotonic()
    deadline = started + plan['shard_seconds']
    report = {'schema': 'opus-speed/1', 'round': plan['round'], 'plan_sha256': capacity.digest(args.plan),
              'provenance_commit': plan['provenance_commit'], 'binding_sha256': capacity.canonical(binding),
              'roster_sha256': capacity.digest(args.roster), 'shard': args.shard, 'host': None,
              'allowance': allowance.admit(indexed, expected, plan)[args.shard],
              'results': [blank(indexed[source, cls]) for source in expected['shards'][args.shard] for cls in CLASSES],
              'errors': [], 'complete': False, 'run_seconds': None}
    try:
        with capacity_run.job_deadline(deadline):
            store.write(report)
            report['host'] = host_check(args.work, binding, plan)
            runtime_matches(binding, tools)
            for result in report['results']:
                if result['status'] == 'derivation_failed':
                    continue
                runtime_matches(binding, tools)
                execute = capacity_run.Executor(ROOT, args.media, args.work, tools, binding['limits'], deadline)
                with tempfile.TemporaryDirectory(prefix='opus-speed-', dir=args.work) as directory:
                    one(execute, indexed[key(result)], result, pathlib.Path(directory), plan)
                runtime_matches(binding, tools)
                if result['status'] == 'failed':
                    store.write(report)
                    break
                store.write(report)
            runtime_matches(binding, tools)
    except BaseException as error:
        report['errors'].append(error_info(error))
    finally:
        report['run_seconds'] = time.monotonic() - started
        if report['run_seconds'] > plan['shard_seconds'] and not report['errors']:
            report['errors'].append(error_info(capacity_run.failure('job_deadline')))
        report['complete'] = not report['errors'] and all(record['status'] in COMPLETE and record_valid(record, plan)
                                                        for record in report['results'])
        store.write(report)
    return report


def main(argv=None):
    parser = capacity.Parser(description=__doc__)
    for name in ('plan', 'roster', 'inputs', 'exclusions', 'tools', 'media', 'work', 'out-dir'):
        parser.add_argument('--' + name, type=pathlib.Path, required=True)
    parser.add_argument('--shard', choices=('0', '1', '2', '3'), required=True)
    try:
        args = parser.parse_args(argv)
        plan, binding = load_plan(args.plan)
        if (capacity.digest(args.exclusions) != plan['exclusions_sha256']
                or capacity.digest(args.inputs) != capacity.load(args.roster)['inputs_sha256']):
            raise Failure('changed_registered_inputs')
        expected = capacity.load(args.roster)
        indexed = input_records(capacity.load(args.inputs), expected, capacity.load(args.exclusions), plan)
        tools = capacity.load(args.tools)
        runtime_matches(binding, tools)
        pair = test_oracle.require(tools['ffmpeg'], tools['ffprobe'])
        tools = {**tools, 'ffmpeg': str(pair.ffmpeg), 'ffprobe': str(pair.ffprobe)}
        registration = {'round': plan['round'], 'plan_sha256': capacity.digest(args.plan),
            'roster_sha256': capacity.digest(args.roster), 'binding_sha256': capacity.canonical(binding),
            'inputs_sha256': expected['inputs_sha256'], 'exclusions_sha256': plan['exclusions_sha256'], 'shard': args.shard}
        with open('/tmp/coreav-opus-speed.lock', 'a') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            store = Store(args.out_dir, plan, args.shard, registration)
            report = run(args, plan, binding, expected, indexed, tools, store)
        return 0 if report['complete'] else 1
    except SystemExit as error:
        return 0 if error.code == 0 else 1
    except BaseException as error:
        print(json.dumps({'schema': 'opus-speed-error/1', **error_info(error)}))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
