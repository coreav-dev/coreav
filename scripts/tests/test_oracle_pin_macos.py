"""Check macOS pin ownership without changing the frozen Linux checker."""
import hashlib
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_pin
import oracle_pin_macos as macos


class MacOSOraclePin(unittest.TestCase):
    def tree(self, files=None):
        root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root)
        files = files or {}
        for path in (oracle_pin.PIN, macos.PIN):
            files[path] = (ROOT / path).read_text()
        for path, text in files.items():
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text)
        subprocess.run(['git', 'init', '-q', str(root)], check=True)
        subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
        return root

    def test_the_repository_passes_both_independent_pin_checks(self):
        self.assertEqual(list(oracle_pin.problems(ROOT)), [])
        self.assertEqual(list(macos.problems(ROOT)), [])

    def test_bash_and_python_read_the_same_mac_pin(self):
        output = subprocess.check_output(['bash', '-c', f'source {ROOT / macos.PIN}; '
                                          'echo "$MACOS_BUILD $MACOS_FFMPEG_SHA256"'], text=True).split()
        pin = macos.load()
        self.assertEqual(output, [pin['MACOS_BUILD'], pin['MACOS_FFMPEG_SHA256']])

    def test_only_the_mac_pin_owns_its_values(self):
        pin = macos.load()
        for name, value in pin.items():
            with self.subTest(name=name):
                root = self.tree({'scripts/duplicate.sh': f'echo {value}\n'})
                self.assertIn('scripts/duplicate.sh:', '\n'.join(macos.problems(root)))
        root = self.tree({'scripts/duplicate.sh':
                          'curl https://ffmpeg.martin-riedl.de/download/other/ffmpeg.zip\n'})
        self.assertIn('scripts/duplicate.sh:', '\n'.join(macos.problems(root)))

    def test_mac_build_must_use_the_shared_release_version(self):
        root = self.tree()
        path = root / macos.PIN
        pin = macos.load(root)
        path.write_text(path.read_text().replace(pin['MACOS_BUILD'].split('-', 1)[0], 'unregistered'))
        self.assertIn('shared release version', '\n'.join(macos.problems(root)))
        self.assertIn('shared release version', '\n'.join(macos.verify_bin(root, root)))

    def test_verification_checks_hash_architecture_signature_banner_and_encoder(self):
        root = self.tree()
        binaries = root / 'bin'
        binaries.mkdir()
        pin = macos.load(root)
        original = (root / macos.PIN).read_text()

        def write(name, *, version=None, encoder='h264_videotoolbox'):
            binary = binaries / name
            binary.write_text('#!/bin/sh\nif [ "$1" = -version ]; then\n' +
                              f'echo "{name} version {version or pin["MACOS_BUILD"]}"\nelse\n' +
                              f'echo " V....D {encoder} VideoToolbox Encoder"\nfi\n')
            binary.chmod(0o755)
            data = (root / macos.PIN).read_text()
            key = f'MACOS_{name.upper()}_BINARY_SHA256'
            (root / macos.PIN).write_text(data.replace(macos.load(root)[key],
                                                     hashlib.sha256(binary.read_bytes()).hexdigest()))

        real_run = subprocess.run

        def run(args, **kwargs):
            if args[0] == 'lipo':
                return subprocess.CompletedProcess(args, 0, stdout='arm64\n')
            if args[0] == 'codesign':
                self.assertIn(pin['MACOS_SIGNING_TEAM'], args[-2])
                self.assertTrue(args[-2].startswith('=anchor apple generic and '))
                return subprocess.CompletedProcess(args, 0)
            return real_run(args, **kwargs)

        def verify():
            return list(macos.verify_bin(binaries, root))

        with mock.patch.object(macos.subprocess, 'run', side_effect=run):
            for name in ('ffmpeg', 'ffprobe'):
                write(name)
            self.assertEqual(verify(), [])
            for name in ('ffmpeg', 'ffprobe'):
                for version in (oracle_pin.load(root)['BUILD'], pin['MACOS_BUILD'] + '-other',
                                oracle_pin.load(root)['VERSION'] + '0'):
                    write(name, version=version)
                    self.assertIn('expected the', '\n'.join(verify()))
                write(name)
            write('ffmpeg', encoder='not_h264_videotoolbox')
            self.assertIn('missing h264_videotoolbox', '\n'.join(verify()))
            write('ffmpeg')
            (binaries / 'ffprobe').unlink()
            self.assertIn('cannot verify', '\n'.join(verify()))
            write('ffprobe')
            (root / macos.PIN).write_text(original)
            with mock.patch.object(macos.subprocess, 'run') as no_execution:
                self.assertIn('binary SHA-256 mismatch', '\n'.join(verify()))
                no_execution.assert_not_called()


if __name__ == '__main__':
    unittest.main()
