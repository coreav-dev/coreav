import json
import os
import pathlib
import signal
import struct
import subprocess
import sys
import tempfile
import time
import types
import unittest
from unittest.mock import patch

HERE = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import agreement
import oracle
import receipts

SECRET = 'credential-marker-do-not-publish'


class ReceiptTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)

    def call(self, code, tool='coreav', timeout=3, stdout_path=None):
        return receipts.invoke('capacity.stage', tool, [sys.executable, '-c', code], timeout, stdout_path)

    def test_success_stderr_json_is_not_coreav_failure_and_no_timer_is_persisted(self):
        result = self.call("import sys; print('ok'); print('{\"warnings\":[]}',file=sys.stderr)")
        self.assertEqual(result.stdout, b'ok\n')
        self.assertEqual(result.outcome, 'ok')
        self.assertTrue(receipts.validate(result.receipt))
        self.assertNotIn('ns', result.receipt)
        self.assertNotIn('elapsed', result.receipt)

    def test_ffmpeg_and_ffprobe_exit_zero_diagnostics_are_fatal_and_closed(self):
        for tool in ('ffmpeg', 'ffprobe'):
            with self.subTest(tool=tool), self.assertRaises(receipts.CommandFailure) as caught:
                self.call(f'import sys; print({SECRET!r},file=sys.stderr)', tool)
            value = caught.exception.receipt
            self.assertEqual(value['outcome'], 'protocol_error')
            self.assertEqual(value['diagnostics']['unknown'], 1)
            self.assertNotIn(SECRET, json.dumps(value))

    def test_dts_is_a_closed_category_and_unknown_lines_have_only_counts(self):
        with self.assertRaises(receipts.CommandFailure) as caught:
            self.call("import sys; sys.stderr.write('Application provided invalid, non monotonically increasing dts: 4 >= 3\\nSECRET URL\\n')", 'ffmpeg')
        diag = caught.exception.receipt['diagnostics']
        self.assertEqual(diag['dts'], 1)
        self.assertEqual(diag['unknown'], 1)
        self.assertEqual(diag['lines'], 2)
        self.assertNotIn('SECRET', json.dumps(diag))

    def test_nonzero_signal_and_launch_error_do_not_include_arguments(self):
        commands = [([sys.executable, '-c', f'import sys; sys.exit({SECRET!r})'], 'exit'),
                    ([sys.executable, '-c', 'import os,signal; os.kill(os.getpid(),signal.SIGKILL)'], 'signal'),
                    ([str(self.root / SECRET)], 'launch_error')]
        for args, outcome in commands:
            with self.subTest(outcome=outcome), self.assertRaises(receipts.CommandFailure) as caught:
                receipts.invoke('capacity.stage', 'coreav', args, 2)
            self.assertEqual(caught.exception.receipt['outcome'], outcome)
            self.assertNotIn(SECRET, json.dumps(caught.exception.receipt))
            self.assertNotIn(str(self.root), str(caught.exception))

    def test_stdout_is_bounded_and_file_capture_is_not(self):
        code = f'import sys; sys.stdout.buffer.write(b"x" * {receipts.BUFFER_LIMIT + 1})'
        with self.assertRaises(receipts.CommandFailure) as caught:
            self.call(code)
        self.assertEqual(caught.exception.receipt['outcome'], 'output_limit')
        path = self.root / 'pcm'
        result = self.call(code, stdout_path=path)
        self.assertEqual(result.stdout, b'')
        self.assertEqual(path.stat().st_size, receipts.BUFFER_LIMIT + 1)

    def test_huge_stderr_is_drained_with_bounded_storage(self):
        size = receipts.BUFFER_LIMIT + 10000
        result = self.call(f'import sys; sys.stderr.buffer.write(b"x" * {size})')
        self.assertEqual(len(result.stderr), receipts.BUFFER_LIMIT)
        self.assertEqual(result.receipt['diagnostics']['bytes'], size)
        self.assertTrue(result.receipt['diagnostics']['truncated'])

    def test_failure_helper_and_validator_reject_untrusted_receipt_fields(self):
        for error in (ValueError(SECRET), OSError(SECRET), RuntimeError(SECRET)):
            value = receipts.exception_receipt(error, SECRET, SECRET)
            self.assertTrue(receipts.validate(value))
            self.assertNotIn(SECRET, json.dumps(value))
        bad = receipts.receipt('oracle.reader', 'ffprobe', 'exit') | {'stderr': SECRET}
        error = receipts.CommandFailure(bad)
        self.assertEqual(error.receipt['outcome'], 'protocol_error')
        self.assertNotIn(SECRET, json.dumps(error.receipt))
        for value in ([], {'operation': []}, receipts.receipt('oracle.reader', 'ffprobe', 'timeout') | {'outcome': []}):
            self.assertFalse(receipts.validate(value))

    def test_bad_arguments_are_closed_protocol_errors(self):
        for operation, tool, timeout in (([], 'ffmpeg', 1), ('capacity.stage', [], 1),
                                         ('capacity.stage', 'ffmpeg', SECRET), ('capacity.stage', 'ffmpeg', float('nan'))):
            with self.assertRaises(receipts.CommandFailure) as caught:
                receipts.invoke(operation, tool, ['unused'], timeout)
            self.assertTrue(receipts.validate(caught.exception.receipt))
            self.assertEqual(caught.exception.receipt['outcome'], 'protocol_error')

    def child_code(self, exit_parent=False):
        return f'''import os, signal, time
child = os.fork()
if child == 0:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    os.close(1)
    os.close(2)
    time.sleep(30)
else:
    with open({str(self.root / 'pid.partial')!r}, 'w') as out:
        out.write(str(child))
    os.replace({str(self.root / 'pid.partial')!r}, {str(self.root / 'pid')!r})
    {'os._exit(0)' if exit_parent else 'time.sleep(30)'}
'''

    def assert_child_dead(self):
        pid = int((self.root / 'pid').read_text())
        for _ in range(100):
            state = pathlib.Path(f'/proc/{pid}/stat')
            try:
                status = state.read_text().split()[2]
            except FileNotFoundError:
                return
            if status == 'Z':
                return
            time.sleep(.01)
        self.fail('descendant survived process-group cleanup')

    def test_timeout_kills_descendant_after_pipes_close(self):
        with self.assertRaises(receipts.CommandFailure) as caught:
            self.call(self.child_code(), timeout=.4)
        self.assertEqual(caught.exception.receipt['outcome'], 'timeout')
        self.assert_child_dead()

    def test_parent_exit_kills_descendant_holding_output_pipes(self):
        self.call(self.child_code(exit_parent=True).replace('    os.close(1)\n    os.close(2)\n', ''))
        self.assert_child_dead()

    def test_parent_exit_cannot_leave_closed_pipe_descendant_alive(self):
        self.call(self.child_code(exit_parent=True))
        self.assert_child_dead()

    def test_cancellation_kills_reaps_and_produces_a_closed_receipt(self):
        script = self.root / 'driver.py'
        script.write_text(f'''import json, pathlib, sys
sys.path.insert(0, {str(HERE)!r})
import receipts
try:
    receipts.invoke('capacity.stage', 'coreav', [sys.executable, '-c', {self.child_code()!r}], 20)
except receipts.CommandFailure as error:
    receipts.atomic_write(pathlib.Path({str(self.root / 'receipt')!r}), error.receipt)
''')
        child = subprocess.Popen([sys.executable, script], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: child.kill() if child.poll() is None else None)
        for _ in range(200):
            if (self.root / 'pid').exists():
                break
            time.sleep(.01)
        self.assertTrue((self.root / 'pid').exists())
        child.send_signal(signal.SIGTERM)
        stdout, stderr = child.communicate(timeout=3)
        self.assertEqual((stdout, stderr), (b'', b''))
        value = json.loads((self.root / 'receipt').read_text())
        self.assertEqual(value['outcome'], 'cancelled')
        self.assertEqual(value['signal'], signal.SIGTERM)
        self.assert_child_dead()


STUB = r'''#!/usr/bin/env python3
import json, os, pathlib, signal, struct, sys, time
home = pathlib.Path(__file__).parent
config = json.loads((home / 'config').read_text())
tool = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with (home / 'commands').open('a') as log:
    log.write(json.dumps([tool, *args]) + '\n')
if tool == 'ffprobe':
    print(json.dumps({'streams': [{'codec_name': 'aac', 'index': 0, 'channels': 2},
                                 {'codec_name': 'opus', 'index': 3, 'channels': 1, 'initial_padding': 1}]}))
elif tool == 'ffmpeg':
    at = args.index('-i')
    assert args[at-2:at] == ['-threads:a', '1']
    assert args.count('-threads:a') == 1
    assert args[args.index('-map')+1] == '0:3'
    stage = 'full' if '-ignore_editlist' in args else 'reference' if 'libopus' in args else 'default'
    if config.get('crash') == stage:
        os.kill(os.getpid(), signal.SIGKILL)
    if config.get('diagnostic') == stage:
        print('credential-marker-do-not-publish https://user:secret@private.invalid /private/path', file=sys.stderr)
    if config.get('sleep') == stage:
        time.sleep(20)
    if 'f32le' in args:
        data = struct.pack('<4f', .25, -.25, 1.1, -1.1)
    else:
        data = struct.pack('<4h', 8192, -8192, 32767, -32768)
    if config.get('truncated') == stage:
        data = data[:-1]
    sys.stdout.buffer.write(data)
elif tool == 'coreav':
    assert args[args.index('--threads')+1] == '1'
    assert args[args.index('--stream')+1] == '3'
    if config.get('bad_wav'):
        sys.stdout.buffer.write(b'credential-marker-do-not-publish')
        sys.exit(0)
    data = struct.pack('<4h', 8192, -8192, 32767, -32768)
    sys.stdout.buffer.write(b'RIFF' + struct.pack('<I', 36+len(data)) + b'WAVEfmt ' + struct.pack('<IHHIIHH',16,1,1,48000,96000,2,16) + b'data' + struct.pack('<I',len(data)) + data)
    print(json.dumps({'warnings': ['credential-marker-do-not-publish']}),file=sys.stderr)
else:
    if config.get('compare_crash'):
        os.kill(os.getpid(), signal.SIGKILL)
    if config.get('compare_malformed'):
        print('credential-marker-do-not-publish',file=sys.stderr)
        sys.exit(0)
    if config.get('disagree'):
        print('Test vector FAILS\nOpus quality metric: -2.0',file=sys.stderr)
        sys.exit(1)
    print('Test vector PASSES\nOpus quality metric: 100.0',file=sys.stderr)
'''


class OracleTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = pathlib.Path(self.directory.name)
        self.config = self.root / 'config'
        self.config.write_text('{}')
        for name in ('ffmpeg', 'ffprobe', 'coreav', 'opus_compare'):
            path = self.root / name
            path.write_text(STUB)
            path.chmod(0o755)
        self.tools = types.SimpleNamespace(**{k: self.root / k for k in ('ffmpeg', 'ffprobe', 'coreav', 'opus_compare')})
        self.media = self.root / 'media'
        self.media.mkdir()
        (self.media / 'test.webm').write_bytes(b'never opened by any real decoder')
        self.work = self.root / 'work'
        self.work.mkdir()
        self.record = {'file': 'test.webm', 'channels': 1}
        self.dc = types.SimpleNamespace(known_divergences=lambda: {})
        self.loader = patch.object(oracle, 'load_reader', return_value=self.dc)
        self.loader.start()
        self.addCleanup(self.loader.stop)

    def judge(self, config=None, timeout=3):
        self.config.write_text(json.dumps(config or {}))
        value = oracle.judge(self.record, self.media, self.work, self.root, self.tools, timeout)
        self.assertTrue(oracle.validate(value), value)
        self.assertNotIn(SECRET, json.dumps(value))
        self.assertNotIn(str(self.root), json.dumps(value))
        return value

    def test_pass_uses_shared_audio_arguments_selected_stream_and_thread_one(self):
        value = self.judge()
        self.assertEqual(value['status'], 'ok', value)
        self.assertTrue(value['passed'])
        self.assertEqual(value['detail']['samples'], {'ours': 4, 'ref': 4, 'default': 4})
        self.assertEqual(value['detail']['default']['snr_db'], 'Infinity')
        self.assertFalse(any(self.work.iterdir()))

    def test_f32_capture_keeps_round_to_even_and_clipping(self):
        output = self.work / 'samples'
        values = (.5 / 32768, 1.5 / 32768, -1.5 / 32768, 2., -2.)
        raw = struct.pack('<5f', *values)
        def execute(operation, tool, args, timeout, stdout_path=None):
            pathlib.Path(stdout_path).write_bytes(raw)
        written = agreement.to_file(execute, 'oracle.reference_decode', 'ffmpeg', [], output, 1, f32=True)
        self.assertEqual(written, 10)
        self.assertEqual(struct.unpack('<5h', output.read_bytes()), (0, 2, -2, 32767, -32768))

    def test_wav_capture_skips_padded_chunks_without_unbounded_header_buffer(self):
        output = self.work / 'samples'
        pcm = struct.pack('<2h', 1, -1)
        raw = b'RIFF' + struct.pack('<I', 30) + b'WAVEJUNK' + struct.pack('<I', 1) + b'x\x00data' + struct.pack('<I', 4) + pcm
        def execute(operation, tool, args, timeout, stdout_path=None):
            pathlib.Path(stdout_path).write_bytes(raw)
        written = agreement.to_file(execute, 'oracle.coreav_decode', 'coreav', [], output, 1, wav=True)
        self.assertEqual(written, 4)
        self.assertEqual(output.read_bytes(), pcm)

    def test_default_compare_crash_keeps_its_own_operation(self):
        def execute(operation, tool, argv, timeout, stdout_path=None):
            if operation == 'oracle.default_compare':
                raise receipts.CommandFailure(receipts.receipt(operation, tool, 'signal', signum=9))
            return receipts.invoke(operation, tool, argv, timeout, stdout_path)
        value = oracle.judge(self.record, self.media, self.work, self.root, self.tools, 3, execute=execute)
        self.assertEqual(value['status'], 'error')
        self.assertEqual(value['error']['operation'], 'oracle.default_compare')

    def test_invalid_probe_json_is_a_probe_error(self):
        def execute(operation, tool, argv, timeout, stdout_path=None):
            return types.SimpleNamespace(stdout=SECRET.encode())
        value = oracle.judge(self.record, self.media, self.work, self.root, self.tools, 3, execute=execute)
        self.assertEqual(value['status'], 'error')
        self.assertEqual(value['error']['operation'], 'oracle.probe')
        self.assertNotIn(SECRET, json.dumps(value))

    def test_comparison_fail_is_an_ordinary_disagreement(self):
        value = self.judge({'disagree': True})
        self.assertEqual(value['status'], 'ok', value)
        self.assertFalse(value['passed'])
        self.assertEqual(value['detail']['quality'], -2.0)
        self.assertIsNone(value['error'])

    def test_reference_default_crash_and_diagnostic_preserve_leaf_stage(self):
        for stage in ('reference', 'default'):
            for mode in ('crash', 'diagnostic'):
                with self.subTest(stage=stage, mode=mode):
                    value = self.judge({mode: stage})
                    self.assertEqual(value['status'], 'error')
                    self.assertEqual(value['error']['operation'], 'oracle.' + stage + '_decode')
                    self.assertEqual(value['error']['tool'], 'ffmpeg')

    def test_timeout_retains_reference_leaf_stage(self):
        value = self.judge({'sleep': 'reference'}, timeout=.3)
        self.assertEqual(value['status'], 'error')
        self.assertEqual(value['error']['outcome'], 'timeout')
        self.assertEqual(value['error']['operation'], 'oracle.reference_decode')

    def test_compare_crash_or_malformed_output_is_not_disagreement(self):
        for config in ({'compare_crash': True}, {'compare_malformed': True}):
            with self.subTest(config=config):
                value = self.judge(config)
                self.assertEqual(value['status'], 'error')
                self.assertEqual(value['error']['operation'], 'oracle.compare')

    def test_truncated_capture_and_bad_wav_are_failures_at_their_leaf(self):
        for config, stage in (({'truncated': 'reference'}, 'reference_decode'),
                              ({'truncated': 'default'}, 'default_decode'), ({'bad_wav': True}, 'coreav_decode')):
            with self.subTest(config=config):
                value = self.judge(config)
                self.assertEqual(value['status'], 'error')
                self.assertEqual(value['error']['operation'], 'oracle.' + stage)

    def install_mp4_model(self):
        self.record['file'] = 'test.mp4'
        (self.media / 'test.webm').rename(self.media / 'test.mp4')

        def reference(path, tools, executor=None):
            self.assertIsNotNone(executor)
            done = executor([tools.ffprobe, '-v', 'error', '-of', 'json', path])
            self.assertEqual(done.returncode, 0)
            return {'index': 3, 'plan': [('silence', 1), ('media', 2, 5)], 'edges': [4, 5], 'edits': []}

        self.dc.reference = reference
        self.dc.effective = lambda plan, total: plan
        self.dc.length = lambda piece: piece[1] if piece[0] == 'silence' else piece[2] - piece[1]
        self.dc.ffmpeg_ops = lambda data, entries: ([([('media', 0, 4)], None)], [])

    def test_mp4_model_preserves_skip_silence_cuts_and_reader_executor(self):
        self.install_mp4_model()
        value = self.judge()
        self.assertEqual(value['status'], 'ok', value)
        self.assertEqual(value['detail']['model'], 4)
        self.assertEqual(value['detail']['ffmpeg_expected'], 4)
        self.assertEqual(value['detail']['full'], 4)
        self.assertTrue(value['detail']['applied'])
        commands = [json.loads(line) for line in (self.root / 'commands').read_text().splitlines()]
        self.assertTrue(any('-ignore_editlist' in command for command in commands))

    def test_mp4_full_decode_error_is_not_hidden_as_reader_failure(self):
        self.install_mp4_model()
        value = self.judge({'crash': 'full'})
        self.assertEqual(value['status'], 'error')
        self.assertEqual(value['error']['operation'], 'oracle.full_decode')

    def test_independent_reader_ffprobe_failure_uses_the_same_seam(self):
        self.install_mp4_model()

        def execute(operation, tool, argv, timeout, stdout_path=None):
            if operation == 'oracle.reader':
                raise receipts.CommandFailure(receipts.receipt(operation, tool, 'signal', signum=9))
            return receipts.invoke(operation, tool, argv, timeout, stdout_path)

        value = oracle.judge(self.record, self.media, self.work, self.root, self.tools, 3, execute=execute)
        self.assertEqual(value['error']['operation'], 'oracle.reader')
        self.assertEqual(value['error']['tool'], 'ffprobe')

    def test_arbitrary_exception_text_and_invalid_probe_never_escape(self):
        self.loader.stop()
        with patch.object(oracle, 'load_reader', side_effect=RuntimeError(SECRET)):
            value = self.judge()
        self.assertEqual(value['error']['outcome'], 'internal_error')
        self.record['selected_stream'] = {'index': 2, 'channels': 1}
        value = self.judge()
        self.assertEqual(value['error']['operation'], 'oracle.probe')

    def test_reader_exception_is_labeled_as_reader_not_model(self):
        self.install_mp4_model()
        self.dc.reference = lambda *args, **kw: (_ for _ in ()).throw(ValueError(SECRET))
        value = self.judge()
        self.assertEqual(value['error']['operation'], 'oracle.reader')

    def test_loader_uses_parent_owned_reader_executor_seam_without_mutating_globals(self):
        self.loader.stop()
        repo = HERE.parents[1]
        dc = oracle.load_reader(repo)
        original = dc.subprocess.run
        class Facts:
            tracks = {}
        with patch.object(dc, 'oracle_kd', return_value=types.SimpleNamespace(read_facts=lambda *args: Facts())):
            def executor(args):
                raise receipts.CommandFailure(receipts.receipt('oracle.reader', 'ffprobe', 'timeout'))
            with self.assertRaises(receipts.CommandFailure) as caught:
                dc.reference(self.media / 'test.webm', self.tools, executor=executor)
        self.assertEqual(caught.exception.receipt['operation'], 'oracle.reader')
        self.assertIs(dc.subprocess.run, original)

    def test_read_receipt_rejects_missing_malformed_partial_and_extra_fields(self):
        path = self.root / 'receipt'
        self.assertEqual(oracle.read_receipt(path)['status'], 'error')
        for data in ('{', '{}', '{"schema":"opus-oracle/1","schema":"opus-oracle/1"}',
                     '{"schema":NaN}', json.dumps({'schema': 'opus-oracle/1', 'status': 'ok', 'passed': True,
                                          'error': None, 'detail': {'stderr': SECRET}})):
            path.write_text(data)
            value = oracle.read_receipt(path)
            self.assertEqual(value['status'], 'error')
            self.assertNotIn(SECRET, json.dumps(value))
        value = self.judge()
        receipts.atomic_write(path, value)
        self.assertEqual(oracle.read_receipt(path), value)

    def test_cli_writes_atomic_error_receipt_without_traceback_or_secret_text(self):
        record = self.root / 'record'
        record.write_text('{')
        out = self.root / 'out'
        args = [sys.executable, HERE / 'oracle.py', '--record', record, '--media', self.media, '--work', self.work,
                '--repo', self.root, '--out', out, '--timeout', '1']
        for role in ('coreav', 'ffmpeg', 'ffprobe', 'opus_compare'):
            args += ['--' + role.replace('_', '-'), getattr(self.tools, role)]
        done = subprocess.run(args, capture_output=True, timeout=3)
        self.assertEqual(done.returncode, 1)
        self.assertEqual((done.stdout, done.stderr), (b'', b''))
        self.assertEqual(oracle.read_receipt(out)['status'], 'error')
        self.assertFalse(out.with_name(out.name + '.partial').exists())


if __name__ == '__main__':
    unittest.main()
