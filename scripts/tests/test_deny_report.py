"""The required `deny` check is green only when the job it reports
succeeded.

The `deny` job is the check a ruleset requires, so a `skipped` result has to
fail it: nothing in `deny.yml` is filtered, no job of it has anything to
skip, and a required check that a skipped job passes says nothing about the
commit (docs/design/ci-checks.md, "A job timeout is not a cancellation").
The report jobs of the other required workflows keep passing a skipped job,
because a path filter skips their work by design.
"""
import importlib.util
import json
import os
import pathlib
import subprocess
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SPEC = importlib.util.spec_from_file_location('workflow_check', ROOT / 'scripts' / 'workflow_check.py')
check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)


def report_script():
    """The `run:` script of the step that reports `deny-checks`, as written
    in the workflow file."""
    entries = check.yaml_entries((ROOT / '.github' / 'workflows' / 'deny.yml').read_text(encoding='utf-8'))
    scripts = [entry['run'] for entry in entries if 'run' in entry and 'select(.value.result' in entry['run']]
    if len(scripts) != 1:
        raise AssertionError(f'deny.yml holds {len(scripts)} reporting steps; expected one')
    return scripts[0]


class DenyReport(unittest.TestCase):
    def run_report(self, *results):
        """The report step's exit status for a `needs` mapping of job results,
        and the summary it wrote."""
        needs = {f'job{index}': {'result': result, 'outputs': {}} for index, result in enumerate(results)}
        with tempfile.TemporaryDirectory() as directory:
            summary = pathlib.Path(directory) / 'summary.md'
            run = subprocess.run(['bash', '-c', report_script()],
                                 env=dict(os.environ, RESULTS=json.dumps(needs), GITHUB_STEP_SUMMARY=str(summary)),
                                 capture_output=True, text=True)
            return run, summary.read_text(encoding='utf-8')

    def test_only_a_successful_job_passes_the_required_check(self):
        run, summary = self.run_report('success')
        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
        self.assertIn('| job0 | success |', summary)

    def test_a_job_that_did_not_succeed_fails_the_required_check(self):
        for result in ('failure', 'cancelled', 'skipped'):
            with self.subTest(result=result):
                run, summary = self.run_report(result)
                self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
                self.assertIn('::error::did not succeed: job0', run.stdout)
                self.assertIn(f'| job0 | {result} |', summary)

    def test_every_job_the_report_waits_for_has_to_succeed(self):
        run, _ = self.run_report('success', 'skipped')
        self.assertEqual(run.returncode, 1, run.stdout + run.stderr)
        self.assertIn('::error::did not succeed: job1', run.stdout)


if __name__ == '__main__':
    unittest.main()
