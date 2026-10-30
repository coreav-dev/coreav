"""scripts/oracle_pin.py: the pin file reads as the installer reads it, the
repository holds no second pin, and the check catches the scattered pins
the repository had before scripts/oracle-ffmpeg.env (a version in workflow
cache keys and in a Python constant, the archive's URL in a script).

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
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
            'crates/a/tests/t.rs': f'// {pin["X264_COMMIT"][:7]}\n',
            'docs/measurements/r/job.sh': 'curl https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz\n',
        })
        found = [p.split(':')[0] for p in oracle_pin.problems(root)]
        self.assertEqual(sorted(found), ['.github/workflows/a.yml', 'crates/a/tests/t.rs', 'scripts/install.sh',
                                         'scripts/names.py', 'scripts/x264.sh'])


if __name__ == '__main__':
    unittest.main()
