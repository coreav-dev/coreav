"""Verified maturin wheels reach native and cross builds without another bootstrap."""
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[2]
PLATFORMS = ('linux-x86_64', 'linux-arm64', 'macos-arm64')
BUILD_ARGS = ['--profile', 'dist', '--locked', '--out', 'dist',
              '--manifest-path', 'crates/coreav-py/Cargo.toml']


class MaturinDownloads(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        scripts = self.root / 'scripts'
        scripts.mkdir()
        shutil.copyfile(ROOT / 'scripts/install-ci-maturin.sh', scripts / 'install-ci-maturin.sh')
        shutil.copyfile(ROOT / 'scripts/fetch-ci-archive.sh', scripts / 'fetch-ci-archive.sh')
        (scripts / 'fetch-ci-archive.sh').chmod(0o755)
        (scripts / 'install-ci-maturin.sh').chmod(0o755)
        shutil.copyfile(ROOT / 'rust-toolchain.toml', self.root / 'rust-toolchain.toml')
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        self.log = self.root / 'events.jsonl'
        self.wheel = self.root / 'fixture.whl'
        self.binary = b'''#!/usr/bin/env python3
import json, os, sys
with open(os.environ['FAKE_LOG'], 'a') as out:
    out.write(json.dumps({'tool': 'maturin', 'args': sys.argv[1:],
                          'env': dict(os.environ)}) + '\\n')
'''
        with zipfile.ZipFile(self.wheel, 'w') as archive:
            archive.writestr('maturin-1.16.0.data/scripts/maturin', self.binary)
        self.sha256 = hashlib.sha256(self.wheel.read_bytes()).hexdigest()
        (scripts / 'maturin.env').write_text(
            'MATURIN_VERSION=1.16.0\n' + ''.join(
                f'MATURIN_{platform}_URL=https://files.pythonhosted.org/fixture.whl\n'
                f'MATURIN_{platform}_SHA256={self.sha256}\n'
                for platform in ('LINUX', 'MACOS')))
        mock = '''import json, os, pathlib, subprocess, sys
name = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
with open(os.environ['FAKE_LOG'], 'a') as out:
    out.write(json.dumps({'tool': name, 'args': args}) + '\\n')
if name == 'python3':
    if args[:2] != ['-m', 'pip']:
        os.execv(os.environ['REAL_PYTHON'], [os.environ['REAL_PYTHON'], *args])
elif name == 'curl':
    if os.environ.get('FAIL_DOWNLOAD'):
        sys.exit(22)
    pathlib.Path(args[args.index('--output') + 1]).write_bytes(
        pathlib.Path(os.environ['FAKE_DOWNLOAD']).read_bytes())
elif name == 'rustc':
    print(os.environ['FAKE_MISSING_LIBDIR'])
elif name == 'docker':
    assert '--interactive' in args
    script = sys.stdin.read()
    mounts = [args[i + 1] for i, arg in enumerate(args) if arg == '-v']
    binary_mount = next(m for m in mounts if m.endswith(':/opt/coreav-maturin:ro'))
    source = binary_mount.removesuffix(':/opt/coreav-maturin:ro')
    assert pathlib.Path(source, 'maturin').is_file()
    assert pathlib.Path(source, 'maturin').stat().st_mode & 0o111
    assert len(mounts) == 2
    assert args[args.index('--workdir') + 1] + ':' + args[args.index('--workdir') + 1] in mounts
    assert not any('credentials' in m or 'ssh' in m or '.gitconfig' in m for m in mounts)
    assert '/releases/' not in script and 'maturin-action' not in script
    env = {k: v for k, v in os.environ.items() if k.startswith('FAKE_') or k in
           ('HOME', 'PATH', 'REAL_PYTHON')}
    for i, arg in enumerate(args):
        if arg == '-e':
            key, sep, value = args[i + 1].partition('=')
            if sep or key in os.environ:
                env[key] = value if sep else os.environ[key]
    index = args.index('bash')
    result = subprocess.run(['bash', *args[index + 1:]],
                            input=script.replace('/opt/coreav-maturin', source),
                            text=True, env=env, timeout=15)
    sys.exit(result.returncode)
'''
        for name in ('curl', 'docker', 'python3', 'rustup', 'rustc', 'uv', 'patchelf', 'sudo'):
            path = self.tools / name
            path.write_text(f'#!{sys.executable}\n' + mock)
            path.chmod(0o755)
        self.env = {'PATH': str(self.tools) + os.pathsep + os.environ['PATH'],
                    'HOME': str(self.root / 'home'), 'RUNNER_TEMP': str(self.root),
                    'GITHUB_WORKSPACE': str(self.root),
                    'COREAV_CI_ARCHIVE_DIR': str(self.root / 'cache'),
                    'FAKE_LOG': str(self.log), 'FAKE_DOWNLOAD': str(self.wheel),
                    'FAKE_MISSING_LIBDIR': str(self.root / 'missing-libdir'),
                    'REAL_PYTHON': sys.executable,
                    'CARGO_HOME': '/do-not-forward',
                    'PYO3_CROSS_LIB_DIR': '/cross/python',
                    'CC_aarch64_unknown_linux_gnu': 'aarch64-linux-gnu-gcc',
                    'MATURIN_TEST_CROSS': 'kept',
                    'SSH_AUTH_SOCK': '/do-not-mount-agent',
                    'UNRELATED_SECRET': 'do-not-forward'}

    def run_installer(self, mode, platform, *args):
        return subprocess.run(['bash', str(self.root / 'scripts/install-ci-maturin.sh'),
                               mode, platform, *args], env=self.env, capture_output=True,
                              text=True, timeout=30)

    def events(self, tool):
        if not self.log.exists():
            return []
        return [row for row in map(json.loads, self.log.read_text().splitlines())
                if row['tool'] == tool]

    def cache(self):
        return self.root / 'cache' / f'maturin-wheel-1.16.0-{self.sha256}.archive'

    def test_verified_install_and_cache_hit_for_every_build_path(self):
        for platform in PLATFORMS:
            with self.subTest(platform=platform):
                self.log.unlink(missing_ok=True)
                self.cache().unlink(missing_ok=True)
                destination = self.root / platform
                first = self.run_installer('install', platform, str(destination))
                self.assertEqual(first.returncode, 0, first.stderr)
                self.assertEqual((destination / 'maturin').read_bytes(), self.binary)
                self.assertEqual(self.cache().read_bytes(), self.wheel.read_bytes())
                self.assertEqual(len(self.events('curl')), 1)
                flags = self.events('curl')[0]['args']
                for flag in ('--fail', '--retry', '--retry-all-errors', '--retry-max-time',
                             '--connect-timeout', '--max-time'):
                    self.assertIn(flag, flags)
                self.assertGreater(int(flags[flags.index('--retry') + 1]), 0)
                self.assertLessEqual(int(flags[flags.index('--retry') + 1]), 8)
                self.assertLessEqual(int(flags[flags.index('--retry-max-time') + 1]), 360)
                second = self.run_installer('install', platform, str(destination))
                self.assertEqual(second.returncode, 0, second.stderr)
                self.assertEqual(len(self.events('curl')), 1)

    def test_invalid_cache_is_never_installed_and_is_replaced_only_by_verified_bytes(self):
        for platform in PLATFORMS:
            with self.subTest(platform=platform):
                self.cache().parent.mkdir(exist_ok=True)
                self.cache().write_bytes(b'corrupt cached wheel')
                self.env['FAIL_DOWNLOAD'] = '1'
                destination = self.root / platform
                rejected = self.run_installer('install', platform, str(destination))
                self.assertNotEqual(rejected.returncode, 0)
                self.assertIn('reason=ci_archive_cache_rejected', rejected.stderr)
                self.assertFalse((destination / 'maturin').exists())
                del self.env['FAIL_DOWNLOAD']
                recovered = self.run_installer('install', platform, str(destination))
                self.assertEqual(recovered.returncode, 0, recovered.stderr)
                self.assertEqual((destination / 'maturin').read_bytes(), self.binary)
                self.assertEqual(self.cache().read_bytes(), self.wheel.read_bytes())

    def test_invalid_download_fails_before_extraction_or_build_for_every_path(self):
        self.wheel.write_bytes(b'unverified download')
        for platform in PLATFORMS:
            with self.subTest(platform=platform):
                result = self.run_installer('build', platform, *BUILD_ARGS)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('reason=ci_archive_digest_mismatch', result.stderr)
        self.assertFalse(self.events('docker'))
        self.assertFalse(self.events('maturin'))
        self.assertFalse(self.cache().exists())

    def test_linux_builds_transport_verified_host_executable_and_preserve_environment(self):
        for platform, image, target, policy in (
                ('linux-x86_64', 'quay.io/pypa/manylinux2014_x86_64:latest',
                 'x86_64-unknown-linux-gnu', []),
                ('linux-arm64', 'ghcr.io/rust-cross/manylinux_2_28-cross:aarch64',
                 'aarch64-unknown-linux-gnu', ['--manylinux', '2_28'])):
            with self.subTest(platform=platform):
                self.log.unlink(missing_ok=True)
                result = self.run_installer('build', platform, *BUILD_ARGS)
                self.assertEqual(result.returncode, 0, result.stderr)
                docker = self.events('docker')
                self.assertEqual(len(docker), 1)
                self.assertIn(image, docker[0]['args'])
                built = self.events('maturin')
                self.assertEqual(len(built), 1)
                self.assertEqual(built[0]['args'], ['build', '--target', target, *policy, *BUILD_ARGS])
                env = built[0]['env']
                for name in ('PYO3_CROSS_LIB_DIR', 'CC_aarch64_unknown_linux_gnu', 'MATURIN_TEST_CROSS'):
                    self.assertEqual(env[name], self.env[name])
                for name in ('CARGO_HOME', 'SSH_AUTH_SOCK', 'UNRELATED_SECRET'):
                    self.assertNotIn(name, env)
                self.assertEqual(env['CARGO_INCREMENTAL'], '0')
                self.assertEqual(env['CARGO_TERM_COLOR'], 'always')
                self.assertIn('/opt/python/cp39-cp39/bin', env['PATH'])
                self.assertIn('/opt/python/cp312-cp312/bin', env['PATH'])
                rustup = [row['args'] for row in self.events('rustup')]
                self.assertIn(['update', '--no-self-update', 'stable'], rustup)
                self.assertIn(['override', 'set', 'stable'], rustup)
                self.assertIn(['component', 'add', 'llvm-tools-preview'], rustup)
                self.assertIn(['target', 'add', target], rustup)
                self.assertEqual([r['args'] for r in self.events('python3') if r['args'][:2] == ['-m', 'pip']],
                                 [['-m', 'pip', 'install', '--user', 'cffi']])

    def test_macos_build_is_native_and_preserves_sdk_defaults_and_overrides(self):
        for custom in (False, True):
            with self.subTest(custom=custom):
                self.log.unlink(missing_ok=True)
                if custom:
                    self.env.update(DEVELOPER_DIR='/custom/xcode', SDKROOT='/custom/sdk')
                result = self.run_installer('build', 'macos-arm64', *BUILD_ARGS)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertFalse(self.events('docker'))
                built = self.events('maturin')
                self.assertEqual(len(built), 1)
                self.assertEqual(built[0]['args'], ['build', '--target', 'aarch64-apple-darwin', *BUILD_ARGS])
                self.assertEqual(built[0]['env']['DEVELOPER_DIR'],
                                 '/custom/xcode' if custom else '/Applications/Xcode.app/Contents/Developer')
                self.assertEqual(built[0]['env']['SDKROOT'], '/custom/sdk' if custom else
                                 '/Applications/Xcode.app/Contents/Developer/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk')
                self.assertIn(['target', 'add', '--toolchain', 'stable', 'aarch64-apple-darwin'],
                              [row['args'] for row in self.events('rustup')])

    def test_unsupported_platform_fails_without_download(self):
        result = self.run_installer('install', 'linux-riscv64', str(self.root / 'bin'))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.events('curl'))


class MaturinWorkflow(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = json.loads(subprocess.check_output([
            'ruby', '-rjson', '-rpsych', '-e',
            'puts JSON.generate(Psych.load_file(ARGV[0]))',
            str(ROOT / '.github/workflows/python.yml')], text=True, timeout=15))

    def test_all_wheel_jobs_use_verified_acquisition_without_the_hidden_action_download(self):
        jobs = self.workflow['jobs']
        for platform in PLATFORMS:
            with self.subTest(platform=platform):
                steps = jobs[platform]['steps']
                self.assertFalse(any('maturin-action' in step.get('uses', '') for step in steps))
                pin = next(i for i, step in enumerate(steps) if step.get('id') == 'maturin-pin')
                self.assertEqual(steps[pin]['run'],
                                 f'scripts/install-ci-maturin.sh metadata {platform} >> "$GITHUB_OUTPUT"')
                cache = next(i for i, step in enumerate(steps)
                             if step.get('uses') == './.github/actions/ci-archive-cache')
                self.assertEqual(steps[cache]['with'], {
                    key: '${{ steps.maturin-pin.outputs.' + key + ' }}'
                    for key in ('tool', 'version', 'sha256')})
                build = next(i for i, step in enumerate(steps)
                             if step.get('run', '').startswith('scripts/install-ci-maturin.sh build'))
                self.assertLess(pin, cache)
                self.assertLess(cache, build)
                self.assertEqual(shlex.split(steps[build]['run']),
                                 ['scripts/install-ci-maturin.sh', 'build', platform, *BUILD_ARGS])
                self.assertEqual(jobs[platform]['runs-on'],
                                 'macos-15' if platform == 'macos-arm64' else 'ubuntu-24.04')
        self.assertIn('python3 -I -m unittest discover -s scripts/tests -p \'test_*.py\'',
                      (ROOT / '.github/workflows/deny.yml').read_text())

    def test_pin_changes_run_python_ci_and_supported_paths_use_current_official_hashes(self):
        steps = self.workflow['jobs']['python-changes']['steps']
        patterns = next(step['with']['paths'].splitlines() for step in steps if step.get('id') == 'filter')
        for path in ('scripts/maturin.env', 'scripts/install-ci-maturin.sh',
                     'scripts/fetch-ci-archive.sh', 'scripts/tests/test_maturin_downloads.py',
                     '.github/actions/ci-archive-cache/action.yml'):
            self.assertTrue(any(fnmatch.fnmatchcase(path, pattern) for pattern in patterns), path)
        for platform, digest in (
                ('linux-x86_64', '837db2f4d93d4899c3ac393fb3cc9eebf35b6002336e5d574f4a4fb6c49fe283'),
                ('linux-arm64', '837db2f4d93d4899c3ac393fb3cc9eebf35b6002336e5d574f4a4fb6c49fe283'),
                ('macos-arm64', '131fcc17ddb3ccc48aa183d2172ebf994c322b858e1b9694fb8744d783df66f6')):
            output = subprocess.check_output(['bash', str(ROOT / 'scripts/install-ci-maturin.sh'),
                                              'metadata', platform], text=True, timeout=15)
            self.assertEqual(output, f'tool=maturin-wheel\nversion=1.16.0\nsha256={digest}\n')


if __name__ == '__main__':
    unittest.main()
