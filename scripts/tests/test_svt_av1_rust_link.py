"""Check Rust link orchestration with stand-in tools, never native compilation."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('svt_av1_rust_link', ROOT / 'scripts/svt_av1_rust_link.py')
link = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(link)
RECIPE = runpy.run_path(str(ROOT / 'scripts/encoders/svt-av1/build.py'))


class RustLink(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.prefix = self.root / 'native installation'
        self.output = self.root / 'rust proof'
        self.prefix.mkdir()
        self.compiler = 'fixture C compiler\nversion details'
        self.manifest = {'version': RECIPE['SVT_VERSION'],
                         'archive_sha256': RECIPE['ARCHIVE_SHA256'],
                         'compiler_version': self.compiler, 'sha256': {}}
        files = {'lib/libSvtAv1Enc.a': b'archive', 'bin/svt-av1-version': b'probe',
                 'include/svt-av1/EbSvtAv1Enc.h': b'header'}
        for name in RECIPE['NOTICES'].values():
            files['share/licenses/svt-av1/' + name] = (
                ROOT / 'scripts/tests/fixtures/svt-av1-notices' / name).read_bytes()
        for name, data in files.items():
            path = self.prefix / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            self.manifest['sha256'][name] = hashlib.sha256(data).hexdigest()
        self.tools = {}
        for name in ('rustup', 'rustc', 'musl-gcc', 'clang', 'nm', 'readelf', 'lipo', 'otool'):
            path = self.root / name
            path.write_text('stand-in')
            path.chmod(0o755)
            self.tools[name] = str(path)
        self.calls = []
        self.failure = ''
        self.target_libdir = self.root / 'rust-target'
        (self.target_libdir / 'self-contained').mkdir(parents=True)
        (self.target_libdir / 'self-contained/libunwind.a').write_bytes(b'pinned target unwinder')
        self.configure(next(iter(RECIPE['TARGETS'])))

    def configure(self, target):
        self.target = target
        self.native = RECIPE['TARGETS'][target]
        self.manifest['target'] = target
        (self.prefix / 'manifest.json').write_text(json.dumps(self.manifest))

    def runner(self, command, environment, timeout):
        self.calls.append((command, dict(environment), timeout))
        version, commit = link.rust_pin()
        if command[0] == self.tools['rustup']:
            self.assertEqual(command[1:], ['which', '--toolchain', version, 'rustc'])
            return self.tools['rustc'] + '\n'
        if command == [self.tools['rustc'], '-vV']:
            if self.failure == 'rust':
                commit = '0' * 40
            return f'rustc fixture\nrelease: {version}\ncommit-hash: {commit}\n'
        if command == [self.tools['rustc'], '--print', 'target-libdir', '--target', self.target]:
            return str(self.target_libdir) + '\n'
        if command == [self.tools[self.native.cc], '--version']:
            return 'wrong compiler' if self.failure == 'compiler' else self.compiler + '\n'
        if command[0] == self.tools['rustc']:
            if self.failure == 'compile':
                raise RuntimeError('fixture compile failure')
            self.compile_command = command
            self.probe = Path(command[command.index('-o') + 1])
            source = Path(command[command.index('-o') - 1]).read_text()
            self.assertNotIn('unsafe', source)
            self.assertNotIn('extern', source)
            self.probe.write_bytes(b'linked executable fixture')
            return ''
        if command[0] == self.tools['nm']:
            names = ['_' + name if self.native.system == 'Darwin' else name for name in link.API_ROOTS]
            if self.failure == 'symbol':
                names.pop()
            return ''.join('00001 T ' + name + '\n' for name in names)
        if command[0] == self.tools['readelf']:
            if command[1] == '-h':
                return 'Machine: ' + ('wrong' if self.failure == 'architecture' else self.native.elf_machine) + '\n'
            if command[1] == '-l':
                return 'LOAD\n' + ('INTERP\n' if self.failure == 'interpreter' else '')
            return 'NEEDED libc.so\n' if self.failure == 'dynamic' else 'No dynamic section\n'
        if command[0] == self.tools['lipo']:
            return 'wrong\n' if self.failure == 'architecture' else self.native.machine + '\n'
        if command[0] == self.tools['otool']:
            dependency = '/outside/libSvtAv1Enc.dylib' if self.failure == 'dynamic' else '/usr/lib/libSystem.B.dylib'
            return str(self.probe) + ':\n\t' + dependency + ' (compatibility version 1.0.0)\n'
        if command == [str(self.probe)]:
            return 'wrong\n' if self.failure == 'run' else link.MESSAGE + '\n'
        self.fail(f'unexpected command: {command}')

    def check(self):
        with patch.object(link.platform, 'system', return_value=self.native.system), \
                patch.object(link.platform, 'machine', return_value=self.native.machine), \
                patch.object(link.shutil, 'which', side_effect=self.tools.get):
            return link.check(self.prefix, self.target, self.output, runner=self.runner)

    def test_every_native_target_roots_the_api_and_publishes_only_after_success(self):
        for target in RECIPE['TARGETS']:
            with self.subTest(target=target):
                self.configure(target)
                result = self.check()
                self.assertEqual(result['target'], target)
                self.assertEqual(result['api_roots'], list(link.API_ROOTS))
                self.assertEqual(result['linker'], self.native.cc)
                self.assertEqual(json.loads((self.output / 'manifest.json').read_text()), result)
                self.assertIn('-Clto=fat', self.compile_command)
                self.assertIn('-Clinker=' + self.tools[self.native.cc], self.compile_command)
                self.assertIn('-lstatic=SvtAv1Enc', self.compile_command)
                self.assertEqual('-Clink-self-contained=no' in self.compile_command,
                                 self.native.system == 'Linux')
                if self.native.system == 'Linux':
                    self.assertEqual(result['rust_unwind_sha256'],
                                     hashlib.sha256(b'pinned target unwinder').hexdigest())
                    runtime_paths = [value.removeprefix('-Lnative=') for value in self.compile_command
                                     if value.startswith('-Lnative=')]
                    self.assertEqual(len(runtime_paths), 2)
                    self.assertNotIn(str(self.target_libdir / 'self-contained'), runtime_paths)
                else:
                    self.assertNotIn('rust_unwind_sha256', result)
                for name in link.API_ROOTS:
                    symbol = '_' + name if self.native.system == 'Darwin' else name
                    self.assertIn('-Clink-arg=-Wl,-u,' + symbol, self.compile_command)
                self.assertTrue(all(0 < timeout <= 180 for _, _, timeout in self.calls))
                shutil.rmtree(self.output)
                self.calls.clear()

    def test_link_failures_never_publish_partial_proof_or_modify_the_installation(self):
        baseline = (self.prefix / 'manifest.json').read_bytes()
        for failure in ('rust', 'compiler', 'compile', 'symbol', 'architecture',
                        'interpreter', 'dynamic', 'run'):
            with self.subTest(failure=failure):
                self.failure = failure
                with self.assertRaises((link.LinkError, RuntimeError)):
                    self.check()
                self.assertFalse(self.output.exists())
                self.assertEqual((self.prefix / 'manifest.json').read_bytes(), baseline)
                self.assertFalse(list(self.root.glob('.svt-rust-link-*')))

    def test_macos_refuses_wrong_architecture_and_non_system_dependencies(self):
        self.configure('aarch64-apple-darwin')
        for failure in ('architecture', 'dynamic', 'symbol', 'run'):
            with self.subTest(failure=failure):
                self.failure = failure
                with self.assertRaises(link.LinkError):
                    self.check()
                self.assertFalse(self.output.exists())

    def test_inherited_build_flags_and_unrelated_secrets_are_not_forwarded(self):
        ignored = ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'LIBRARY_PATH',
                   'LD_LIBRARY_PATH', 'DYLD_LIBRARY_PATH', 'CC', 'UNRELATED_SECRET')
        with patch.dict(os.environ, {key: 'fixture-marker' for key in ignored}):
            self.check()
        for _, environment, _ in self.calls:
            self.assertFalse(set(ignored) & set(environment))
            self.assertEqual(environment['RUSTUP_TOOLCHAIN'], link.rust_pin()[0])

    def test_refuses_existing_relative_or_nested_output(self):
        for path in (self.prefix, Path('relative'), self.prefix / 'nested'):
            self.output = path
            with self.subTest(path=path), self.assertRaisesRegex(link.LinkError, 'output'):
                self.check()
        self.assertFalse(self.calls)

    def test_refuses_wrong_host_before_linking(self):
        with patch.object(link.platform, 'system', return_value='unsupported'):
            with self.assertRaisesRegex(link.LinkError, 'native host'):
                link.check(self.prefix, self.target, self.output, runner=self.runner)
        self.assertFalse(self.calls)

    def test_modified_installation_is_rejected_before_linking(self):
        (self.prefix / 'lib/libSvtAv1Enc.a').write_bytes(b'changed')
        with self.assertRaises(ValueError):
            self.check()
        self.assertFalse(self.calls)

    def test_missing_target_unwinder_never_falls_back_to_the_host(self):
        (self.target_libdir / 'self-contained/libunwind.a').unlink()
        with self.assertRaisesRegex(link.LinkError, 'bundled libunwind'):
            self.check()
        self.assertFalse(self.output.exists())
        self.assertFalse(any('-o' in command for command, _, _ in self.calls))

    def test_command_refuses_unsupported_target_without_native_calls(self):
        result = subprocess.run(
            [sys.executable, '-I', str(ROOT / 'scripts/svt_av1_rust_link.py'),
             '--prefix', str(self.prefix), '--target', 'unsupported', '--output', str(self.output)],
            capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn('unsupported native target', result.stderr)
        self.assertNotIn('Traceback', result.stderr)
        self.assertFalse(self.output.exists())


if __name__ == '__main__':
    unittest.main()
