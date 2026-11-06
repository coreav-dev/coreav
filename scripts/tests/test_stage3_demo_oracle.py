import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class OracleSelection(unittest.TestCase):
    def test_generation_uses_the_resolver_program_before_entering_the_sandbox(self):
        script = Path(__file__).resolve().parents[2] / 'tools/stage3-report/demo.sh'
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            requested, qualified, tools = (root / name for name in ('requested', 'qualified', 'tools'))
            for folder in (requested, qualified, tools):
                folder.mkdir()
            log = root / 'executions'

            def program(path, body):
                path.write_text('#!/bin/bash\n' + body)
                path.chmod(0o755)

            program(requested / 'ffmpeg', 'echo unqualified >> "$TEST_LOG"; exit 99\n')
            program(qualified / 'ffmpeg', 'echo qualified >> "$TEST_LOG"; for arg in "$@"; do out="$arg"; done; : > "$out"\n')
            program(tools / 'python3', 'printf "%s\\n" "$TEST_FFMPEG" "$TEST_FFPROBE"\n')
            program(tools / 'unshare', 'exit 1\n')
            program(tools / 'sha256sum', 'echo synthetic\n')
            env = {**os.environ, 'PATH': str(tools) + os.pathsep + os.environ['PATH'],
                   'COREAV': '/usr/bin/true', 'FFMPEG': str(requested / 'ffmpeg'), 'TEST_LOG': str(log),
                   'TEST_FFMPEG': str(qualified / 'ffmpeg'), 'TEST_FFPROBE': str(qualified / 'ffprobe')}
            done = subprocess.run(['/bin/bash', str(script)], cwd=root, env=env, capture_output=True, text=True)
            self.assertEqual(done.returncode, 3, done.stderr)
            self.assertEqual(log.read_text().splitlines(), ['qualified'])
