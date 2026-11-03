"""Exercise native-case orchestration with explicitly synthetic child results."""

import importlib.util
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('svt_av1_native_cases', ROOT / 'scripts/svt_av1_native_cases.py')
DRIVER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = DRIVER
SPEC.loader.exec_module(DRIVER)


class NativeCases(unittest.TestCase):
    def build_context(self, root):
        return {'revision': 'a' * 40, 'workflow_run': '1', 'workflow_attempt': '1',
                'cargo_path': 'synthetic-pinned-cargo', 'rustc_path': 'synthetic-pinned-rustc',
                'toolchain': 'synthetic-test-toolchain', 'native_prefix': str(root)}

    def compile(self, command, record, mutation=None):
        target = command[command.index('--target') + 1]
        target_directory = Path(command[command.index('--target-dir') + 1])
        self.assertFalse(target_directory.exists())
        binary = target_directory / target / 'debug/deps/native-test'
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b'synthetic executable, never executed')
        artifact = {'reason': 'compiler-artifact',
                    'target': {'name': 'coreav_backend_svt_av1', 'kind': ['lib'],
                               'src_path': str(ROOT / 'crates/coreav-backend-svt-av1/src/lib.rs')},
                    'profile': {'test': True}, 'executable': str(binary), 'fresh': False,
                    'features': ['native']}
        records = [artifact, {'reason': 'build-finished', 'success': True}]
        if mutation == 'fresh':
            artifact['fresh'] = True
        elif mutation == 'failed':
            records[-1]['success'] = False
        elif mutation == 'missing-finish':
            records.pop()
        elif mutation == 'duplicate':
            records.insert(0, dict(artifact))
        elif mutation == 'wrong-binary':
            artifact['executable'] = str(binary.parent.parent / 'other')
        elif mutation == 'missing-feature':
            artifact['features'] = []
        elif mutation == 'extra-feature':
            artifact['features'] = ['default', 'native']
        record.update(pid=987654321, exit_code=0, timed_out=False,
                      stdout=''.join(json.dumps(row) + '\n' for row in records),
                      stderr='synthetic compiler stderr')

    def run_cases(self, failure=None):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            identity = self.build_context(root)
            calls = []
            builds = []
            by_id = {case.id: case for case in DRIVER.cases.load()}

            def runner(command, environment, timeout, *, stdin=None, record):
                if command[0] == identity['cargo_path']:
                    builds.append(command)
                    self.compile(command, record)
                    return
                calls.append((command, environment, timeout))
                directory = Path(environment['COREAV_SVT_CASE_DIRECTORY'])
                case = by_id[directory.name]
                outcome = environment['COREAV_SVT_CASE_OUTCOME']
                frames = int(environment['COREAV_SVT_CASE_FRAMES'])
                layout = {'B': case.retained_bytes, 'F': 32, 'P': case.picture_bytes + 32,
                          'S': case.capacity, 'K': case.keyint, 'slot': 16}
                peak = 32 + case.capacity * 16 + min(frames, case.capacity) * (layout['P'] - 16)
                if case.capacity:
                    (directory / 'layout.json').write_text(json.dumps(layout))
                if outcome == 'retained-limit':
                    frames = 0
                self.assertEqual(stdin.read(), b'fixture')
                record.update(pid=987654321, exit_code=0, timed_out=False,
                              stdout='synthetic stdout', stderr='synthetic stderr')
                if environment['COREAV_SVT_CASE_MODE'] == 'deadline-control':
                    record.update(timed_out=True, exit_code=-15)
                    if failure != 'deadline-marker':
                        (directory / 'retained-before-deadline.json').write_text(json.dumps({
                            'pid': record['pid'], 'frames': frames, 'peak': peak,
                            'header_drops': 0, 'plane_drops': 0, 'live_sessions': 1,
                        }))
                    raise DRIVER.PROCESS.BuildError('synthetic registered deadline')
                if len(calls) == 1 and failure == 'timeout':
                    record.update(timed_out=True, exit_code=-15)
                    try:
                        raise subprocess.TimeoutExpired(command, timeout)
                    except subprocess.TimeoutExpired as error:
                        raise DRIVER.PROCESS.BuildError('synthetic timeout') from error
                if len(calls) == 1 and failure == 'status':
                    record['exit_code'] = None
                if len(calls) == 1 and failure == 'crash':
                    record['exit_code'] = -11
                    raise DRIVER.PROCESS.BuildError('synthetic child failure')
                if len(calls) == 1 and failure == 'count':
                    frames += 1
                if not (len(calls) == 1 and failure == 'completion'):
                    completion = {'outcome': outcome, 'frames': frames}
                    if outcome in ('empty-input', 'retained-limit'):
                        completion['open_attempts'] = 0
                    else:
                        segments = ((frames + case.capacity - 1) // case.capacity
                                    if outcome == 'complete' else 1)
                        completion.update(peak=peak, header_drops=frames + segments,
                                          plane_drops=frames, peak_sessions=1)
                        if outcome == 'retained-until-exit':
                            completion.update(header_drops=0, plane_drops=0)
                        if outcome == 'complete':
                            completion.update(segments=segments, packets=segments,
                                              open_attempts=segments)
                        if directory.name == 'one' and failure == 'peak':
                            completion['peak'] -= 1
                    (directory / 'complete.json').write_text(json.dumps(completion))

            output = root / 'results'
            with patch.object(DRIVER, 'context', return_value=identity), \
                    patch.object(DRIVER.cases, 'write_input', side_effect=lambda out, case: out.write(b'fixture')), \
                    patch.object(DRIVER.os, 'killpg', side_effect=ProcessLookupError):
                if failure:
                    with self.assertRaisesRegex(ValueError, 'did not complete'):
                        DRIVER.run(root, 'x86_64-unknown-linux-musl', output, runner)
                else:
                    DRIVER.run(root, 'x86_64-unknown-linux-musl', output, runner)
            report = json.loads((output / 'report.json').read_text())
            self.assertEqual(len(calls), 51)
            self.assertEqual(len(builds), 1)
            self.assertEqual(len(report['cases']), 51)
            self.assertEqual(report['complete'], failure is None)
            self.assertTrue(all(0 < call[2] <= 180 for call in calls))
            self.assertEqual([row['case']['id'] for row in report['cases']],
                             [case.id for case in DRIVER.cases.load()])
            if failure == 'timeout':
                self.assertTrue(report['cases'][0]['timed_out'])
                self.assertEqual((output / 'empty/stderr.log').read_text(), 'synthetic stderr')
            if failure == 'crash':
                self.assertEqual(report['cases'][0]['exit_code'], -11)

    def test_every_case_has_one_isolated_child_and_complete_record(self):
        self.run_cases()

    def test_missing_records_crashes_timeouts_and_counts_fail_closed(self):
        for failure in ['completion', 'status', 'crash', 'timeout', 'count', 'peak', 'deadline-marker']:
            with self.subTest(failure=failure):
                self.run_cases(failure)

    def test_changed_build_identity_is_refused_before_children(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            identity = self.build_context(root)
            called = []

            def runner(command, environment, timeout, *, record):
                called.append(command)
                self.compile(command, record)

            with patch.object(DRIVER, 'context', side_effect=[identity, identity,
                              {**identity, 'revision': 'b' * 40}]):
                with self.assertRaisesRegex(ValueError, 'identity changed'):
                    DRIVER.run(root, 'x86_64-unknown-linux-musl', root / 'results', runner)
            self.assertEqual(len(called), 1)
            self.assertFalse(json.loads((root / 'results/build.json').read_text())['complete'])
            self.assertFalse((root / 'results/report.json').exists())

    def test_build_capture_requires_fresh_complete_target_native_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = 'aarch64-unknown-linux-musl'
            for mutation in ['none', 'fresh', 'failed', 'missing-finish', 'duplicate', 'wrong-binary',
                             'missing-feature', 'extra-feature']:
                def runner(command, environment, timeout, *, record):
                    self.compile(command, record, mutation)

                with self.subTest(mutation=mutation), \
                        patch.object(DRIVER, 'context', return_value=self.build_context(root)):
                    if mutation == 'none':
                        binary, receipt = DRIVER._build(root, target, root / mutation, runner)
                        self.assertEqual(receipt['binary_sha256'], DRIVER.digest(binary))
                        self.assertTrue(receipt['complete'])
                    else:
                        with self.assertRaises(ValueError):
                            DRIVER._build(root, target, root / mutation, runner)

    def test_old_messages_and_binary_cannot_be_relabelled_as_a_new_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = 'x86_64-unknown-linux-musl'
            identity = self.build_context(root)
            old = {}

            def compiler(command, environment, timeout, *, record):
                self.compile(command, record)
                old.update(record)

            with patch.object(DRIVER, 'context', return_value=identity):
                binary, receipt = DRIVER._build(root, target, root / 'old', compiler)
            self.assertTrue(receipt['complete'])
            old_bytes = binary.read_bytes()
            updated = {**identity, 'revision': 'b' * 40, 'workflow_run': '2'}

            def replay(command, environment, timeout, *, record):
                record.update(old)

            with patch.object(DRIVER, 'context', return_value=updated):
                with self.assertRaisesRegex(ValueError, 'outside this invocation'):
                    DRIVER._build(root, target, root / 'new', replay)
            self.assertEqual(binary.read_bytes(), old_bytes)
            self.assertFalse(json.loads((root / 'new/build.json').read_text())['complete'])

    def test_existing_destination_is_refused_before_cargo(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(DRIVER, 'context', return_value=self.build_context(root)), \
                    patch.object(DRIVER.PROCESS, 'run') as runner:
                with self.assertRaises(FileExistsError):
                    DRIVER._build(root, 'x86_64-unknown-linux-musl', root, runner)
                runner.assert_not_called()

    def test_build_owner_pins_command_and_clears_inherited_build_overrides(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            identity = self.build_context(root)

            def compiler(command, environment, timeout, *, record):
                self.assertEqual(command[0], identity['cargo_path'])
                self.assertEqual(environment['RUSTC'], identity['rustc_path'])
                self.assertEqual(environment['RUSTUP_TOOLCHAIN'], identity['toolchain'])
                self.assertEqual(environment['SVT_AV1_PREFIX'], str(root))
                for name in ['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER',
                             'RUSTC_WORKSPACE_WRAPPER']:
                    self.assertEqual(environment[name], '')
                self.assertNotIn('CARGO_BUILD_TARGET_DIR', environment)
                self.assertIn('--no-run', command)
                self.assertIn('--locked', command)
                self.assertEqual(command[command.index('--features') + 1], 'native')
                self.assertEqual(command[command.index('--jobs') + 1], '1')
                self.compile(command, record)

            with patch.object(DRIVER, 'context', return_value=identity), \
                    patch.dict(DRIVER.os.environ, {'RUSTFLAGS': '--cfg stale',
                               'RUSTC_WRAPPER': 'stale-wrapper',
                               'CARGO_BUILD_TARGET_DIR': str(root / 'stale')}):
                binary, receipt = DRIVER._build(root, 'x86_64-unknown-linux-musl', root / 'new', compiler)
            self.assertEqual(receipt['identity'], identity)
            self.assertEqual(receipt['messages_sha256'], DRIVER.digest(root / 'new/cargo-messages.jsonl'))
            self.assertEqual(receipt['binary_sha256'], DRIVER.digest(binary))
            self.assertEqual(receipt['process']['exit_code'], 0)

    def test_cli_does_not_accept_imported_executables_messages_or_build_records(self):
        for option in ['--binary', '--cargo-messages', '--build-record']:
            with self.subTest(option=option), contextlib.redirect_stderr(io.StringIO()), \
                    patch.object(sys, 'argv', ['driver', '--prefix', 'prefix', '--target',
                                 'x86_64-unknown-linux-musl', '--output', 'output', option, 'old']), \
                    patch.object(DRIVER, 'run') as run:
                with self.assertRaises(SystemExit) as error:
                    DRIVER.main()
                self.assertEqual(error.exception.code, 2)
                run.assert_not_called()

    def test_context_verifies_the_clean_revision_and_pinned_compiler(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve(strict=True)
            (root / 'manifest.json').write_text('{}')
            for name in ['cargo', 'rustc']:
                (root / name).write_bytes(f'synthetic {name}'.encode())
            alias = root / 'toolchain-alias'
            alias.symlink_to(root, target_is_directory=True)
            pin = dict(line.split('=', 1) for line in (ROOT / 'scripts/rust-dist.env')
                       .read_text().splitlines() if line and not line.startswith('#'))
            revision = 'a' * 40
            for toolroot, mutation in [(toolroot, mutation) for toolroot in (root, alias)
                                       for mutation in ['none', 'dirty', 'revision', 'release', 'compiler']]:
                def probe(command, **options):
                    self.assertEqual(options['timeout'], 30)
                    if command == ['git', 'status', '--porcelain']:
                        return ' M source.rs\n' if mutation == 'dirty' else ''
                    if command == ['git', 'rev-parse', 'HEAD']:
                        return 'b' * 40 if mutation == 'revision' else revision
                    if command[:2] == ['rustup', 'which']:
                        self.assertEqual(command[2:4], ['--toolchain', pin['RUST_DIST_TOOLCHAIN']])
                        return str(toolroot / command[-1])
                    self.assertEqual(command, [str(root / 'rustc'), '-vV'])
                    release = 'old' if mutation == 'release' else pin['RUST_DIST_TOOLCHAIN']
                    commit = 'old' if mutation == 'compiler' else pin['RUST_DIST_COMMIT']
                    return f'release: {release}\ncommit-hash: {commit}\n'

                with self.subTest(alias=toolroot == alias, mutation=mutation), \
                        patch.object(DRIVER.licenses, 'check'), \
                        patch.object(DRIVER.subprocess, 'check_output', side_effect=probe), \
                        patch.dict(DRIVER.os.environ, {'GITHUB_SHA': revision,
                                   'GITHUB_RUN_ID': '1', 'GITHUB_RUN_ATTEMPT': '2'}):
                    if mutation != 'none':
                        with self.assertRaises(ValueError):
                            DRIVER.context(root, 'x86_64-unknown-linux-musl')
                    else:
                        identity = DRIVER.context(root, 'x86_64-unknown-linux-musl')
                        self.assertEqual(identity['revision'], revision)
                        self.assertEqual(identity['workflow_attempt'], '2')
                        self.assertEqual(identity['cargo_sha256'], DRIVER.digest(root / 'cargo'))
                        self.assertEqual(identity['rustc_sha256'], DRIVER.digest(root / 'rustc'))
                        self.assertEqual(identity['cargo_path'], str(root / 'cargo'))
                        self.assertEqual(identity['rustc_path'], str(root / 'rustc'))


if __name__ == '__main__':
    unittest.main()
