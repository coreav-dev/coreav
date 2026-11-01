import pathlib
import runpy
import shutil
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
BASE = runpy.run_path(str(ROOT / 'scripts/tests/test_notices_check.py'))


class RustDistributionNoticesCheck(unittest.TestCase):
    def setUp(self):
        self.fixture = BASE['NoticesCheck']()
        self.fixture.setUp()
        self.addCleanup(shutil.rmtree, self.fixture.dir)
        self.pin = dict(line.split('=', 1) for line in
                        (ROOT / 'scripts/rust-dist.env').read_text().splitlines())

    def notices(self, target):
        return (f'Rust dependency notices for coreav on {target}.\n'
                f'=== Rust standard library {self.pin["RUST_DIST_TOOLCHAIN"]} '
                f'({self.pin["RUST_DIST_COMMIT"]}) (Rust toolchain) ===\n').encode()

    def archive(self, name, target):
        entries = self.fixture.files('package')
        entries['package/THIRD-PARTY-NOTICES.txt'] = self.notices(target)
        return self.fixture.zip(name, entries) if name.endswith('.zip') else self.fixture.tarball(name, entries)

    def test_every_rust_distribution_shape(self):
        for platform, target in [
                ('linux-x64', 'x86_64-unknown-linux-musl'),
                ('linux-arm64', 'aarch64-unknown-linux-musl'),
                ('darwin-arm64', 'aarch64-apple-darwin'),
                ('darwin-x64', 'x86_64-apple-darwin'),
                ('win32-x64', 'x86_64-pc-windows-msvc')]:
            suffix = 'zip' if platform == 'win32-x64' else 'tar.gz'
            for name in [f'coreav-cli-{platform}-1.0.0.tgz',
                         f'coreav-1.0.0-{target}.{suffix}',
                         f'coreav-full-1.0.0-{target}.{suffix}']:
                with self.subTest(name=name):
                    result = BASE['check']('--third-party', self.archive(name, target))
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        result = BASE['check']('--third-party', self.archive('coreav-1.0.0.tgz', 'wasm32-wasip1'))
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_missing_changed_or_scattered_rust_notices_fail(self):
        target = 'aarch64-apple-darwin'
        notice = self.notices(target)
        for value in [None, b'old notices', b'\xff',
                      notice.replace(target.encode(), b'x86_64-apple-darwin'),
                      notice.replace(self.pin['RUST_DIST_COMMIT'].encode(), b'0' * 40)]:
            with self.subTest(value=value):
                entries = self.fixture.files('package')
                if value is not None:
                    entries['package/THIRD-PARTY-NOTICES.txt'] = value
                archive = self.fixture.tarball('coreav-cli-darwin-arm64-1.0.0.tgz', entries)
                result = BASE['check']('--third-party', archive)
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn('missing or stale THIRD-PARTY-NOTICES.txt', result.stdout)
        entries = self.fixture.files('package')
        entries['package/licenses/THIRD-PARTY-NOTICES.txt'] = notice
        archive = self.fixture.tarball('coreav-cli-darwin-arm64-1.0.0.tgz', entries)
        self.assertEqual(BASE['check']('--third-party', archive).returncode, 1)

    def test_unknown_target_fails_closed(self):
        archive = self.archive('unknown.tgz', 'aarch64-apple-darwin')
        result = BASE['check']('--third-party', archive)
        self.assertEqual(result.returncode, 1)
        self.assertIn('unrecognized Rust binary archive target', result.stdout)

    def test_wasi_archive_names_do_not_cover_the_javascript_package(self):
        for name in ('coreav-wasm-1.0.0.tgz', 'coreav-other-1.0.0.tgz'):
            with self.subTest(name=name):
                result = BASE['check']('--third-party', self.archive(name, 'wasm32-wasip1'))
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
                self.assertIn('unrecognized Rust binary archive target', result.stdout)


if __name__ == '__main__':
    unittest.main()
