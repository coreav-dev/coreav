import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_pin


@unittest.skipUnless(sys.platform == 'linux', 'Linux shared-oracle deployment layout')
class CommandPair(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.build = oracle_pin.load()['BUILD']

    def pair(self, directory):
        directory.mkdir(parents=True)
        for name in ('ffmpeg', 'ffprobe'):
            program = directory / name
            program.write_text(f'#!/bin/sh\necho "{name} version {self.build}"\n')
            program.chmod(0o700)

    def arguments(self):
        media = self.root / 'generated.ts'
        media.write_bytes(b'\x47' + bytes(187))
        return ['ts_speed_gcp.py', '--out', str(self.root / 'result'), '--coreav', str(media),
                '--coreav-cli', str(media), '--lav-dir', str(self.root / 'lav'), '--dry-run', str(media)]

    def test_command_upload_relocates_both_verified_programs(self):
        self.pair(self.root / 'lav/bin')
        code = ('import shutil,sys\nfrom pathlib import Path\nfrom unittest.mock import patch\n'
                'sys.path.insert(0, sys.argv[1])\nimport ts_speed_gcp as driver\n'
                'saved = Path(sys.argv[2])\noriginal = shutil.rmtree\n'
                'def capture(directory, **kwargs):\n'
                '    shutil.copytree(directory, saved)\n    original(directory, **kwargs)\n'
                'sys.argv = sys.argv[3:]\n'
                'with patch.object(shutil, "rmtree", side_effect=capture):\n'
                '    assert driver.main() == 0\n')
        saved = self.root / 'relocated'
        result = subprocess.run([sys.executable, '-I', '-c', code, str(ROOT / 'scripts'), str(saved),
                                 *self.arguments()], capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        script = (saved / 'job.sh').read_text()
        self.assertIn('lav/bin/ffmpeg lav/bin/ffprobe', script)
        resolved = subprocess.run([sys.executable, '-I', str(saved / 'scripts/test_oracle.py'), '--resolve',
                                   '--ffmpeg', str(saved / 'lav/bin/ffmpeg'),
                                   '--ffprobe', str(saved / 'lav/bin/ffprobe')],
                                  capture_output=True, text=True, timeout=30)
        self.assertEqual(resolved.returncode, 0, resolved.stderr)
        self.assertEqual(resolved.stdout.splitlines(),
                         [str(saved / 'lav/bin/ffmpeg'), str(saved / 'lav/bin/ffprobe')])

    def test_probe_only_input_fails_before_an_upload_is_created(self):
        self.pair(self.root / 'lav/bin')
        (self.root / 'lav/bin/ffmpeg').unlink()
        result = subprocess.run([sys.executable, '-I', str(ROOT / 'scripts/ts_speed_gcp.py'),
                                 *self.arguments()[1:]], capture_output=True, text=True, timeout=30)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('stage=oracle_qualification', result.stderr)
        self.assertFalse((self.root / 'result').exists())

    def test_lav_builder_keeps_the_pair_from_the_shared_archive(self):
        pin = oracle_pin.load()
        archive_root = self.root / pin['LAV_ARCHIVE']
        self.pair(archive_root / 'bin')
        for directory in ('include', 'lib'):
            (archive_root / directory).mkdir()
            (archive_root / directory / 'fixture').write_text('generated layout fixture\n')
        (archive_root / 'LICENSE.txt').write_text('generated layout fixture\n')
        archive = self.root / 'fixture.tar.xz'
        with tarfile.open(archive, 'w:xz') as output:
            output.add(archive_root, arcname=archive_root.name)
        checkout = self.root / 'checkout'
        source = checkout / 'tools/ts-speed'
        source.mkdir(parents=True)
        (checkout / 'scripts').mkdir()
        shutil.copy(ROOT / 'tools/ts-speed/build-lav.sh', source)
        environment = (ROOT / 'scripts/oracle-ffmpeg.env').read_text()
        environment = environment.replace(pin['LAV_SHA256'], hashlib.sha256(archive.read_bytes()).hexdigest())
        (checkout / 'scripts/oracle-ffmpeg.env').write_text(environment)
        helpers = self.root / 'helpers'
        helpers.mkdir()
        curl = helpers / 'curl'
        curl.write_text(f'#!{sys.executable}\nimport os,shutil,sys\nfrom pathlib import Path\n'
                        'shutil.copyfile(os.environ["FIXTURE_ARCHIVE"], sys.argv[sys.argv.index("-o") + 1])\n'
                        'Path(os.environ["FIXTURE_URL"]).write_text(sys.argv[-1])\n')
        gcc = helpers / 'gcc'
        gcc.write_text(f'#!{sys.executable}\nimport sys\nfrom pathlib import Path\n'
                       'output = Path(sys.argv[sys.argv.index("-o") + 1])\n'
                       'output.write_text("#!/bin/sh\\necho generated-layout-fixture\\n")\n'
                       'output.chmod(0o700)\n')
        for program in (curl, gcc):
            program.chmod(0o700)
        destination = self.root / 'installed'
        url = self.root / 'url'
        env = dict(os.environ, PATH=str(helpers) + os.pathsep + os.environ['PATH'],
                   FIXTURE_ARCHIVE=str(archive), FIXTURE_URL=str(url))
        result = subprocess.run(['bash', str(source / 'build-lav.sh'), str(destination)], env=env,
                                 capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(url.read_text().rsplit('/', 1)[-1], pin['LAV_ARCHIVE'] + '.tar.xz')
        self.assertIn('lgpl', pin['LAV_ARCHIVE'])
        for name in ('ffmpeg', 'ffprobe'):
            self.assertEqual((destination / 'bin' / name).read_bytes(), (archive_root / 'bin' / name).read_bytes())
        self.assertFalse((destination / 'include').exists())


if __name__ == '__main__':
    unittest.main()
