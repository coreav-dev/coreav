import importlib.util
import base64
import contextlib
import gc
import io
import json
import os
import pathlib
import struct
import shutil
import subprocess
import sys
import tempfile
import tracemalloc
import types
import unittest
import wave
from unittest.mock import Mock, patch

HERE = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
import capacity
import oracle
import receipts

spec = importlib.util.spec_from_file_location('capacity_run', HERE / 'run.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)

SECRET = 'credential-marker-not-for-a-receipt'


def disagreement():
    return {'schema': 'opus-oracle/1', 'status': 'ok', 'passed': False,
            'detail': {'samples': {'ours': 960, 'ref': 960, 'default': 960},
                       'length_ok': True, 'default_length_ok': True, 'compare_passed': False, 'quality': None},
            'error': None}


def work_result(operation):
    pcm = {'frames': 960, 'channels': 1, 'sample_rate': 48000, 'sample_width': 2, 'output_bytes': 1964}
    if operation == 'raw_identity':
        return {'passed': True, 'packets': 1, 'frames': 960, 'channels': 1}
    if operation == 'cli_identity':
        return {'passed': True, **pcm}
    if operation == 'ffmpeg_cli':
        return pcm
    if operation == 'oracle':
        return disagreement()
    if operation in ('head_lib', 'base_lib'):
        return {'samples': 960, 'bytes': 1920}
    if operation == 'lav_lib':
        return {'samples': 960, 'bytes': 1920, 'errors': 0}
    if operation == 'lav_dec':
        return {'samples': 960, 'packets': 1, 'errors': 0}
    return {'samples': 960}


def write_wave(path, frames=960, channels=1):
    with wave.open(str(path), 'wb') as output:
        output.setnchannels(channels)
        output.setsampwidth(2)
        output.setframerate(48000)
        output.writeframes(b'\0' * frames * channels * 2)


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)
        self.record = {'sha256': 'a' * 64, 'class': 'mp4', 'mode': 'celt', 'file': 'synthetic.mp4',
                       'channels': 1, 'packets': {'celt/20': 1}}
        self.binding = {'synthetic': 'bound-by-parent'}
        self.host = {'cpu': 'Intel Xeon Platinum 8581C', 'smt': False, 'outputs_tmpfs': True,
                     'memory_as_bytes': 1 << 30, 'tmpfs_budget_bytes': 4096, 'tmpfs_capacity_bytes': 4096}
        self.limits = {'memory_bytes': 1 << 30, 'tmpfs_bytes': 4096, 'per_call_seconds': 5,
                       'job_seconds': 30, 'concurrency': 1}
        self.store = runner.Store(self.root / 'checkpoint.json', self.root / 'out.json', self.binding)
        self.calls = []

    def execute(self, operation, record):
        self.calls.append(operation)
        return work_result(operation)

    def run_pass(self, execute=None, rebind=None, **kwargs):
        return runner.run(self.binding, [self.record], self.host, self.limits, self.store,
                          execute or self.execute, rebind or (lambda: self.binding), self.root, **kwargs)

    def test_fixed_schedule_has_one_call_per_operation_and_keeps_oracle_disagreement(self):
        result = self.run_pass()
        self.assertEqual(self.calls, list(capacity.OPERATIONS))
        self.assertTrue(result['complete'])
        self.assertFalse(result['passed'])
        self.assertEqual(result['inputs'][0]['operations']['oracle']['status'], 'complete')
        self.assertFalse(result['inputs'][0]['operations']['oracle']['result']['passed'])
        self.assertEqual(capacity.load(self.root / 'out.json'), result)
        self.assertFalse(capacity.qualify(result, self.binding))
        serialized = json.dumps(result)
        for key in ('"ns"', 'elapsed', 'median', 'speed', 'duration'):
            self.assertNotIn(key, serialized)

    def test_complete_roster_is_checkpointed_before_first_operation(self):
        def execute(operation, record):
            checkpoint = capacity.load(self.root / 'checkpoint.json')
            slots = checkpoint['inputs'][0]['operations']
            self.assertEqual(set(slots), set(capacity.OPERATIONS))
            self.assertEqual(slots[operation]['status'], 'running')
            return self.execute(operation, record)
        self.run_pass(execute)

    def test_spot_resume_restarts_only_the_active_pair_and_needs_no_old_outputs(self):
        def interrupt(operation, record):
            self.calls.append(operation)
            if operation == 'cli_identity':
                raise receipts.CommandFailure(receipts.receipt('capacity.cli.head', 'head_coreav', 'cancelled'))
            return work_result(operation)
        first = self.run_pass(interrupt)
        self.assertEqual(self.calls, ['raw_identity', 'cli_identity'])
        self.assertEqual(first['inputs'][0]['operations']['cli_identity']['status'], 'interrupted')
        self.calls.clear()
        second = self.run_pass(resume=True)
        self.assertEqual(self.calls, list(capacity.OPERATIONS)[1:])
        self.assertTrue(second['complete'])

    def test_crashed_running_slot_is_retried_but_completed_failures_are_terminal(self):
        value = runner.new_receipt(self.binding, [self.record], self.host, False)
        slots = value['inputs'][0]['operations']
        slots['raw_identity'].update(status='failed', error=runner.failure('exit').receipt)
        slots['cli_identity'].update(status='running')
        self.store.write(value)
        result = self.run_pass(resume=True)
        self.assertNotIn('raw_identity', self.calls)
        self.assertIn('cli_identity', self.calls)
        for op in ('head_dec', 'base_dec', 'lav_dec'):
            self.assertNotIn(op, self.calls)
            self.assertEqual(result['inputs'][0]['operations'][op]['status'], 'pending')
        self.assertFalse(result['complete'])
        self.calls.clear()
        self.run_pass(resume=True)
        self.assertEqual(self.calls, [])

    def test_binary_failure_does_not_suppress_independent_operations_or_leak_errors(self):
        def execute(operation, record):
            self.calls.append(operation)
            if operation == 'head_lib':
                raise RuntimeError(SECRET)
            return work_result(operation)
        result = self.run_pass(execute)
        self.assertEqual(self.calls, list(capacity.OPERATIONS))
        self.assertEqual(result['inputs'][0]['operations']['head_lib']['status'], 'failed')
        self.assertEqual(result['inputs'][0]['operations']['ffmpeg_cli']['status'], 'complete')
        self.assertNotIn(SECRET, json.dumps(result))
        self.calls.clear()
        self.run_pass(resume=True)
        self.assertEqual(self.calls, [])

    def test_oracle_failure_is_terminal_but_independent_work_continues(self):
        def execute(operation, record):
            if operation == 'oracle':
                return oracle.failure(receipts.receipt('oracle.compare', 'opus_compare', 'timeout'))
            return self.execute(operation, record)
        result = self.run_pass(execute)
        self.assertEqual(result['inputs'][0]['operations']['oracle']['status'], 'failed')
        self.assertIn('ffmpeg_cli', self.calls)
        self.calls.clear()
        self.run_pass(resume=True)
        self.assertEqual(self.calls, [])

    def test_job_deadline_interruption_retries_but_call_timeout_stays_terminal(self):
        for outcome, status in (('job_deadline', 'interrupted'), ('timeout', 'failed')):
            with self.subTest(outcome=outcome):
                self.calls.clear()
                def execute(operation, record):
                    if operation == 'oracle':
                        return oracle.failure(receipts.receipt('oracle.compare', 'opus_compare', outcome))
                    return self.execute(operation, record)
                value = self.run_pass(execute)
                self.assertEqual(value['inputs'][0]['operations']['oracle']['status'], status)
                self.calls.clear()
                self.run_pass(resume=True)
                self.assertEqual('oracle' in self.calls, outcome == 'job_deadline')

    def test_bad_counts_and_missing_results_fail_without_suppressing_other_engines(self):
        for operation, replacement in (('head_dec', {'samples': 1}), ('lav_dec', {'samples': 960, 'packets': 2, 'errors': 0}),
                                       ('base_lib', {'samples': 960, 'bytes': 1}), ('head_lib', None)):
            with self.subTest(operation=operation):
                result = self.run_pass(lambda op, rec: replacement if op == operation else work_result(op))
                self.assertEqual(result['inputs'][0]['operations'][operation]['status'], 'failed')
                self.assertEqual(result['inputs'][0]['operations']['ffmpeg_cli']['status'], 'complete')

    def test_binding_drift_before_resume_rejects_all_reuse(self):
        self.run_pass()
        self.calls.clear()
        with self.assertRaises(capacity.Invalid):
            self.run_pass(rebind=lambda: {'changed': True}, resume=True)
        self.assertEqual(self.calls, [])

    def test_binding_drift_during_active_operation_is_checkpointed_as_failure(self):
        calls = 0
        def rebind():
            nonlocal calls
            calls += 1
            return self.binding if calls <= 2 else {'changed': True}
        result = self.run_pass(rebind=rebind)
        self.assertEqual(self.calls, ['raw_identity'])
        self.assertEqual(result['inputs'][0]['operations']['raw_identity']['status'], 'failed')
        self.assertFalse(result['passed'])

    def test_resume_rejects_unknown_fields_and_timing_in_complete_results(self):
        value = self.run_pass()
        value['inputs'][0]['operations']['head_dec']['result']['ns'] = 123
        self.store.write(value)
        self.calls.clear()
        with self.assertRaises(capacity.Invalid):
            self.run_pass(resume=True)
        self.assertEqual(self.calls, [])

    def test_smoke_cannot_qualify_even_if_qualifier_is_accidentally_permissive(self):
        with patch.object(capacity, 'qualify', return_value=True) as qualify:
            value = self.run_pass(smoke=True)
        self.assertEqual(value['purpose'], 'smoke')
        self.assertFalse(value['passed'])
        qualify.assert_not_called()
        self.assertFalse(capacity.qualify(value, self.binding))

    def test_tmpfs_shortage_is_closed_and_stops_work(self):
        with patch.object(runner.os, 'statvfs', return_value=types.SimpleNamespace(f_bavail=0, f_frsize=4096)):
            result = self.run_pass()
        self.assertEqual(self.calls, [])
        self.assertEqual(result['inputs'][0]['operations']['raw_identity']['status'], 'failed')
        self.assertFalse(result['complete'])

    def test_store_failure_prevents_starting_the_next_operation(self):
        original = self.store.write
        writes = 0
        def write(value):
            nonlocal writes
            writes += 1
            if writes == 3:
                raise capacity.Invalid('checkpoint_store')
            original(value)
        with patch.object(self.store, 'write', side_effect=write), self.assertRaises(capacity.Invalid):
            self.run_pass()
        self.assertEqual(self.calls, ['raw_identity'])

    def test_job_deadline_leaves_unstarted_work_pending(self):
        value = runner.run(self.binding, [self.record], self.host, self.limits, self.store,
                           self.execute, lambda: self.binding, self.root, deadline=0)
        self.assertEqual(self.calls, [])
        self.assertFalse(value['complete'])
        self.assertTrue(all(slot['status'] == 'pending' for slot in value['inputs'][0]['operations'].values()))

    def test_six_long_synthetic_cells_qualify_but_smoke_and_short_work_do_not(self):
        self.limits['tmpfs_bytes'] = 4 << 30
        self.host.update(tmpfs_budget_bytes=4 << 30, tmpfs_capacity_bytes=4 << 30)
        records = []
        for index, (mode, container) in enumerate((mode, cls) for mode in ('celt', 'hybrid', 'silk')
                                                 for cls in ('mp4', 'matroska')):
            records.append(dict(self.record, sha256=f'{index:064x}', mode=mode, **{'class': container},
                                packets={mode + '/20': capacity.MIN_FULL_FRAMES // 960}))
        binding = {'schema': 'opus-capacity-binding/1', 'scope': 'spent',
                   'inputs': [{key: value for key, value in row.items() if key != 'file'} for row in records],
                   'tools': {role: 'b' * 64 for role in capacity.TOOLS},
                   'oracle': {'pin_sha256': 'c' * 64, 'lav_archive_sha256': capacity.oracle_pin.load()['LAV_SHA256'],
                              'libraries': {name + '.so.1': 'e' * 64 for name in
                                            ('libavcodec', 'libavformat', 'libavutil', 'libswresample')}},
                   'judging': {name: 'c' * 64 for name in capacity.JUDGING_INPUTS},
                   'runtime': {'python': '3.12', 'numpy': '2.0.0'}, 'operations': list(capacity.OPERATIONS),
                   'minimum_full_frames': capacity.MIN_FULL_FRAMES, 'limits': self.limits}
        value = runner.new_receipt(binding, records, self.host, False)
        for row in value['inputs']:
            for operation, slot in row['operations'].items():
                result = work_result(operation)
                for key in ('frames', 'samples'):
                    if key in result:
                        result[key] = capacity.MIN_FULL_FRAMES
                if 'packets' in result:
                    result['packets'] = capacity.MIN_FULL_FRAMES // 960
                if 'bytes' in result:
                    result['bytes'] = capacity.MIN_FULL_FRAMES * 2
                if 'output_bytes' in result:
                    result['output_bytes'] = capacity.MIN_FULL_FRAMES * 2 + 44
                if operation == 'oracle':
                    result['detail']['samples'] = dict.fromkeys(('ours', 'ref', 'default'), capacity.MIN_FULL_FRAMES)
                    if row['class'] == 'mp4':
                        result['detail'].update(applied=True, model=capacity.MIN_FULL_FRAMES,
                                                full=capacity.MIN_FULL_FRAMES,
                                                ffmpeg_expected=capacity.MIN_FULL_FRAMES, entries_count=0)
                slot.update(status='complete', result=result)
        value['complete'] = True
        self.assertTrue(capacity.qualify(value, binding))
        value['purpose'] = 'smoke'
        self.assertFalse(capacity.qualify(value, binding))
        value['purpose'] = 'full'
        value['inputs'][-1]['operations']['raw_identity']['result']['frames'] -= 1
        self.assertFalse(capacity.qualify(value, binding))


class BoundaryTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)
        self.limits = {'memory_bytes': 1 << 30, 'tmpfs_bytes': 4096}

    def host(self, filesystem='tmpfs', memory_kib=4 << 20, siblings='0', cpu='8581C'):
        def text(path, *args, **kwargs):
            if str(path) == '/proc/self/mountinfo':
                device = self.root.stat().st_dev
                return f'1 0 {os.major(device)}:{os.minor(device)} / {self.root.resolve()} rw - {filesystem} none rw\n'
            if str(path) == '/proc/meminfo':
                return f'MemAvailable: {memory_kib} kB\n'
            if str(path) == '/proc/cpuinfo':
                return f'model name : Intel(R) Xeon(R) Platinum {cpu}\n'
            return siblings
        sibling = self.root / 'thread_siblings_list'
        with patch.object(pathlib.Path, 'read_text', text), patch.object(pathlib.Path, 'glob', return_value=[sibling]), \
                patch.object(os, 'statvfs', return_value=types.SimpleNamespace(f_blocks=1, f_frsize=4096, f_bavail=1)), \
                patch.object(runner.resource, 'setrlimit') as limit, patch.dict(os.environ):
            host = runner.host_setup(self.root, self.limits)
            limit.assert_called_once_with(runner.resource.RLIMIT_AS, (1 << 30, 1 << 30))
            self.assertEqual(os.environ['OMP_NUM_THREADS'], '1')
            self.assertEqual(os.environ['OPENBLAS_NUM_THREADS'], '1')
            return host

    def test_host_enforces_inherited_address_space_and_verified_tmpfs(self):
        host = self.host()
        self.assertEqual(host, {'cpu': 'Intel Xeon Platinum 8581C', 'smt': False, 'outputs_tmpfs': True,
                                'memory_as_bytes': 1 << 30, 'tmpfs_budget_bytes': 4096, 'tmpfs_capacity_bytes': 4096})

    def test_wrong_filesystem_memory_shortage_and_smt_are_closed_failures(self):
        for kwargs in ({'filesystem': 'ext4'}, {'memory_kib': 1}, {'siblings': '0,1'}):
            with self.subTest(kwargs=kwargs), self.assertRaises(capacity.Invalid):
                self.host(**kwargs)

    def test_wrong_cpu_is_refused_before_any_capacity_work(self):
        with self.assertRaises(capacity.Invalid) as caught:
            self.host(cpu='unknown')
        self.assertEqual(caught.exception.code, 'cpu_mismatch')

    def test_free_space_on_a_larger_tmpfs_does_not_establish_the_output_limit(self):
        work = self.root.resolve()
        device = work.stat().st_dev
        mount = f'1 0 {os.major(device)}:{os.minor(device)} / {work} rw - tmpfs none rw\n'
        stat = types.SimpleNamespace(f_blocks=2, f_frsize=4096, f_bavail=2)
        with patch.object(pathlib.Path, 'read_text', return_value=mount), patch.object(os, 'statvfs', return_value=stat):
            with self.assertRaisesRegex(capacity.Invalid, 'tmpfs_capacity_mismatch'):
                runner.bounded_tmpfs(work, self.limits)
        mount += f'2 1 1:2 / {work}/escape rw - ext4 none rw\n'
        with patch.object(pathlib.Path, 'read_text', return_value=mount):
            with self.assertRaisesRegex(capacity.Invalid, 'nested_output_mount'):
                runner.bounded_tmpfs(work, self.limits)

    @unittest.skipUnless(sys.platform == 'linux', 'The output bound is enforced by Linux tmpfs')
    def test_kernel_bound_covers_child_written_files_and_streamed_captures(self):
        mount = self.root / 'bounded'
        mount.mkdir()
        prefix = [] if os.geteuid() == 0 else [shutil.which('sudo') or 'sudo', '-n']
        subprocess.run([*prefix, 'mount', '-t', 'tmpfs', '-o', 'size=4096,mode=0777', 'tmpfs', str(mount)],
                       check=True, capture_output=True)
        try:
            self.assertEqual(runner.bounded_tmpfs(mount, self.limits), 4096)
            script = self.root / 'writer'
            script.write_text(f'#!{sys.executable}\nimport sys,wave\n'
                              'with wave.open(sys.argv[-1], "wb") as out:\n'
                              ' out.setnchannels(1); out.setsampwidth(2); out.setframerate(48000)\n'
                              ' out.writeframes(bytes(2048))\n')
            script.chmod(0o700)
            tools = dict.fromkeys(capacity.TOOLS, script)
            limits = dict(self.limits, per_call_seconds=5)
            execute = runner.Executor(HERE.parents[1], self.root, mount, tools, limits, float('inf'))
            record = {'file': 'synthetic.mp4', 'channels': 1}
            with patch.object(execute, 'selected_stream', return_value=0):
                with self.assertRaises(receipts.CommandFailure):
                    execute('cli_identity', record)
            self.assertEqual(list(mount.iterdir()), [])
            capture = mount / 'capture'
            with self.assertRaises(receipts.CommandFailure):
                execute.invoke('capacity.ffmpeg', 'ffmpeg',
                               [sys.executable, '-c', 'import sys; sys.stdout.buffer.write(bytes(16384))'],
                               stdout_path=capture)
            self.assertLessEqual(capture.stat().st_size, 4096)
        finally:
            subprocess.run([*prefix, 'umount', str(mount)], check=True, capture_output=True)

    def test_cli_cannot_bind_a_different_tree_than_the_running_judge(self):
        args = []
        for name in ('inputs', 'media', 'tools', 'limits', 'repo', 'work', 'checkpoint', 'out'):
            args += ['--' + name, str(self.root)]
        output = io.StringIO()
        with patch.object(capacity, 'load') as load, contextlib.redirect_stdout(output):
            self.assertEqual(runner.main(args), 1)
        load.assert_not_called()
        error = json.loads(output.getvalue())
        self.assertEqual(error['schema'], 'opus-capacity-error/1')
        self.assertTrue(receipts.validate(error['error']))
        self.assertNotIn(str(self.root), output.getvalue())

    def test_optional_r2_store_uses_bound_key_and_uploads_before_returning(self):
        client = Mock()
        sdk = types.SimpleNamespace(client=Mock(return_value=client))
        env = {name: 'synthetic' for name in ('R2_ENDPOINT', 'R2_BUCKET', 'R2_ACCESS_KEY_ID', 'R2_SECRET_ACCESS_KEY')}
        with patch.dict(sys.modules, boto3=sdk), patch.dict(os.environ, env):
            store = runner.Store(self.root / 'checkpoint', self.root / 'out', {'safe': True}, 'capacity/test')
        store.write({'safe': True})
        kwargs = client.put_object.call_args.kwargs
        self.assertEqual(kwargs['Key'], 'capacity/test/' + capacity.canonical({'safe': True}) + '.json')
        self.assertEqual(json.loads(kwargs['Body']), {'safe': True})
        self.assertEqual(capacity.load(store.checkpoint), {'safe': True})
        client.put_object.side_effect = OSError(SECRET)
        with self.assertRaises(capacity.Invalid) as caught:
            store.write({'safe': True})
        self.assertNotIn(SECRET, str(caught.exception))

    def test_r2_resume_refuses_duplicate_keys_and_never_emits_store_errors(self):
        store = runner.Store(self.root / 'absent', self.root / 'out', {})
        store.client, store.bucket, store.key = Mock(), 'synthetic', 'synthetic'
        store.client.get_object.return_value = {'Body': io.BytesIO(b'{"safe":true,"safe":false}')}
        with self.assertRaises(capacity.Invalid):
            store.read()
        store.client.get_object.side_effect = RuntimeError(SECRET)
        with self.assertRaises(capacity.Invalid) as caught:
            store.read()
        self.assertNotIn(SECRET, str(caught.exception))

    def test_cli_parse_errors_never_echo_supplied_arguments(self):
        output, errors = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(errors):
            code = runner.main(['--' + SECRET, SECRET])
        self.assertEqual(code, 1)
        self.assertNotIn(SECRET, output.getvalue() + errors.getvalue())
        value = json.loads(output.getvalue())
        self.assertTrue(receipts.validate(value['error']))
        self.assertEqual(errors.getvalue(), '')


class OutputTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)
        self.tools = {role: role for role in capacity.TOOLS}
        self.record = {'file': 'synthetic.mp4', 'sha256': 'a' * 64, 'channels': 1, 'packets': {'celt/20': 1}}
        self.executor = runner.Executor(HERE.parents[1], self.root, self.root, self.tools,
                                        {'per_call_seconds': 5}, float('inf'))
        bounded = patch.object(runner, 'bounded_tmpfs', return_value=4096)
        bounded.start()
        self.addCleanup(bounded.stop)

    def test_wave_parser_refuses_missing_empty_and_truncated_outputs(self):
        path = self.root / 'synthetic.wav'
        with self.assertRaises(receipts.CommandFailure):
            runner.wav_info(path, 1)
        write_wave(path, frames=0)
        with self.assertRaises(receipts.CommandFailure):
            runner.wav_info(path, 1)
        write_wave(path)
        path.write_bytes(path.read_bytes()[:-2])
        with self.assertRaises(receipts.CommandFailure):
            runner.wav_info(path, 1)

    def test_subprocess_counters_drop_ns_warnings_and_arbitrary_fields(self):
        script = self.root / 'stub'
        script.write_text('#!' + sys.executable + '\nimport json\nprint(json.dumps(' + repr(
            {'ok': True, 'ns': SECRET, 'samples': 960, 'bytes': 1920, 'warnings': SECRET, 'error': SECRET}) + '))\n')
        script.chmod(0o700)
        self.executor.tools['head_abench'] = script
        with patch.object(self.executor, 'selected_stream', return_value=3):
            result = self.executor('head_lib', self.record)
        self.assertEqual(result, {'samples': 960, 'bytes': 1920})
        self.assertNotIn(SECRET, json.dumps(result))
        self.assertEqual(list(self.root.glob('capacity-*')), [])

    def test_all_commands_use_same_checked_stream_and_pairs_clean_their_outputs(self):
        calls = []
        def invoke(operation, role, argv, timeout=None, stdout_path=None):
            calls.append((operation, role, [str(value) for value in argv]))
            summary = {'ok': True, 'ns': SECRET, 'samples': 960, 'packets': 1, 'bytes': 1920, 'errors': 0}
            if role == 'ffprobe':
                summary = {'streams': [{'codec_name': 'aac', 'index': 0, 'channels': 1},
                                       {'codec_name': 'opus', 'index': 3, 'channels': 1}]}
            elif operation.startswith('capacity.raw.'):
                pathlib.Path(argv[-1]).write_bytes(struct.pack('<5Q', 1, 0, 960, 1, 960) + b'\0' * 3840)
            elif operation.startswith('capacity.cli.') or operation == 'capacity.ffmpeg':
                write_wave(argv[-1])
            return types.SimpleNamespace(stdout=json.dumps(summary).encode())
        with patch.object(self.executor, 'invoke', side_effect=invoke), patch.object(oracle, 'judge', return_value=disagreement()) as judge:
            for operation in capacity.OPERATIONS:
                result = self.executor(operation, self.record)
                self.assertTrue(capacity.result_valid(operation, result), operation)
                self.assertNotIn(SECRET, json.dumps(result))
                self.assertEqual(list(self.root.glob('capacity-*')), [])
        self.assertEqual(sum(role == 'ffprobe' for _, role, _ in calls), 1)
        self.assertEqual(judge.call_args.args[0]['selected_stream'], {'index': 3, 'channels': 1})
        for operation, role, argv in calls:
            if role == 'ffprobe':
                continue
            if operation.startswith('capacity.cli.'):
                self.assertEqual(argv[argv.index('--stream') + 1], '3')
            elif operation == 'capacity.ffmpeg':
                self.assertEqual(argv[argv.index('-map') + 1], '0:3')
                at = argv.index('-i')
                self.assertEqual(argv[at - len(runner.AUDIO_DECODE_INPUT):at], list(runner.AUDIO_DECODE_INPUT))
            else:
                self.assertEqual(argv[3], '3')

    def test_second_half_pair_failure_cleans_both_outputs(self):
        def invoke(operation, role, argv, timeout=None, stdout_path=None):
            write_wave(argv[-1])
            if role == 'head_coreav':
                raise runner.failure('exit')
            return types.SimpleNamespace(stdout=b'{}')
        with patch.object(self.executor, 'selected_stream', return_value=0), patch.object(self.executor, 'invoke', side_effect=invoke):
            with self.assertRaises(receipts.CommandFailure):
                self.executor('cli_identity', self.record)
        self.assertEqual(list(self.root.glob('capacity-*')), [])

    def test_failed_probe_is_not_silently_retried_for_each_operation(self):
        with patch.object(self.executor, 'invoke', side_effect=runner.failure('exit')) as invoke:
            for operation in capacity.OPERATIONS:
                with self.assertRaises(receipts.CommandFailure):
                    self.executor(operation, self.record)
        self.assertEqual(invoke.call_count, 1)

    def test_many_failed_operations_retain_only_current_stderr_and_flat_memory(self):
        child = self.root / 'failed-worker'
        child.write_text('#!' + sys.executable + '\nimport sys\n'
            'sys.stderr.buffer.write(sys.argv[2].encode() + b":" + b"x" * 2097153)\n'
            'sys.exit(2)\n')
        child.chmod(0o700)
        self.executor.tools['head_abench'] = child
        retained, sizes, memory = [], [], []
        tracemalloc.start()
        try:
            with patch.object(self.executor, 'selected_stream', return_value=0):
                for index in range(32):
                    record = {**self.record, 'file': f'failure-{index:02}.mp4'}
                    with self.assertRaises(receipts.CommandFailure):
                        self.executor('head_lib', record)
                    latest = self.executor.failures[-1]
                    self.assertTrue(base64.b64decode(latest['stderr_base64']).startswith(
                        str(self.root / record['file']).encode() + b':'))
                    self.assertEqual(latest['stderr_retained_bytes'], receipts.BUFFER_LIMIT)
                    self.assertTrue(latest['truncated'])
                    retained.append(len(self.executor.failures))
                    sizes.append(sum(len(value['stderr_base64']) for value in self.executor.failures))
                    gc.collect()
                    memory.append(tracemalloc.get_traced_memory()[0])
            self.assertEqual(retained, [1] * 32)
            self.assertEqual(max(sizes), 4 * ((receipts.BUFFER_LIMIT + 2) // 3))
            self.assertLess(max(memory[4:]) - min(memory[4:]), receipts.BUFFER_LIMIT)
            self.assertEqual(list(self.root.glob('capacity-*')), [])
        finally:
            tracemalloc.stop()

    def test_new_operation_drops_stale_diagnostics_before_preparation(self):
        self.executor.failures = [{'stderr_base64': 'previous operation'}]
        self.executor.last_diagnostic = {'stderr_base64': 'previous leaf'}
        with patch.object(self.executor, 'selected_stream', side_effect=runner.failure('exit')):
            with self.assertRaises(receipts.CommandFailure):
                self.executor('head_lib', self.record)
        self.assertEqual(self.executor.failures, [])
        self.assertIsNone(self.executor.last_diagnostic)

    def test_expired_job_never_launches_a_child(self):
        self.executor.deadline = 0
        with patch.object(receipts, 'invoke') as invoke, self.assertRaises(receipts.CommandFailure):
            self.executor.invoke('capacity.lib.head', 'head_abench', ['unused'])
        invoke.assert_not_called()

    def test_timeout_cause_uses_the_invocation_budget_not_the_later_clock(self):
        for remaining, expected in ((2, 'job_deadline'), (10, 'timeout')):
            self.executor.deadline = remaining
            with patch.object(runner.time, 'monotonic', side_effect=[0, 100]), \
                    patch.object(receipts, 'invoke', side_effect=runner.failure('timeout')) as invoke:
                with self.assertRaises(receipts.CommandFailure) as caught:
                    self.executor.invoke('capacity.lib.head', 'head_abench', ['unused'])
            self.assertEqual(caught.exception.receipt['outcome'], expected)
            self.assertEqual(invoke.call_args.args[3], min(remaining, 5))


if __name__ == '__main__':
    unittest.main()
