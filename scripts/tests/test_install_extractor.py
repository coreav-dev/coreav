"""The historical extractor's installer tries again when the publisher answers with a page that is not
the archive, logs only that answer's size and hash (its text could carry workflow commands), and reaches
for the bench bucket last. curl, sleep and (where missing) sha256sum are stand-ins on PATH, so
nothing is downloaded."""
import hashlib
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCRIPT = ROOT / 'docs' / 'measurements' / 'h264-training' / 'tools' / 'install-extractor.sh'
PAGE = '::add-mask::value\n::set-output name=x::y\n\x1b]0;title\x07 Rate limit exceeded'


class InstallExtractor(unittest.TestCase):
    def test_a_page_that_is_not_the_archive_is_retried_shown_and_then_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            shim = tmp / 'shim'
            shim.mkdir()
            calls = tmp / 'calls'
            (shim / 'curl').write_text(
                '#!/bin/bash\nwhile [ $# -gt 0 ]; do [ "$1" = --output ] && out=$2; shift; done\n'
                f"printf '%b' '{PAGE.encode('unicode_escape').decode()}' > \"$out\"\necho call >> {calls}\n")
            (shim / 'sleep').write_text('#!/bin/bash\nexit 0\n')
            if shutil.which('sha256sum') is None:
                (shim / 'sha256sum').write_text('#!/bin/bash\nif [[ " $* " == *--check* ]]; then cat | shasum -a 256 -c -; else shasum -a 256 "$@"; fi\n')
            for tool in shim.iterdir():
                tool.chmod(0o755)
            env = {key: value for key, value in os.environ.items() if not key.startswith('R2_')}
            env.update(PATH=f"{shim}:{os.environ['PATH']}", RUNNER_TEMP=str(tmp / 'run'))
            (tmp / 'run').mkdir()
            result = subprocess.run(['bash', str(SCRIPT)], env=env, capture_output=True, text=True, check=False)
            attempts = calls.read_text().count('call')
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertEqual(attempts, 6)
        self.assertEqual(result.stderr.count('not the archive'), 6)
        self.assertIn(f'answered with {len(PAGE.encode())} bytes (sha256 {hashlib.sha256(PAGE.encode()).hexdigest()})',
                      result.stderr)
        for text in ('::', 'add-mask', 'set-output', '\x1b', 'Rate limit'):
            self.assertNotIn(text, result.stderr)
        self.assertIn('R2_ACCESS_KEY_ID', result.stderr)
        self.assertTrue(result.stderr.rstrip().endswith('No source served the historical extractor archive.'))


if __name__ == '__main__':
    unittest.main()
