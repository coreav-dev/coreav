import os
import pathlib
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import vp9_speed_gcp as runner


class GeneratedSelection(unittest.TestCase):
    def test_host_queries_and_worker_use_the_owner_returned_pair(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'bin').mkdir()
            qualified = root / 'qualified tools'
            qualified.mkdir()
            ffmpeg, ffprobe = qualified / 'selected-ffmpeg', qualified / 'selected-ffprobe'
            for path in (ffmpeg, ffprobe):
                path.write_text('#!/bin/bash\nprintf "%s\\n" "$0" >> "$EXECUTION_LOG"\n'
                                'printf "%s\\n" "ffmpeg version test" " V vp9 " '
                                '"Stream #0:0 -> #0:0 (vp9"\n')
                path.chmod(0o755)
            for name in runner.PROGRAMS:
                (root / 'bin' / name).write_text('#!/bin/bash\nprintf "%s\\n" "$0" >> "$EXECUTION_LOG"\n'
                    'printf "%s\\n" "ffmpeg version test" " V vp9 " "Stream #0:0 -> #0:0 (vp9"\n')
            script = '''python3() {
    if [ "${3-}" = --resolve ]; then
        printf '%s\n%s\n' "$QUALIFIED_FFMPEG" "$QUALIFIED_FFPROBE"
    elif [ "${3-}" = oracle ]; then
        printf '%s\n' "$@" > "$WORKER_LOG"
    fi
}
sudo() { return 0; }
lscpu() { return 0; }
nproc() { return 0; }
free() { return 0; }
uname() { return 0; }
df() { return 0; }
cat() { return 0; }
dpkg-query() { return 0; }
ldd() { return 0; }
sha256sum() { return 0; }
'''
            script += runner.job_script(SimpleNamespace(expect=[], phases=['oracle']), 'gate.bin', 'first.bin')
            result = subprocess.run(['/bin/bash', '-c', script], cwd=root, capture_output=True, text=True,
                                    env={**os.environ, 'QUALIFIED_FFMPEG': str(ffmpeg),
                                         'QUALIFIED_FFPROBE': str(ffprobe),
                                         'EXECUTION_LOG': str(root / 'executed'),
                                         'WORKER_LOG': str(root / 'worker')}, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / 'executed').read_text().splitlines(), [str(ffmpeg)] * 3)
            arguments = (root / 'worker').read_text().splitlines()
            self.assertEqual(arguments[arguments.index('--ffmpeg') + 1], str(ffmpeg))
            self.assertEqual(arguments[arguments.index('--ffprobe') + 1], str(ffprobe))


if __name__ == '__main__':
    unittest.main()
