"""Exercise the macOS installer without a network or a native encoder."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_pin_macos


class OracleInstall(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.scripts = self.root / 'scripts'
        self.scripts.mkdir()
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        self.dest = self.root / 'installed'
        self.dest.mkdir()
        for name in ('install-oracle-ffmpeg-macos.sh', 'oracle_pin_macos.py', 'oracle-ffmpeg-macos.env',
                     'oracle_pin.py', 'oracle-ffmpeg.env'):
            shutil.copyfile(ROOT / 'scripts' / name, self.scripts / name)
        for name in ('ffmpeg', 'ffprobe'):
            (self.dest / name).write_text('original ' + name)
        self.env = {**os.environ, 'PATH': str(self.tools) + os.pathsep + os.environ['PATH'],
                    'FIXTURES': str(self.root), 'COREAV_ORACLE_FROM_R2': '1'}
        self.tool('uname', 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac\n')
        self.tool('lipo', 'echo "${TEST_ARCH:-arm64}"\n')
        self.tool('codesign', 'exit "${TEST_SIGNATURE_EXIT:-0}"\n')
        self.tool('curl', 'while [ "$1" != --output ]; do URL=$1; shift; done\n'
                  'cp "$FIXTURES/${URL##*/}" "$2"\n')
        self.pin = oracle_pin_macos.load(self.root)
        self.archives()

    def tool(self, name, body):
        path = self.tools / name
        path.write_text('#!/bin/sh\nset -eu\n' + body)
        path.chmod(0o755)

    def archives(self, *, bad_version=None, encoder='h264_videotoolbox', missing=None):
        pin = dict(self.pin)
        for name in ('ffmpeg', 'ffprobe'):
            version = 'unregistered' if bad_version == name else pin['MACOS_BUILD']
            program = ('#!/bin/sh\nif [ "$1" = -version ]; then\n' +
                       f'echo "{name} version {version}"\nelse\n' +
                       f'echo " V....D {encoder} VideoToolbox Encoder"\nfi\n').encode()
            path = self.root / (name + '.zip')
            with zipfile.ZipFile(path, 'w') as archive:
                archive.writestr('wrong-member' if missing == name else name, program)
            pin[f'MACOS_{name.upper()}_SHA256'] = hashlib.sha256(path.read_bytes()).hexdigest()
            pin[f'MACOS_{name.upper()}_BINARY_SHA256'] = hashlib.sha256(program).hexdigest()
        (self.scripts / 'oracle-ffmpeg-macos.env').write_text(''.join(f'{k}={v}\n' for k, v in pin.items()))

    def install(self):
        return subprocess.run(['bash', str(self.scripts / 'install-oracle-ffmpeg-macos.sh'), str(self.dest)],
                              env=self.env, capture_output=True, text=True, timeout=30)

    def assert_preserved(self, result):
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        for name in ('ffmpeg', 'ffprobe'):
            self.assertEqual((self.dest / name).read_text(), 'original ' + name)
        self.assertEqual(sorted(p.name for p in self.dest.iterdir()), ['ffmpeg', 'ffprobe'])

    def test_success_verifies_and_installs_both_binaries_without_linux_fallback(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotIn('taking the pinned archive from R2', result.stderr)
        for name in ('ffmpeg', 'ffprobe'):
            self.assertIn(f'{name} version {self.pin["MACOS_BUILD"]}', result.stdout)
            self.assertTrue(os.access(self.dest / name, os.X_OK))
        self.assertEqual(sorted(p.name for p in self.dest.iterdir()), ['ffmpeg', 'ffprobe'])

    def test_bad_archive_hash_preserves_both_installed_programs(self):
        for name in ('ffmpeg', 'ffprobe'):
            with self.subTest(name=name):
                self.archives()
                with (self.root / (name + '.zip')).open('ab') as archive:
                    archive.write(b'changed')
                self.assert_preserved(self.install())

    def test_bad_banner_missing_encoder_or_member_preserves_both_programs(self):
        for options in ({'bad_version': 'ffmpeg'}, {'bad_version': 'ffprobe'},
                        {'encoder': 'other'}, {'missing': 'ffmpeg'}, {'missing': 'ffprobe'}):
            with self.subTest(options=options):
                self.archives(**options)
                self.assert_preserved(self.install())

    def test_wrong_architecture_or_signature_preserves_both_programs(self):
        for key, value in (('TEST_ARCH', 'x86_64'), ('TEST_SIGNATURE_EXIT', '1')):
            with self.subTest(key=key):
                self.env[key] = value
                self.assert_preserved(self.install())
                del self.env[key]

    def test_unsupported_host_never_downloads(self):
        self.tool('uname', 'echo unsupported\n')
        self.tool('curl', 'touch "$FIXTURES/downloaded"\nexit 1\n')
        result = self.install()
        self.assert_preserved(result)
        self.assertIn('supports macOS arm64 only', result.stderr)
        self.assertFalse((self.root / 'downloaded').exists())

    def test_partial_replacement_and_destination_corruption_restore_the_old_pair(self):
        for failure in ('partial', 'corrupt'):
            with self.subTest(failure=failure):
                self.tool('install', 'cp "$3" "$5/ffmpeg"\n' +
                          ('exit 1\n' if failure == 'partial' else
                           'cp "$4" "$5/ffprobe"\nprintf changed >> "$5/ffprobe"\n'))
                self.assert_preserved(self.install())

    def test_failed_first_install_removes_both_new_programs(self):
        for name in ('ffmpeg', 'ffprobe'):
            (self.dest / name).unlink()
        self.tool('install', 'cp "$3" "$5/ffmpeg"\nexit 1\n')
        self.assertNotEqual(self.install().returncode, 0)
        self.assertEqual(list(self.dest.iterdir()), [])

    def test_symlink_destinations_are_refused_without_touching_the_target(self):
        target = self.root / 'external'
        target.write_text('untouched')
        (self.dest / 'ffprobe').unlink()
        (self.dest / 'ffprobe').symlink_to(target)
        self.assertNotEqual(self.install().returncode, 0)
        self.assertEqual(target.read_text(), 'untouched')
        self.assertEqual((self.dest / 'ffmpeg').read_text(), 'original ffmpeg')
        self.assertTrue((self.dest / 'ffprobe').is_symlink())


if __name__ == '__main__':
    unittest.main()
