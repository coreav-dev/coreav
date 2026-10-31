"""Run the R4 rehearsal step with stand-in reports, without media or network access."""
import json
import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


class Rehearsal(unittest.TestCase):
    def test_reports_survive_a_rejected_rehearsal(self):
        workflow = (ROOT / '.github/workflows/compat-0.1.0.yml').read_text()
        step = workflow.split("      - name: Run the R4 host's job here\n", 1)[1]
        block = step.split('        run: |\n', 1)[1].split('\n      - ', 1)[0]
        script = '\n'.join(line[10:] for line in block.splitlines())
        for code, reason, accepted in ((0, '', True), (5, 'the fallback log of gate-null is void', True),
                                       (5, 'the gate did not finish (error)', False)):
            with self.subTest(code=code, reason=reason), tempfile.TemporaryDirectory() as tmp:
                d = pathlib.Path(tmp)
                (d / 'bin').mkdir()
                (d / 'bin' / 'coreav').touch()
                fake = d / 'driver.py'
                fake.write_text('import json, os, pathlib, sys\n'
                                'out = pathlib.Path(os.environ["RUNNER_TEMP"]) / "r4"\n'
                                '(out / "vm").mkdir(parents=True)\n'
                                'reason = os.environ["FAKE_REASON"]\n'
                                '(out / "verdict.json").write_text(json.dumps({"reasons": [reason] if reason else []}))\n'
                                'for name in ("gate-aa", "gate-null", "r4"):\n'
                                '    (out / "vm" / (name + ".report.json")).write_text("{}")\n'
                                '    (out / "vm" / (name + ".report.txt")).write_text("report")\n'
                                '(out / "vm" / "ffmpeg-version.txt").write_text("test oracle")\n'
                                'sys.exit(int(os.environ["FAKE_EXIT"]))\n')
                done = subprocess.run(['bash', '-c', script.replace('scripts/compat_r4_gcp.py', str(fake))],
                                      env={**os.environ, 'RUNNER_TEMP': tmp, 'FAKE_EXIT': str(code),
                                           'FAKE_REASON': reason}, capture_output=True, timeout=30)
                self.assertEqual(done.returncode == 0, accepted, done.stderr.decode())
                public = d / 'r4-public'
                self.assertTrue((public / 'verdict.json').is_file())
                self.assertEqual(json.loads((public / 'verdict.json').read_text())['reasons'],
                                 [reason] if reason else [])
                self.assertEqual(len(list(public.iterdir())), 8)


if __name__ == '__main__':
    unittest.main()
