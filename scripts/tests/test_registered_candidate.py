"""Manual measurement workflows must use the candidate registered on main."""
import copy
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
README = pathlib.Path('docs/measurements/compat-0.1.0/README.md')
CANDIDATE = 'a' * 40
REGISTRATION = f'Release candidate: `{CANDIDATE}`.\n'
WORKFLOWS = ('compat-0.1.0.yml', 'codec-coverage.yml')


def load_workflow(name):
    return json.loads(subprocess.check_output([
        'ruby', '-ryaml', '-rjson', '-e', 'puts JSON.generate(YAML.load_file(ARGV[0], aliases: true))',
        str(ROOT / '.github/workflows' / name)], text=True))


class Workflows(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflows = {name: load_workflow(name) for name in WORKFLOWS}

    def run_plan(self, name, commit, registration=REGISTRATION, event='workflow_dispatch', ref='refs/heads/main',
                 selection='rehearsal'):
        workflow = self.workflows[name]
        plan = next(job for key, job in workflow['jobs'].items() if key.endswith('-plan'))
        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp)
            (directory / README).parent.mkdir(parents=True)
            (directory / README).write_text(registration)
            (directory / 'scripts').mkdir()
            guard = ROOT / 'scripts/registered_candidate.py'
            if guard.exists():
                shutil.copy(guard, directory / 'scripts')
            output = directory / 'output'
            env = {**os.environ, 'GITHUB_REF': ref, 'EVENT': event, 'SET': selection, 'OPEN': 'true',
                   'COMMIT': commit, 'SHA': 'c' * 40, 'R2_ENDPOINT': 'test', 'R2_ACCESS_KEY_ID': 'test',
                   'R2_SECRET_ACCESS_KEY': 'test', 'R2_BUCKET': 'test', 'GITHUB_OUTPUT': str(output),
                   'GITHUB_STEP_SUMMARY': str(directory / 'summary')}
            candidate = next((s for s in plan['steps'] if s.get('id') == 'candidate'), None)
            if event != 'pull_request' and candidate:
                self.assertEqual(candidate['env']['COMMIT'], '${{ inputs.commit }}')
                checked = subprocess.run(['bash', '-e', '-c', candidate['run']], cwd=directory, env=env,
                                         capture_output=True, text=True, timeout=30)
                if checked.returncode:
                    return checked.returncode, output.read_text() if output.exists() else '', checked.stderr, False
                env['COMMIT'] = dict(line.split('=', 1) for line in output.read_text().splitlines())['commit']
                output.unlink()
            step = next(s for s in plan['steps'] if s.get('id') == 'plan')
            script = step['run'] + '\ntouch measurement-started\n'
            done = subprocess.run(['bash', '-e', '-c', script], cwd=directory, env=env,
                                  capture_output=True, text=True, timeout=30)
            return (done.returncode, output.read_text() if output.exists() else '', done.stderr,
                    (directory / 'measurement-started').exists())

    def test_omitted_or_mismatched_commits_stop_both_workflows_before_measurement(self):
        for name in WORKFLOWS:
            for selection in ('rehearsal', 'draw6' if name == 'compat-0.1.0.yml' else 'v1'):
                for commit in ('', 'b' * 40):
                    with self.subTest(workflow=name, selection=selection, commit=commit):
                        code, output, error, started = self.run_plan(name, commit, selection=selection)
                        self.assertNotEqual(code, 0, output)
                        self.assertNotIn('commit=', output)
                        self.assertFalse(started)

    def test_only_the_registered_commit_on_main_can_start_manual_measurement(self):
        for name in WORKFLOWS:
            with self.subTest(workflow=name):
                for selection in ('rehearsal', 'draw6' if name == 'compat-0.1.0.yml' else 'v1'):
                    code, output, error, started = self.run_plan(name, CANDIDATE, selection=selection)
                    self.assertEqual(code, 0, error)
                    self.assertIn(f'commit={CANDIDATE}\n', output)
                    self.assertTrue(started)
                for registration, ref in (('', 'refs/heads/main'), (REGISTRATION, 'refs/heads/other')):
                    code, output, error, started = self.run_plan(name, CANDIDATE, registration, ref=ref)
                    self.assertNotEqual(code, 0, output)
                    self.assertNotIn('commit=', output)
                    self.assertFalse(started)

    def test_pull_requests_plan_their_own_head_without_a_registration(self):
        for name in WORKFLOWS:
            with self.subTest(workflow=name):
                code, output, error, started = self.run_plan(name, '', '', event='pull_request', ref='refs/pull/1/merge')
                self.assertEqual(code, 0, error)
                self.assertIn(f"commit={'c' * 40}\n", output)
                self.assertTrue(started)

    def assert_gated(self, workflow):
        jobs = workflow['jobs']
        key = next(key for key in jobs if key.endswith('-plan'))
        steps = jobs[key]['steps']
        checkout = next(s for s in steps if s.get('uses', '').startswith('actions/checkout@'))
        guard = next(s for s in steps if s.get('id') == 'candidate')
        plan = next(s for s in steps if s.get('id') == 'plan')
        self.assertLess(steps.index(checkout), steps.index(guard))
        self.assertLess(steps.index(guard), steps.index(plan))
        self.assertEqual(checkout['if'], "github.event_name != 'pull_request'")
        self.assertEqual(guard['if'], checkout['if'])
        self.assertNotIn('ref', checkout.get('with', {}))
        self.assertFalse(checkout['with']['persist-credentials'])
        self.assertIn(str(README), checkout['with']['sparse-checkout'])
        self.assertIn('scripts/registered_candidate.py', checkout['with']['sparse-checkout'])
        self.assertEqual(plan['env']['COMMIT'], '${{ steps.candidate.outputs.commit }}')
        self.assertEqual(guard['env']['COMMIT'], '${{ inputs.commit }}')
        for name, job in jobs.items():
            if name != key:
                needs = job.get('needs', [])
                self.assertIn(key, [needs] if isinstance(needs, str) else needs)
                if 'always()' in job.get('if', ''):
                    self.assertIn(f"needs.{key}.result == 'success'", job['if'])
            for step in job['steps']:
                if step is not guard:
                    self.assertNotIn('inputs.commit', json.dumps(step))
                if step.get('uses', '').startswith('actions/checkout@') and step is not checkout:
                    self.assertEqual(step['with']['ref'], '${{ needs.' + key + '.outputs.commit }}')

    def test_every_measurement_job_is_gated_by_the_validated_plan(self):
        for name, workflow in self.workflows.items():
            with self.subTest(workflow=name):
                self.assert_gated(workflow)

    def test_the_gate_check_rejects_a_bypass_in_either_workflow(self):
        for name, workflow in self.workflows.items():
            for mutation in ('needs', 'raw_input', 'registration_ref', 'report'):
                with self.subTest(workflow=name, mutation=mutation):
                    broken = copy.deepcopy(workflow)
                    plan = next(job for key, job in broken['jobs'].items() if key.endswith('-plan'))
                    build = next(job for key, job in broken['jobs'].items() if key.endswith('-build'))
                    if mutation == 'needs':
                        del build['needs']
                    elif mutation == 'raw_input':
                        next(s for s in plan['steps'] if s.get('id') == 'plan')['env']['COMMIT'] = '${{ inputs.commit }}'
                    elif mutation == 'registration_ref':
                        plan['steps'][0]['with']['ref'] = '${{ inputs.commit }}'
                    else:
                        report = next(job for key, job in broken['jobs'].items() if key.endswith('-report'))
                        report['if'] = 'always()'
                    with self.assertRaises(AssertionError):
                        self.assert_gated(broken)


class Candidate(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        sys.path.insert(0, str(ROOT / 'scripts'))
        import registered_candidate
        cls.guard = registered_candidate

    def test_an_exact_full_sha_is_required(self):
        self.assertEqual(self.guard.allowed(REGISTRATION, 'refs/heads/main', CANDIDATE), CANDIDATE)
        for commit in ('', 'b' * 40, CANDIDATE[:12], 'A' * 40, CANDIDATE + '\n', 'main'):
            with self.subTest(commit=commit), self.assertRaises(ValueError):
                self.guard.allowed(REGISTRATION, 'refs/heads/main', commit)

    def test_registration_is_unique_and_well_formed(self):
        for text in ('', REGISTRATION * 2, REGISTRATION.replace(CANDIDATE, CANDIDATE[:12]),
                     REGISTRATION.replace(CANDIDATE, 'A' * 40), REGISTRATION.replace('`.', '`')):
            with self.subTest(text=text), self.assertRaises(ValueError):
                self.guard.registered(text)

    def test_other_refs_do_not_authorize_their_own_readme(self):
        for ref in ('', 'refs/tags/main', 'refs/heads/other', 'refs/pull/1/merge'):
            with self.subTest(ref=ref), self.assertRaises(ValueError):
                self.guard.allowed(REGISTRATION, ref, CANDIDATE)

    def test_the_round_readme_is_unregistered_or_has_one_exact_candidate(self):
        text = (ROOT / README).read_text()
        if 'Release candidate:' in text:
            self.guard.registered(text)


if __name__ == '__main__':
    unittest.main()
