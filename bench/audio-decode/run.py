#!/usr/bin/env python3
"""Execute one untimed, resumable Opus capacity pass over spent inputs."""
import argparse
import contextlib
import importlib.util
import json
import os
import pathlib
import re
import resource
import signal
import sys
import tempfile
import time
import types
import wave

import capacity
import oracle
import receipts

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2] / 'scripts'))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT


def failure(outcome='protocol_error'):
    return receipts.CommandFailure(receipts.receipt('capacity.stage', 'harness', outcome))


def closed_error(error):
    if isinstance(error, capacity.Invalid):
        return failure().receipt
    return receipts.exception_receipt(error)


@contextlib.contextmanager
def job_deadline(deadline):
    def stop(signum, frame):
        raise failure('timeout')

    old = signal.signal(signal.SIGALRM, stop)
    signal.setitimer(signal.ITIMER_REAL, max(0.001, deadline - time.monotonic()))
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, old)


def positive(value):
    return type(value) is int and 0 < value <= receipts.COUNT_LIMIT


def new_receipt(binding, records, host, smoke):
    return {'schema': 'opus-capacity/1', 'purpose': 'smoke' if smoke else 'full',
            'binding': binding, 'host': host,
            'inputs': [{'sha256': record['sha256'], 'class': record['class'], 'mode': record['mode'],
                        'operations': {op: {'status': 'pending', 'result': None, 'error': None}
                                       for op in capacity.OPERATIONS}} for record in records],
            'complete': False, 'passed': False}


def validate_checkpoint(value, expected):
    if (not isinstance(value, dict) or set(value) != set(expected)
            or any(value[key] != expected[key] for key in ('schema', 'purpose', 'binding', 'host'))
            or type(value['complete']) is not bool or type(value['passed']) is not bool
            or not isinstance(value['inputs'], list) or len(value['inputs']) != len(expected['inputs'])):
        raise capacity.Invalid('invalid_receipt')
    for row, wanted in zip(value['inputs'], expected['inputs']):
        if (not isinstance(row, dict) or set(row) != set(wanted)
                or any(row[key] != wanted[key] for key in ('sha256', 'class', 'mode'))
                or not isinstance(row['operations'], dict) or set(row['operations']) != set(capacity.OPERATIONS)):
            raise capacity.Invalid('invalid_receipt')
        for operation, slot in row['operations'].items():
            if not isinstance(slot, dict) or set(slot) != {'status', 'result', 'error'}:
                raise capacity.Invalid('invalid_receipt')
            status, result, error = slot['status'], slot['result'], slot['error']
            if status == 'complete':
                valid = capacity.result_valid(operation, result) and error is None
            elif status in ('failed', 'interrupted'):
                valid = result is None and receipts.validate(error) and error['outcome'] not in ('ok', 'disagree')
            elif status in ('pending', 'running'):
                valid = result is None and error is None
            else:
                valid = False
            if not valid:
                raise capacity.Invalid('invalid_receipt')
    return value


class Store:
    def __init__(self, checkpoint, out, binding, prefix=None):
        self.checkpoint, self.out = pathlib.Path(checkpoint), pathlib.Path(out)
        self.client = None
        if prefix is not None:
            if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9/_-]{0,255}', prefix) or '..' in prefix:
                raise capacity.Invalid('invalid_store')
            names = ('R2_ENDPOINT', 'R2_BUCKET', 'R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY')
            if not all(os.environ.get(name) for name in names):
                raise capacity.Invalid('missing_credentials')
            import boto3
            self.client = boto3.client('s3', endpoint_url=os.environ['R2_ENDPOINT'],
                                       aws_access_key_id=os.environ['R2_ACCESS_KEY_ID'],
                                       aws_secret_access_key=os.environ['R2_SECRET_ACCESS_KEY'])
            self.bucket = os.environ['R2_BUCKET']
            self.key = prefix.rstrip('/') + '/' + capacity.canonical(binding) + '.json'

    def read(self):
        if self.checkpoint.exists():
            return capacity.load(self.checkpoint)
        if self.client is None:
            raise capacity.Invalid('missing_checkpoint')
        try:
            response = self.client.get_object(Bucket=self.bucket, Key=self.key)
            with contextlib.closing(response['Body']) as body:
                data = body.read(8 * 1024 * 1024 + 1)
            if len(data) > 8 * 1024 * 1024:
                raise ValueError
            def pairs(items):
                value = {}
                for key, item in items:
                    if key in value:
                        raise ValueError
                    value[key] = item
                return value
            return json.loads(data, object_pairs_hook=pairs)
        except Exception:
            raise capacity.Invalid('checkpoint_store') from None

    def write(self, value):
        capacity.atomic(self.checkpoint, value)
        capacity.atomic(self.out, value)
        if self.client is not None:
            try:
                self.client.put_object(Bucket=self.bucket, Key=self.key,
                                       Body=json.dumps(value, allow_nan=False, separators=(',', ':')).encode(),
                                       ContentType='application/json')
            except Exception:
                raise capacity.Invalid('checkpoint_store') from None


def host_setup(work, limits):
    work = pathlib.Path(work).resolve(strict=True)
    if not work.is_dir():
        raise capacity.Invalid('invalid_work')
    device = work.stat().st_dev
    identity = f'{os.major(device)}:{os.minor(device)}'
    filesystems = set()
    for line in pathlib.Path('/proc/self/mountinfo').read_text().splitlines():
        left, right = line.split(' - ', 1)
        if left.split()[2] == identity:
            filesystems.add(right.split()[0])
    if filesystems != {'tmpfs'}:
        raise capacity.Invalid('work_not_tmpfs')
    check_space(work, limits)
    available = next((int(line.split()[1]) * 1024 for line in pathlib.Path('/proc/meminfo').read_text().splitlines()
                      if line.startswith('MemAvailable:')), 0)
    if available < limits['memory_bytes']:
        raise capacity.Invalid('memory_shortage')
    resource.setrlimit(resource.RLIMIT_AS, (limits['memory_bytes'], limits['memory_bytes']))
    cpuinfo = pathlib.Path('/proc/cpuinfo').read_text()
    cpu = 'Intel Xeon Platinum 8581C' if re.search(r'model name\s*:.*\b8581C\b', cpuinfo) else 'unknown'
    if cpu == 'unknown':
        raise capacity.Invalid('cpu_mismatch')
    sibling_lists = list(pathlib.Path('/sys/devices/system/cpu').glob('cpu[0-9]*/topology/thread_siblings_list'))
    smt = not sibling_lists or any(',' in path.read_text() or '-' in path.read_text() for path in sibling_lists)
    if smt:
        raise capacity.Invalid('smt_enabled')
    for key in ('OMP_NUM_THREADS', 'OPENBLAS_NUM_THREADS', 'MKL_NUM_THREADS', 'BLIS_NUM_THREADS',
                'VECLIB_MAXIMUM_THREADS', 'NUMEXPR_NUM_THREADS'):
        os.environ[key] = '1'
    return {'cpu': cpu, 'smt': False, 'outputs_tmpfs': True,
            'memory_as_bytes': limits['memory_bytes'], 'tmpfs_budget_bytes': limits['tmpfs_bytes']}


def check_space(work, limits):
    stat = os.statvfs(work)
    if stat.f_bavail * stat.f_frsize < limits['tmpfs_bytes']:
        raise capacity.Invalid('tmpfs_shortage')


def wav_info(path, channels):
    try:
        with wave.open(str(path), 'rb') as source:
            frames = source.getnframes()
            if (source.getnchannels() != channels or source.getframerate() != capacity.INPUT_RATE
                    or source.getsampwidth() != 2 or source.getcomptype() != 'NONE' or not positive(frames)):
                raise ValueError
            remaining = frames * channels * 2
            while remaining:
                data = source.readframes(min(65536, remaining // (channels * 2)))
                if not data or len(data) > remaining:
                    raise ValueError
                remaining -= len(data)
        return {'frames': frames, 'channels': channels, 'sample_rate': capacity.INPUT_RATE,
                'sample_width': 2, 'output_bytes': pathlib.Path(path).stat().st_size}
    except (OSError, EOFError, wave.Error, ValueError):
        raise failure() from None


def identical(left, right):
    with open(left, 'rb') as first, open(right, 'rb') as second:
        while True:
            a, b = first.read(1 << 20), second.read(1 << 20)
            if a != b:
                return False
            if not a:
                return True


class Executor:
    def __init__(self, repo, media, work, tools, limits, deadline):
        self.repo, self.media, self.work = pathlib.Path(repo), pathlib.Path(media), pathlib.Path(work)
        self.tools, self.limits, self.deadline = tools, limits, deadline
        self.streams = {}
        spec = importlib.util.spec_from_file_location('capacity_opus_identity', self.repo / 'scripts/opus_identity.py')
        self.identity = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.identity)

    def invoke(self, operation, role, argv, timeout=None, stdout_path=None):
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise failure('timeout')
        bound = min(self.limits['per_call_seconds'], remaining)
        if timeout is not None:
            bound = min(bound, timeout)
        return receipts.invoke(operation, role, argv, bound, stdout_path=stdout_path)

    def selected_stream(self, record):
        key = record['sha256']
        if key not in self.streams:
            try:
                done = self.invoke('capacity.input', 'ffprobe', [self.tools['ffprobe'], '-v', 'error',
                                   '-show_entries', 'stream=index,codec_name,channels', '-of', 'json',
                                   self.media / record['file']])
                streams = json.loads(done.stdout)['streams']
                stream = next((s for s in streams if s.get('codec_name') == 'opus'), None)
                if (not isinstance(stream, dict) or type(stream.get('index')) is not int or stream['index'] < 0
                        or type(stream.get('channels')) is not int or stream['channels'] != record['channels']):
                    raise failure()
                self.streams[key] = stream['index']
            except Exception as error:
                self.streams[key] = receipts.exception_receipt(error, 'capacity.input', 'ffprobe')
        selected = self.streams[key]
        if isinstance(selected, dict):
            raise receipts.CommandFailure(selected)
        return selected

    def __call__(self, operation, record):
        stream = self.selected_stream(record)
        source, channels = self.media / record['file'], record['channels']
        with tempfile.TemporaryDirectory(prefix='capacity-', dir=self.work) as directory:
            work = pathlib.Path(directory)
            if operation == 'raw_identity':
                paths = [work / 'base.raw', work / 'head.raw']
                counts = []
                for side, path in zip(('base', 'head'), paths):
                    done = self.invoke('capacity.raw.' + side, side + '_abench',
                                       [self.tools[side + '_abench'], 'raw', source, stream, path])
                    summary = json.loads(done.stdout)
                    if (summary.get('ok') is not True or summary.get('packets') != sum(record['packets'].values())
                            or not positive(summary.get('samples'))):
                        raise failure()
                    counts.append(summary['samples'])
                result = self.identity.compare(*paths, sum(record['packets'].values()), channels)
                if any(count != result.get('frames') for count in counts):
                    raise failure()
                return {key: result.get(key) for key in ('passed', 'packets', 'frames', 'channels')}
            if operation == 'cli_identity':
                paths = [work / 'base.wav', work / 'head.wav']
                infos = []
                for side, path in zip(('base', 'head'), paths):
                    self.invoke('capacity.cli.' + side, side + '_coreav',
                                [self.tools[side + '_coreav'], 'decode-audio', '--force', '--threads', '1',
                                 '--stream', stream, '--format', 's16', source, path])
                    infos.append(wav_info(path, channels))
                return {'passed': infos[0] == infos[1] and identical(*paths), **infos[1]}
            if operation == 'oracle':
                selected = {**record, 'selected_stream': {'index': stream, 'channels': channels}}
                tools = types.SimpleNamespace(coreav=self.tools['head_coreav'], ffmpeg=self.tools['ffmpeg'],
                                              ffprobe=self.tools['ffprobe'], opus_compare=self.tools['opus_compare'])
                return oracle.judge(selected, self.media, work, self.repo, tools,
                                    self.limits['per_call_seconds'], execute=self.invoke)
            if operation == 'ffmpeg_cli':
                path = work / 'ffmpeg.wav'
                self.invoke('capacity.ffmpeg', 'ffmpeg',
                            [self.tools['ffmpeg'], '-nostdin', '-v', 'error', '-y', *AUDIO_DECODE_INPUT,
                             '-i', source, '-map', f'0:{stream}', '-vn', '-sn', '-dn', '-c:a', 'pcm_s16le',
                             '-ar', str(capacity.INPUT_RATE), '-ac', str(channels), path])
                return wav_info(path, channels)
            side, kind = operation.split('_')
            role = 'lav' if side == 'lav' else side + '_abench'
            command = ('decode' if kind == 'lib' else 'decoder') if side == 'lav' else ('lib' if kind == 'lib' else 'decoder')
            label = ('capacity.lav.' + ('library' if kind == 'lib' else 'decoder') if side == 'lav'
                     else 'capacity.' + ('lib' if kind == 'lib' else 'decoder') + '.' + side)
            done = self.invoke(label, role, [self.tools[role], command, source, stream])
            summary = json.loads(done.stdout)
            if not isinstance(summary, dict) or summary.get('ok') is not True:
                raise failure()
            fields = {'samples'} | ({'bytes'} if kind == 'lib' else set())
            if side == 'lav':
                fields |= {'errors'} | ({'packets'} if kind == 'dec' else set())
            return {key: summary.get(key) for key in fields}


def reconcile(row, record):
    slots = row['operations']
    raw = slots['raw_identity']['result'] if slots['raw_identity']['status'] == 'complete' else None
    cli = slots['cli_identity']['result'] if slots['cli_identity']['status'] == 'complete' else None
    ffmpeg = slots['ffmpeg_cli']['result'] if slots['ffmpeg_cli']['status'] == 'complete' else None
    for op, slot in slots.items():
        if slot['status'] != 'complete':
            continue
        result = slot['result']
        valid = True
        if op == 'raw_identity':
            valid = result['passed'] and result['packets'] == sum(record['packets'].values()) and result['channels'] == record['channels']
        elif op == 'cli_identity':
            valid = result['passed'] and result['channels'] == record['channels']
        elif op in ('head_dec', 'base_dec') and raw:
            valid = result['samples'] == raw['frames']
        elif op == 'lav_dec' and raw:
            valid = result['packets'] == raw['packets'] and abs(result['samples'] - raw['frames']) <= 5760
        elif op in ('head_lib', 'base_lib', 'lav_lib') and cli:
            frames = cli['frames']
            valid = result['bytes'] == result['samples'] * record['channels'] * 2
            valid &= (abs(result['samples'] - frames) <= 5760 if op == 'lav_lib' else result['samples'] == frames)
            if ffmpeg:
                valid &= abs(result['samples'] - ffmpeg['frames']) <= 5760
        if not valid:
            slot.update(status='failed', result=None, error=failure().receipt)


def run(binding, records, host, limits, store, execute, rebind, work, *, resume=False, smoke=False, deadline=None):
    expected = new_receipt(binding, records, host, smoke)
    value = validate_checkpoint(store.read(), expected) if resume else expected
    for row in value['inputs']:
        for slot in row['operations'].values():
            if slot['status'] == 'running':
                slot.update(status='interrupted', error=failure('cancelled').receipt)
    value['complete'] = value['passed'] = False
    if rebind() != binding:
        raise capacity.Invalid('binding_changed')
    store.write(value)
    with receipts.cancellation('capacity.stage', 'harness'):
        for row, record in zip(value['inputs'], records):
            slots = row['operations']
            reconcile(row, record)
            for operation in capacity.OPERATIONS:
                slot = slots[operation]
                if slot['status'] in ('complete', 'failed'):
                    continue
                dependency = ('raw_identity' if operation.endswith('_dec') else
                              'cli_identity' if operation.endswith('_lib') else None)
                if dependency and slots[dependency]['status'] != 'complete':
                    continue
                if deadline is not None and time.monotonic() >= deadline:
                    store.write(value)
                    return value
                if rebind() != binding:
                    raise capacity.Invalid('binding_changed')
                slot.update(status='running', result=None, error=None)
                store.write(value)
                stop = False
                try:
                    check_space(work, limits)
                    result = execute(operation, record)
                    if rebind() != binding:
                        raise capacity.Invalid('binding_changed')
                    if operation == 'oracle' and oracle.validate(result) and result['status'] == 'error':
                        raise receipts.CommandFailure(result['error'])
                    if not capacity.result_valid(operation, result):
                        raise failure()
                    slot.update(status='complete', result=result, error=None)
                    reconcile(row, record)
                except BaseException as error:
                    closed = closed_error(error)
                    stop = closed['outcome'] == 'cancelled' or isinstance(error, capacity.Invalid)
                    stop |= deadline is not None and time.monotonic() >= deadline
                    slot.update(status='interrupted' if closed['outcome'] == 'cancelled' else 'failed',
                                result=None, error=closed)
                store.write(value)
                if stop:
                    return value
    value['complete'] = all(slot['status'] in ('complete', 'failed') for row in value['inputs']
                            for slot in row['operations'].values())
    value['passed'] = not smoke and capacity.qualify(value, binding)
    store.write(value)
    return value


class Parser(argparse.ArgumentParser):
    def error(self, message):
        raise capacity.Invalid('invalid_arguments')


def main(argv=None):
    parser = Parser(description=__doc__)
    for name in ('inputs', 'media', 'tools', 'limits', 'repo', 'work', 'checkpoint', 'out'):
        parser.add_argument('--' + name, type=pathlib.Path, required=True)
    parser.add_argument('--resume', action='store_true')
    parser.add_argument('--smoke', action='store_true')
    parser.add_argument('--r2-prefix')
    try:
        args = parser.parse_args(argv)
        if args.repo.resolve() != pathlib.Path(__file__).resolve().parents[2]:
            raise capacity.Invalid('different_judging_tree')
        inputs, tools, limits = capacity.load(args.inputs), capacity.load(args.tools), capacity.normalized_limits(capacity.load(args.limits))
        rebind = lambda: capacity.make_binding(args.repo, inputs, args.media, tools, limits)
        binding = rebind()
        records = capacity.records(inputs)
        host = host_setup(args.work, limits)
        store = Store(args.checkpoint, args.out, binding, args.r2_prefix)
        deadline = time.monotonic() + limits['job_seconds']
        execute = Executor(args.repo, args.media, args.work, tools, limits, deadline)
        with job_deadline(deadline):
            value = run(binding, records, host, limits, store, execute, rebind, args.work,
                        resume=args.resume, smoke=args.smoke, deadline=deadline)
        return 0 if value['passed'] or (args.smoke and value['complete'] and all(
            slot['status'] == 'complete' for row in value['inputs'] for slot in row['operations'].values())) else 1
    except SystemExit as error:
        return 0 if error.code == 0 else 1
    except BaseException as error:
        print(json.dumps({'schema': 'opus-capacity-error/1', 'error': closed_error(error)}, separators=(',', ':')))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
