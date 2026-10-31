"""scripts/oracle_pin.py: the pin file reads as the installer reads it, the
repository holds no second pin, and the check catches the scattered pins
the repository had before scripts/oracle-ffmpeg.env (a version in workflow
cache keys and in a Python constant, the archive's URL in a script).

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_pin  # noqa: E402


class OraclePin(unittest.TestCase):
    def tree(self, files):
        root = pathlib.Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, root)
        for rel, text in {oracle_pin.PIN: (ROOT / oracle_pin.PIN).read_text(), **files}.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        subprocess.run(['git', 'init', '-q', str(root)], check=True)
        subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
        return root

    def test_bash_reads_the_pin_as_python_does(self):
        out = subprocess.run(['bash', '-c', f'source {ROOT / oracle_pin.PIN} && echo "$VERSION $SHA256"'],
                             check=True, capture_output=True, text=True).stdout.split()
        pin = oracle_pin.load()
        self.assertEqual(out, [pin['VERSION'], pin['SHA256']])

    def test_the_repository_has_one_pin(self):
        self.assertEqual(list(oracle_pin.problems(ROOT)), [])

    def test_a_second_pin_fails(self):
        pin = oracle_pin.load()
        root = self.tree({
            '.github/workflows/a.yml': "          key: linux-x64-ffmpeg-7.0.2-${{ hashFiles('x') }}\n",
            'scripts/names.py': "VERSION = '7.0.2'\n",
            'scripts/install.sh': 'curl https://johnvansickle.com/ffmpeg/releases/ffmpeg-7.0.2-amd64-static.tar.xz\n',
            'scripts/x264.sh': 'COMMIT=4613ac3c15fd75cebc4b9f65b7fb95e70a3acce1\n',
            'scripts/x264_pin.py': "X264_COMMIT = '4613ac3c15fd75cebc4b9f65b7fb95e70a3acce1'\n",
            'scripts/ffmpeg_pin.py': "FFMPEG_VERSION = '10.0'\n",
            'crates/a/tests/t.rs': f'// {pin["X264_COMMIT"][:7]}\n',
            'docs/measurements/r/job.sh': 'curl https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz\n',
        })
        found = [p.split(':')[0] for p in oracle_pin.problems(root)]
        self.assertEqual(sorted(found), ['.github/workflows/a.yml', 'crates/a/tests/t.rs', 'scripts/ffmpeg_pin.py',
                                         'scripts/install.sh', 'scripts/names.py', 'scripts/x264.sh',
                                         'scripts/x264_pin.py'])

    def test_historical_extractor_is_only_for_the_training_job(self):
        call = '          bash docs/measurements/h264-training/tools/install-extractor.sh\n'
        root = self.tree({'.github/workflows/ci.yml': 'jobs:\n  training-identity:\n' + call,
                          'docs/measurements/h264-training/tools/extract.py': "path = 'coreav-training-extractor'\n"})
        self.assertEqual(list(oracle_pin.problems(root)), [])
        workflow = root / '.github/workflows/ci.yml'
        workflow.write_text('jobs:\n  b9:\n' + call)
        self.assertIn('belongs only to training-identity', '\n'.join(oracle_pin.problems(root)))
        workflow.write_text('jobs:\n  training-identity:\n' + call + '          echo path >> "$GITHUB_PATH"\n')
        self.assertIn('must not export', '\n'.join(oracle_pin.problems(root)))
        workflow.write_text('jobs:\n  training-identity:\n' + call)
        other = root / 'docs/measurements/other.sh'
        other.write_text('coreav-training-extractor\n')
        subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
        self.assertIn('belongs only to training-identity', '\n'.join(oracle_pin.problems(root)))

    def test_verify_binaries_accepts_only_the_current_build(self):
        root = self.tree({})
        binaries = root / 'bin'
        binaries.mkdir()
        build = oracle_pin.load(root)['BUILD']

        def write(name, version):
            binary = binaries / name
            binary.write_text(f'#!/bin/sh\nprintf "%s\\n" "{name} version {version} Copyright"\n')
            binary.chmod(0o755)

        for name in ('ffmpeg', 'ffprobe'):
            write(name, build + '-20260930')
        self.assertEqual(list(oracle_pin.verify_bin(binaries, root)), [])
        for name in ('ffmpeg', 'ffprobe'):
            write(name, '7.0.2-static')
            self.assertEqual(len(list(oracle_pin.verify_bin(binaries, root))), 1)
            write(name, build)
        write('ffmpeg', 'n9.0.20')
        self.assertEqual(len(list(oracle_pin.verify_bin(binaries, root))), 1)
        (binaries / 'ffmpeg').unlink()
        self.assertIn('cannot verify', '\n'.join(oracle_pin.verify_bin(binaries, root)))

    def test_workflows_verify_cached_oracles(self):
        for path in (ROOT / '.github/workflows').glob('*.yml'):
            data = json.loads(subprocess.check_output(
                ['ruby', '-ryaml', '-rjson', '-e',
                 'puts JSON.generate(YAML.load_file(ARGV[0], aliases: true))', str(path)]))
            for name, job in data.get('jobs', {}).items():
                steps = job.get('steps', [])
                installers = [s for s in steps if 'scripts/install-oracle-ffmpeg.sh' in s.get('run', '')]
                if not installers:
                    continue
                verified = any('scripts/oracle_pin.py --verify-bin' in s.get('run', '') for s in steps)
                uncached = all('if' not in s for s in installers)
                self.assertTrue(verified or uncached, f'{path.name}: {name}: verify cached oracle binaries')


if __name__ == '__main__':
    unittest.main()
