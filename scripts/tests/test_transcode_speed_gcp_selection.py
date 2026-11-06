import json
from pathlib import Path
import shlex
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import transcode_speed_gcp as driver


class Selection(unittest.TestCase):
    def test_generated_version_and_runner_use_the_owner_returned_pair(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            selected = root / 'selected encoder'
            selected.write_text('#!/bin/sh\nprintf "qualified version\\n"\n')
            selected.chmod(0o700)
            probe = root / 'separate probe'
            code = (f'import sys; from pathlib import Path; sys.path.insert(0, {str(driver.HERE)!r}); '
                    'import test_oracle; '
                    f'test_oracle.require = lambda *a: test_oracle.Oracle(Path({str(selected)!r}), Path({str(probe)!r})); '
                    'sys.exit(test_oracle.main())')
            script = driver.job_script('gate.h264', ['h264'])
            admission = script.split('(sudo apt-get', 1)[0].replace('chmod +x bin/*', 'true')
            admission = admission.replace('python3 -I scripts/test_oracle.py',
                                          f'{shlex.quote(sys.executable)} -I -B -c {shlex.quote(code)}')
            version = script.split('sha256sum bin/* scripts/*; ', 1)[1].split('; }', 1)[0]
            recorder = root / 'record.py'
            recorder.write_text('import json, pathlib, sys\npathlib.Path("arguments.json").write_text(json.dumps(sys.argv[1:]))\n')
            runner = next(line for line in script.splitlines() if line.startswith('python3 -I scripts/transcode_speed.py run'))
            runner = runner.replace('python3 -I scripts/transcode_speed.py',
                                    f'{shlex.quote(sys.executable)} -I -B {shlex.quote(str(recorder))}')
            done = subprocess.run(['bash', '-c', admission + version + '\n' + runner], cwd=root,
                                  capture_output=True, timeout=10)
            self.assertEqual(done.returncode, 0, done.stderr)
            self.assertEqual(done.stdout, b'qualified version\n')
            args = json.loads((root / 'arguments.json').read_text())
            self.assertEqual(args[args.index('--ffmpeg') + 1], str(selected))
            self.assertEqual(args[args.index('--ffprobe') + 1], str(probe))
            self.assertIn('ffmpeg=--encoder ffmpeg --ffmpeg ' + str(selected), args)
