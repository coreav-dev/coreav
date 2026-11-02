"""Run the R4 rehearsal step with stand-in reports, without media or network access."""
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


def run_script(name, file='compat-0.1.0.yml', occurrence=0):
    workflow = (ROOT / '.github/workflows' / file).read_text()
    step = workflow.split(f'      - name: {name}\n')[occurrence + 1]
    block = step.split('        run: |\n', 1)[1].split('\n      - ', 1)[0]
    return '\n'.join(line[10:] for line in block.splitlines())


class Rehearsal(unittest.TestCase):
    def driver(self, directory):
        fake = directory / 'driver.py'
        fake.write_text('import json, os, pathlib, sys\n'
                        'out = pathlib.Path(os.environ["RUNNER_TEMP"]) / "r4"\n'
                        '(out / "vm").mkdir(parents=True)\n'
                        'reason = os.environ["FAKE_REASON"]\n'
                        '(out / "verdict.json").write_text(json.dumps({"verdict": "void" if os.environ["FAKE_EXIT"] == "5" else "r4",\n'
                        '                                             "reasons": [reason] if reason else []}))\n'
                        'reports = json.loads(os.environ.get("FAKE_REPORTS", "{}"))\n'
                        'for name in ("gate-aa", "gate-null", "r4"):\n'
                        '    (out / "vm" / (name + ".report.json")).write_text(json.dumps(reports.get(name, {})))\n'
                        '    (out / "vm" / (name + ".report.txt")).write_text("report")\n'
                        '(out / "vm" / "ffmpeg-version.txt").write_text("test oracle")\n'
                        'sys.exit(int(os.environ["FAKE_EXIT"]))\n')
        return fake

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
                fake = self.driver(d)
                done = subprocess.run(['bash', '-c', script.replace('scripts/compat_r4_gcp.py', str(fake))],
                                      env={**os.environ, 'RUNNER_TEMP': tmp, 'FAKE_EXIT': str(code),
                                           'FAKE_REASON': reason}, capture_output=True, timeout=30)
                self.assertEqual(done.returncode == 0, accepted, done.stderr.decode())
                public = d / 'r4-public'
                self.assertTrue((public / 'verdict.json').is_file())
                self.assertEqual(json.loads((public / 'verdict.json').read_text())['reasons'],
                                 [reason] if reason else [])
                self.assertEqual(len(list(public.iterdir())), 8)

    def test_only_coverage_proved_refusal_groups_are_expected_unmeasured_diagnostics(self):
        base = {'class': 'remux', 'family': 'all', 'eligible': 2, 'refused': 2,
                'timed': 0, 'failed': 0, 'oracle_failed': 0, 'unprocessed': 0}
        for row, accepted in ((base, True), ({**base, 'eligible': 3}, False),
                              ({**base, 'failed': 1}, False), ({**base, 'oracle_failed': 1}, False),
                              ({**base, 'timed': 1}, False), ({**base, 'eligible': 0, 'refused': 0}, False),
                              ({**base, 'family': 'mp4-mov'}, False), ({**base, 'unprocessed': 1}, False)):
            with self.subTest(row=row), tempfile.TemporaryDirectory() as tmp:
                directory = pathlib.Path(tmp)
                (directory / 'bin').mkdir()
                (directory / 'bin' / 'coreav').touch()
                fake = self.driver(directory)
                reports = {'gate-null': {'unmeasured': ['remux all'], 'coverage_groups': [row]}}
                script = run_script("Run the R4 host's job here").replace('scripts/compat_r4_gcp.py', str(fake))
                done = subprocess.run(['bash', '-c', script],
                                      env={**os.environ, 'RUNNER_TEMP': tmp, 'FAKE_EXIT': '5',
                                           'FAKE_REASON': 'gate-null left remux all unmeasured',
                                           'FAKE_REPORTS': json.dumps(reports)}, capture_output=True, timeout=30)
                self.assertEqual(done.returncode == 0, accepted, done.stderr.decode())
                self.assertEqual(len(list((directory / 'r4-public').iterdir())), 8)

    def test_oracle_failed_groups_keep_their_void_rehearsal_diagnostic(self):
        for refused, oracle_failed in ((0, 2), (1, 1)):
            with self.subTest(refused=refused), tempfile.TemporaryDirectory() as tmp:
                directory = pathlib.Path(tmp)
                (directory / 'bin').mkdir()
                (directory / 'bin' / 'coreav').touch()
                fake = self.driver(directory)
                row = {'class': 'remux', 'family': 'matroska-webm', 'eligible': 2,
                       'refused': refused, 'oracle_failed': oracle_failed, 'timed': 0, 'failed': 0, 'unprocessed': 0}
                reports = {'r4': {'unmeasured': ['remux matroska-webm'], 'coverage_groups': [row]}}
                script = run_script("Run the R4 host's job here").replace('scripts/compat_r4_gcp.py', str(fake))
                done = subprocess.run(['bash', '-c', script],
                                      env={**os.environ, 'RUNNER_TEMP': tmp, 'FAKE_EXIT': '5',
                                           'FAKE_REASON': 'r4 left remux matroska-webm unmeasured',
                                           'FAKE_REPORTS': json.dumps(reports)}, capture_output=True, timeout=30)
                self.assertEqual(done.returncode, 0, done.stderr.decode())
                public = json.loads((directory / 'r4-public/verdict.json').read_text())
                self.assertEqual(public['verdict'], 'void')


class Evidence(unittest.TestCase):
    def test_r3_auxiliary_oracles_resolve_the_pinned_programs_on_path(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = pathlib.Path(tmp)
            (d / 'private').mkdir()
            for directory, label in (('oracle', 'pinned'), ('other', 'other')):
                (d / directory).mkdir()
                for name in ('ffmpeg', 'ffprobe'):
                    program = d / directory / name
                    program.write_text(f'#!/bin/sh\nprintf "{label}\\n"\n')
                    program.chmod(0o755)
            runner = d / 'r3.py'
            runner.write_text('import os, pathlib, shutil, subprocess, sys\n'
                              'root = pathlib.Path(os.environ["RUNNER_TEMP"])\n'
                              'for name in ("ffmpeg", "ffprobe"):\n'
                              '    assert shutil.which(name) == str(root / "oracle" / name), "R3 did not prepend the pinned oracle directory"\n'
                              '    assert subprocess.check_output([name], text=True) == "pinned\\n"\n'
                              'flag = "--records" if sys.argv[1] == "run" else "--summary"\n'
                              'pathlib.Path(sys.argv[sys.argv.index(flag) + 1]).write_text("{}\\n")\n')
            script = run_script('R3, the copy outputs').replace('tools/compat-corpus/r3.py', str(runner))
            done = subprocess.run(['bash', '-e', '-c', script],
                                  env={**os.environ, 'RUNNER_TEMP': tmp,
                                       'PATH': str(d / 'other') + ':' + os.environ['PATH']},
                                  capture_output=True, timeout=10)
            self.assertEqual(done.returncode, 0, done.stderr.decode())

    def test_r1_uses_the_registered_profile_and_records_its_binary_hash(self):
        for index, stem in enumerate(('coreav', 'mkv', 'ts')):
            with self.subTest(stem=stem), tempfile.TemporaryDirectory() as tmp:
                d = pathlib.Path(tmp)
                (d / 'bin').mkdir()
                cargo = d / 'bin' / 'cargo'
                cargo.write_text('#!/bin/sh\nset -e\nprintf "%s\\n" "$*" > cargo.args\n'
                                 'mkdir -p target/dist\nprintf binary > target/dist/coreav\n')
                cargo.chmod(0o755)
                runner = d / 'runner.py'
                runner.write_text('import pathlib, sys\n'
                                  'assert sys.argv[sys.argv.index("--coreav") + 1] == "target/dist/coreav"\n'
                                  'pathlib.Path(sys.argv[sys.argv.index("--report") + 1]).write_text("{}")\n')
                script = run_script('R1 of the 0.1.0 round', 'ci.yml', index)
                script = script.replace('tools/compat-corpus/run.py', str(runner))
                done = subprocess.run(['bash', '-e', '-c', script], cwd=d,
                                      env={**os.environ, 'RUNNER_TEMP': tmp, 'PATH': str(d / 'bin') + ':' + os.environ['PATH'],
                                           'GITHUB_STEP_SUMMARY': str(d / 'summary.md')},
                                      capture_output=True, timeout=30)
                self.assertEqual(done.returncode, 0, done.stderr.decode())
                self.assertEqual((d / 'cargo.args').read_text().strip(), 'build --locked --profile dist -p coreav-cli')
                digest = hashlib.sha256(b'binary').hexdigest()
                self.assertIn(digest, (d / f'{stem}-r1.sha256').read_text())
                self.assertIn(digest, (d / 'summary.md').read_text())
                self.assertIn(f'${{{{ runner.temp }}}}/{stem}-r1.sha256',
                              (ROOT / '.github/workflows/ci.yml').read_text())

    def test_a_crashed_scan_cannot_omit_either_summary(self):
        for mode, accepted in (('none', False), ('first-only', False), ('all', True)):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as tmp:
                d = pathlib.Path(tmp)
                (d / 'bin').mkdir()
                (d / 'bin' / 'coreav').touch()
                fake = d / 'scan.py'
                fake.write_text('import json, os, pathlib, sys\n'
                                'if "--dest" in sys.argv:\n'
                                '    dest = pathlib.Path(sys.argv[sys.argv.index("--dest") + 1])\n'
                                '    dest.mkdir(parents=True)\n'
                                '    (dest / "files.json").write_text("[]")\n'
                                '    sys.exit(0)\n'
                                'out = pathlib.Path(sys.argv[sys.argv.index("--out") + 1])\n'
                                'if "select" in sys.argv:\n'
                                '    out.write_text("[]")\n'
                                '    sys.exit(0)\n'
                                'if os.environ["MODE"] == "all" or (os.environ["MODE"] == "first-only" and out.stem == "r4-gate"):\n'
                                '    out.write_text(json.dumps({"files": {}, "pairs": 1, "pairs logged": 1,\n'
                                '          "pairs by cause": {}, "commands logged": [], "commands": {}}))\n'
                                'sys.exit(1)\n')
                script = run_script('Scan the R4 pairs').replace('tools/compat-corpus/round_files.py', str(fake))
                script = script.replace('tools/compat-corpus/r4_scan.py', str(fake))
                done = subprocess.run(['bash', '-c', script], env={**os.environ, 'RUNNER_TEMP': tmp, 'MODE': mode,
                                      'GITHUB_STEP_SUMMARY': str(d / 'summary.md')}, capture_output=True, timeout=30)
                self.assertEqual(done.returncode == 0, accepted, done.stderr.decode())
                if accepted:
                    self.assertEqual(len(list((d / 'scan').glob('*.json'))), 2)

    def test_measurement_checkouts_do_not_persist_credentials(self):
        workflow = (ROOT / '.github/workflows/compat-0.1.0.yml').read_text()
        checkouts = workflow.split('      - uses: actions/checkout@')[1:]
        self.assertEqual(len(checkouts), 6)
        for checkout in checkouts:
            self.assertIn('          persist-credentials: false', checkout.split('\n      - ', 1)[0])


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
