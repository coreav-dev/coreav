"""Run the R4 rehearsal step with stand-in reports, without media or network access."""
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


def run_script(name):
    workflow = (ROOT / '.github/workflows/compat-0.1.0.yml').read_text()
    step = workflow.split(f'      - name: {name}\n', 1)[1]
    block = step.split('        run: |\n', 1)[1].split('\n      - ', 1)[0]
    return '\n'.join(line[10:] for line in block.splitlines())


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


class Shards(unittest.TestCase):
    def test_a_crashed_report_without_a_summary_fails_the_shard(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = pathlib.Path(tmp)
            (d / 'private').mkdir()
            fake = d / 'crash.py'
            fake.write_text('import os, pathlib, sys\n'
                            'if "run" in sys.argv:\n'
                            '    (pathlib.Path(os.environ["RUNNER_TEMP"]) / "private/r3.jsonl").write_text("{}\\n")\n'
                            'sys.exit(1)\n')
            script = run_script('R3, the copy outputs').replace('tools/compat-corpus/r3.py', str(fake))
            done = subprocess.run(['bash', '-c', script], env={**os.environ, 'RUNNER_TEMP': tmp},
                                  capture_output=True, timeout=30)
            self.assertNotEqual(done.returncode, 0)
            self.assertIn(b'R3 produced no summary', done.stdout)

    def test_shards_keep_missed_and_void_results_with_summaries(self):
        for step in ('R2, the keys of the set', 'R3, the copy outputs'):
            for code in (1, 2):
                with self.subTest(step=step, code=code), tempfile.TemporaryDirectory() as tmp:
                    d = pathlib.Path(tmp)
                    (d / 'bin').mkdir()
                    (d / 'private').mkdir()
                    for binary in ('coreav', 'r3-harness'):
                        (d / 'bin' / binary).touch()
                    fake = d / 'result.py'
                    fake.write_text('import os, pathlib, sys\n'
                                    'for flag in ("--records", "--summary"):\n'
                                    '    if flag in sys.argv:\n'
                                    '        pathlib.Path(sys.argv[sys.argv.index(flag) + 1]).write_text("{}\\n")\n'
                                    'sys.exit(int(os.environ["FAKE_EXIT"]))\n')
                    script = run_script(step).replace('tools/compat-corpus/run.py', str(fake))
                    script = script.replace('tools/compat-corpus/r3.py', str(fake))
                    done = subprocess.run(['bash', '-c', script], env={**os.environ, 'RUNNER_TEMP': tmp,
                                          'FAKE_EXIT': str(code)}, capture_output=True, timeout=30)
                    self.assertEqual(done.returncode, 0, done.stderr.decode())

    def test_crashed_commands_without_summaries_fail_the_shard(self):
        for step in ('R2, the keys of the set', 'R3, the copy outputs'):
            with self.subTest(step=step), tempfile.TemporaryDirectory() as tmp:
                d = pathlib.Path(tmp)
                (d / 'bin').mkdir()
                for binary in ('coreav', 'r3-harness'):
                    (d / 'bin' / binary).touch()
                fake = d / 'crash.py'
                fake.write_text('import sys\nsys.exit(1)\n')
                script = run_script(step).replace('tools/compat-corpus/run.py', str(fake))
                script = script.replace('tools/compat-corpus/r3.py', str(fake))
                done = subprocess.run(['bash', '-c', script], env={**os.environ, 'RUNNER_TEMP': tmp},
                                      capture_output=True, timeout=30)
                self.assertNotEqual(done.returncode, 0)

    def test_aggregation_requires_exactly_all_nonempty_shard_summaries(self):
        for fault in ('missing-shard', 'missing-summary', 'empty-summary', 'extra-shard'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                d = pathlib.Path(tmp)
                for shard in range(4):
                    directory = d / 'shards' / f'compat-shard-{shard}'
                    directory.mkdir(parents=True)
                    for kind in ('r2', 'r3'):
                        (directory / f'{kind}-summary.json').write_text('{}')
                if fault == 'missing-shard':
                    shutil.rmtree(d / 'shards' / 'compat-shard-3')
                elif fault == 'empty-summary':
                    (d / 'shards' / 'compat-shard-2' / 'r3-summary.json').write_text('')
                elif fault == 'missing-summary':
                    (d / 'shards' / 'compat-shard-2' / 'r2-summary.json').unlink()
                else:
                    (d / 'shards' / 'compat-shard-4').mkdir()
                fake = d / 'merge.py'
                fake.write_text('import os, pathlib\n'
                                '(pathlib.Path(os.environ["RUNNER_TEMP"]) / "merged").touch()\n')
                script = run_script('Add up the shards').replace('tools/compat-corpus/run.py', str(fake))
                script = script.replace('tools/compat-corpus/r3.py', str(fake))
                done = subprocess.run(['bash', '-c', script], env={**os.environ, 'RUNNER_TEMP': tmp,
                                      'MEASURE': 'success', 'SHARDS': '4'}, capture_output=True, timeout=30)
                self.assertNotEqual(done.returncode, 0)
                self.assertFalse((d / 'merged').exists())

    def test_complete_shards_are_aggregated_once_each(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = pathlib.Path(tmp)
            for shard in range(4):
                directory = d / 'shards' / f'compat-shard-{shard}'
                directory.mkdir(parents=True)
                for kind in ('r2', 'r3'):
                    (directory / f'{kind}-summary.json').write_text('{}')
            (d / 'bin').mkdir()
            (d / 'bin' / 'sha256.txt').write_text('abc coreav\n')
            fake = d / 'merge.py'
            fake.write_text('import json, pathlib, sys\n'
                            'inputs = sys.argv[sys.argv.index("--summaries") + 1:sys.argv.index("--summary")]\n'
                            'assert len(inputs) == len(set(inputs)) == 4\n'
                            'summary = {"fallback_log": {}, "unmeasured": [], "families": {},\n'
                            '           "files": {}, "taken_by_no_file": [], "commands": []}\n'
                            'pathlib.Path(sys.argv[sys.argv.index("--summary") + 1]).write_text(json.dumps(summary))\n'
                            'sys.exit(1 if "keys-report" in sys.argv else 2)\n')
            script = run_script('Add up the shards').replace('tools/compat-corpus/run.py', str(fake))
            script = script.replace('tools/compat-corpus/r3.py', str(fake))
            done = subprocess.run(['bash', '-c', script], env={**os.environ, 'RUNNER_TEMP': tmp,
                                  'MEASURE': 'success', 'SHARDS': '4', 'SET': 'rehearsal', 'COMMIT': 'test',
                                  'GITHUB_STEP_SUMMARY': str(d / 'summary.md')}, capture_output=True, timeout=30)
            self.assertEqual(done.returncode, 0, done.stderr.decode())
            self.assertTrue((d / 'summary' / 'r2.json').is_file())
            self.assertTrue((d / 'summary' / 'r3.json').is_file())


if __name__ == '__main__':
    unittest.main()
