import io
import pathlib
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / 'packages' / 'cli' / 'scripts' / 'notices_check.py'
FILES = ('LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE')


def check(*archives):
    return subprocess.run([sys.executable, '-I', str(SCRIPT), *map(str, archives)],
                          capture_output=True, text=True)


class NoticesCheck(unittest.TestCase):
    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp())

    def tarball(self, name, entries):
        path = self.dir / name
        with tarfile.open(path, 'w:gz') as t:
            for member, data in entries.items():
                info = tarfile.TarInfo(member)
                info.size = len(data)
                t.addfile(info, io.BytesIO(data))
        return path

    def zip(self, name, entries):
        path = self.dir / name
        with zipfile.ZipFile(path, 'w') as z:
            for member, data in entries.items():
                z.writestr(member, data)
        return path

    def files(self, prefix, **changed):
        entries = {f'{prefix}/{name}': (ROOT / name).read_bytes() for name in FILES}
        for name, data in changed.items():
            entries.pop(f'{prefix}/{name.replace("_", "-")}')
            if data is not None:
                entries[f'{prefix}/{name.replace("_", "-")}'] = data
        entries[f'{prefix}/bin/coreav'] = b'binary'
        return entries

    def test_every_kind_of_archive_with_the_notices_passes(self):
        archives = [self.tarball('coreav-1.0.0.tgz', self.files('package')),
                    self.tarball('coreav-1.0.0-x86_64-unknown-linux-musl.tar.gz',
                                 self.files('coreav-1.0.0-x86_64-unknown-linux-musl')),
                    self.zip('coreav-1.0.0-x86_64-pc-windows-msvc.zip',
                             self.files('coreav-1.0.0-x86_64-pc-windows-msvc')),
                    self.zip('coreav-1.0.0-cp39-abi3-linux_x86_64.whl',
                             self.files('coreav-1.0.0.dist-info/licenses'))]
        result = check(*archives)
        self.assertEqual(result.returncode, 0, result.stdout)

    def test_a_package_without_notice_fails(self):
        for archive in (self.tarball('coreav-1.0.0.tgz', self.files('package', NOTICE=None)),
                        self.zip('coreav-1.0.0.zip', self.files('coreav-1.0.0', NOTICE=None))):
            result = check(archive)
            self.assertEqual(result.returncode, 1, archive.name)
            self.assertIn('missing or changed: NOTICE', result.stdout)

    def test_a_stale_or_scattered_notice_fails(self):
        stale = self.tarball('stale.tgz', self.files('package', NOTICE=b'an older NOTICE\n'))
        self.assertIn('missing or changed: NOTICE', check(stale).stdout)
        entries = self.files('package', NOTICE=None)
        entries['package/lib/NOTICE'] = (ROOT / 'NOTICE').read_bytes()
        scattered = self.tarball('scattered.tgz', entries)
        result = check(scattered)
        self.assertEqual(result.returncode, 1)
        self.assertIn('not in one directory', result.stdout)

    def test_no_archive_is_a_failure(self):
        self.assertEqual(check().returncode, 1)


if __name__ == '__main__':
    unittest.main()
