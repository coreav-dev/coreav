import copy
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
sys.path.insert(0, str(ROOT / 'scripts/tests'))
import ci_full as ci
import merge_group_result as reuse
from test_ci_full import FakeApi, HEAD
from test_main_ci_plan import evaluate

TREE = 'd' * 40


def document(name):
    return json.loads(subprocess.check_output(['ruby', '-rjson', '-rpsych', '-e',
        'd=Psych.load_file(ARGV[0]); d["on"] ||= d.delete(true); puts JSON.generate(d)',
        str(ROOT / '.github/workflows' / (name + '.yml'))], text=True))


class Coverage(unittest.TestCase):
    def setUp(self):
        self.api = FakeApi()
        self.name = 'ci-full'
        self.push = self.api.runs[42]
        self.scheduled = dict(self.push, id=900, event='schedule', created_at='2026-11-09T01:00:00Z',
                              head_commit={'id': HEAD, 'tree_id': TREE})
        self.api.runs[900] = self.scheduled
        self.api.jobs[900] = [dict(job, run_id=900, conclusion='skipped', steps=[])
                              for job in self.api.jobs[42]]
        self.gate = {'name': 'ci-full-coverage', 'run_id': 900, 'run_attempt': 1, 'head_sha': HEAD,
                     'status': 'completed', 'conclusion': 'success',
                     'steps': [{'name': 'Covered by run 42 attempt 1', 'status': 'completed',
                                'conclusion': 'success'}]}
        self.api.jobs[900].append(self.gate)
        self.index = [self.push]
        self.fresh_index = None
        self.index_reads = 0
        self.paths = []
        self.env = {'GITHUB_EVENT_NAME': 'schedule', 'GITHUB_REF': 'refs/heads/main',
                    'GITHUB_REPOSITORY': ci.REPOSITORY, 'GITHUB_SHA': HEAD,
                    'GITHUB_RUN_ID': '900', 'GITHUB_RUN_ATTEMPT': '1'}

    def request(self, path):
        self.paths.append(path)
        if '/workflows/' in path:
            self.index_reads += 1
            runs = self.fresh_index if self.index_reads > 1 and self.fresh_index is not None else self.index
            return {'workflow_runs': copy.deepcopy(runs), 'total_count': len(runs)}
        if path.endswith('/runs/900'):
            return copy.deepcopy(self.scheduled)
        if '/jobs?' in path:
            jobs = self.api.jobs[42]
            return {'jobs': copy.deepcopy(jobs), 'total_count': len(jobs)}
        return copy.deepcopy(self.push)

    def lookup(self):
        return reuse.full_result(self.env, self.request, TREE, 'ci-full.yml', ci)

    def test_covered_push_has_no_age_bound_and_names_its_attempt(self):
        self.assertEqual(self.lookup()[0], (42, 1))
        self.assertIn('/actions/runs/42/attempts/1', self.lookup()[1])
        self.assertIn('full suite skipped', self.lookup()[1])
        self.assertNotIn('\n', self.lookup()[1])
        self.assertTrue(all('created=' not in path for path in self.paths))
        self.assertEqual(ci.report(self.api.event(900), self.api), [])
        self.assertFalse(any(method in ('POST', 'PATCH', 'DELETE') for method, *_ in self.api.calls))

    def test_every_ineligible_push_falls_back(self):
        changes = [('event', 'pull_request'), ('event', 'merge_group'), ('event', 'workflow_dispatch'),
                   ('head_sha', 'b' * 40), ('head_branch', 'topic'), ('path', '.github/workflows/other.yml'),
                   ('name', 'other'), ('workflow_id', 999), ('head_repository', {'full_name': 'fork/repo'}),
                   ('repository', {'full_name': 'fork/repo'}), ('status', 'in_progress'),
                   ('status', 'queued'), ('conclusion', 'failure'), ('conclusion', 'cancelled'),
                   ('conclusion', 'timed_out'), ('created_at', '2026-11-10T01:00:00Z')]
        for key, value in changes:
            with self.subTest(key=key, value=value):
                self.setUp()
                self.push[key] = value
                self.assertIsNone(self.lookup()[0])

    def test_missing_newer_failed_rerun_and_truncated_indexes_do_not_cover(self):
        self.index = []
        self.assertIsNone(self.lookup()[0])
        self.setUp()
        self.index.append(dict(self.push, id=43, created_at='2026-11-08T01:00:00Z', conclusion='failure'))
        self.assertIsNone(self.lookup()[0])
        self.setUp()
        self.fresh_index = [dict(self.push, run_attempt=2, status='in_progress', conclusion=None)]
        self.assertIsNone(self.lookup()[0])
        self.setUp()
        self.index *= 21
        self.assertIsNone(self.lookup()[0])
        self.setUp()
        original = self.request
        def truncated(path):
            data = original(path)
            if 'total_count' in data:
                data['total_count'] += 1
            return data
        with mock.patch.object(self, 'request', side_effect=truncated):
            self.assertIsNone(self.lookup()[0])

    def test_every_full_job_and_proof_is_required_but_measurement_jobs_are_not(self):
        original = copy.deepcopy(self.api.jobs[42])
        for mutation in ('missing', 'duplicate', 'skipped', 'wrong_attempt', 'skipped_proof'):
            with self.subTest(mutation=mutation):
                jobs = copy.deepcopy(original)
                if mutation == 'missing': jobs = [job for job in jobs if job['name'] != 'transcode-compat-shard (2)']
                elif mutation == 'duplicate': jobs.append(copy.deepcopy(jobs[0]))
                elif mutation == 'skipped': jobs[0]['conclusion'] = 'skipped'
                elif mutation == 'wrong_attempt': jobs[0]['run_attempt'] = 2
                else: jobs[0]['steps'][0]['conclusion'] = 'skipped'
                self.api.jobs[42] = jobs
                self.assertIsNone(self.lookup()[0])
        self.api.jobs[42] = original + [dict(original[0], name='measurement-only', conclusion='skipped', steps=[])]
        self.assertEqual(self.lookup()[0], (42, 1))

    def test_claim_must_be_unique_successful_exact_and_all_other_jobs_skipped(self):
        for mutation in ('missing', 'duplicate', 'skipped', 'failed', 'extra_text', 'wrong_type', 'ran_full',
                         'failed_schedule', 'failed_gate', 'duplicate_gate'):
            with self.subTest(mutation=mutation):
                self.setUp()
                steps = self.gate['steps']
                if mutation == 'missing': steps.clear()
                elif mutation == 'duplicate': steps.append(copy.deepcopy(steps[0]))
                elif mutation == 'skipped': steps[0]['conclusion'] = 'skipped'
                elif mutation == 'failed': steps[0]['conclusion'] = 'failure'
                elif mutation == 'extra_text': steps[0]['name'] += ' SECRET'
                elif mutation == 'wrong_type': steps[0]['name'] = None
                elif mutation == 'ran_full': self.api.jobs[900][0]['conclusion'] = 'success'
                elif mutation == 'failed_schedule': self.scheduled['conclusion'] = 'failure'
                elif mutation == 'failed_gate': self.gate['conclusion'] = 'failure'
                else: self.api.jobs[900].append(copy.deepcopy(self.gate))
                self.assertFalse(ci.covered_schedule(self.api, self.name, self.scheduled, self.api.jobs[900]))
                self.assertTrue(ci.report(self.api.event(900), self.api))
                self.assertNotIn('SECRET', json.dumps(self.api.issues))

    def test_claim_rechecks_the_pinned_attempt_not_a_later_rerun(self):
        self.assertTrue(ci.covered_schedule(self.api, self.name, self.scheduled, self.api.jobs[900]))
        self.assertIn(('GET', '/repos/' + ci.REPOSITORY + '/actions/runs/42/attempts/1', None), self.api.calls)
        self.scheduled['event'] = 'push'
        self.assertFalse(ci.covered_schedule(self.api, self.name, self.scheduled, self.api.jobs[900]))

    def test_api_failure_does_not_write_an_issue_or_skip_full_work(self):
        with mock.patch.object(self, 'request', side_effect=OSError('SECRET')):
            result, message = self.lookup()
            self.assertIsNone(result)
            self.assertNotIn('SECRET', message)
        with mock.patch.object(self.api, 'request', side_effect=ci.Failure('api_transport_error')):
            with self.assertRaises(ci.Failure):
                ci.covered_schedule(self.api, self.name, self.scheduled, self.api.jobs[900])
        self.assertEqual(self.api.issues, [])

    def test_full_push_pr_merge_group_and_dispatch_never_reuse(self):
        for event in ('push', 'pull_request', 'merge_group', 'workflow_dispatch'):
            with self.subTest(event=event), mock.patch.object(self, 'request') as request:
                self.env['GITHUB_EVENT_NAME'] = event
                self.assertIsNone(self.lookup()[0])
                request.assert_not_called()

    def test_default_and_explicit_manual_suites_are_independent_of_push_coverage(self):
        self.assertEqual(ci.selected_suites(), ['ci-full', 'vp9-corpus', 'vp9-conformance'])
        self.assertEqual(ci.selected_suites(['hevc-conformance']), ['hevc-conformance'])
        for suites in ([], ['unknown']):
            with self.assertRaises(ci.Failure): ci.selected_suites(suites)
        self.api.manual()
        self.assertEqual(ci.verify(26, self.api, ['hevc-conformance'])[1], [('hevc-conformance', 45, 1)])
        self.assertFalse(any('event=push' in call[1] or 'event=schedule' in call[1] for call in self.api.calls))


class Wiring(unittest.TestCase):
    def test_gates_are_schedule_only_fail_open_and_share_the_registry(self):
        self.assertEqual(set(reuse.FULL_WORKFLOW_FILES), {spec['file'] for spec in ci.FULL_WORKFLOWS.values()})
        for name, spec in ci.FULL_WORKFLOWS.items():
            with self.subTest(workflow=name):
                workflow = document(name)
                self.assertEqual(workflow['name'], name)
                gate = workflow['jobs']['full-coverage']
                self.assertEqual(gate['name'], name + '-coverage')
                self.assertEqual(gate['if'], "github.event_name == 'schedule'")
                self.assertEqual(gate['permissions'], {'contents': 'read', 'actions': 'read'})
                self.assertEqual(gate['outputs']['reuse'], "${{ steps.result.outputs.reuse == 'true' }}")
                self.assertTrue(gate['steps'][0]['continue-on-error'])
                action = gate['steps'][1]
                self.assertEqual(action['uses'], './.github/actions/merge-group-result')
                self.assertEqual(action['with']['workflow'], spec['file'])
                self.assertTrue(action['continue-on-error'])
                self.assertEqual(gate['steps'][2]['name'], 'Covered by run ${{ steps.result.outputs.run }} attempt ${{ steps.result.outputs.attempt }}')
                self.assertEqual(gate['steps'][2]['run'], 'true')

    def test_all_scheduled_work_is_gated_and_descendants_require_success(self):
        for name in ci.FULL_WORKFLOWS:
            jobs = document(name)['jobs']
            for job_id, job in jobs.items():
                if job_id == 'full-coverage':
                    continue
                if name == 'vp9-conformance' and job_id in ('build-round', 'pixel-ceiling'):
                    self.assertIn("github.event_name == 'workflow_dispatch'", job['if'])
                    continue
                condition = job.get('if', '')
                needs = job.get('needs', [])
                needs = [needs] if isinstance(needs, str) else needs
                with self.subTest(workflow=name, job=job_id):
                    if name == 'hevc-conformance' and job_id in ('headers', 'hevc-smoke'):
                        self.assertEqual(needs, ['full-coverage'])
                        self.assertEqual(condition, "!cancelled() && needs.full-coverage.outputs.reuse != 'true'")
                    if 'always()' in condition:
                        self.assertIn('full-coverage', needs)
                        self.assertIn("needs.full-coverage.outputs.reuse != 'true'", condition)
                    elif needs == ['full-coverage']:
                        self.assertIn('!cancelled()', condition)
                        self.assertIn("needs.full-coverage.outputs.reuse != 'true'", condition)
                        for covered, cancelled in ((True, False), (False, False), (False, True)):
                            context = {'cancelled': cancelled, 'github': {'event_name': 'schedule'},
                                       'inputs': {'mode': 'conformance'},
                                       'needs': {'full-coverage': {'outputs': {'reuse': str(covered).lower()}}}}
                            self.assertEqual(bool(evaluate(condition, context)), not covered and not cancelled)
                    else:
                        self.assertTrue(needs)
                        self.assertNotIn('full-coverage', needs)
                        self.assertIn('!cancelled()', condition)
                        for parent in needs:
                            self.assertIn(f"needs.{parent}.result == 'success'", condition)

    def test_skipped_gate_outputs_do_not_skip_normal_event_roots(self):
        for name in ci.FULL_WORKFLOWS:
            workflow = document(name)
            for job_id, job in workflow['jobs'].items():
                if job.get('needs') != 'full-coverage':
                    continue
                for event in ('push', 'pull_request', 'workflow_dispatch'):
                    if event not in workflow['on']:
                        continue
                    context = {'github': {'event_name': event, 'ref': 'refs/heads/main'},
                               'inputs': {'mode': 'conformance'},
                               'needs': {'full-coverage': {'result': 'skipped', 'outputs': {'reuse': ''}}}}
                    with self.subTest(workflow=name, job=job_id, event=event):
                        expected = not (name == 'hevc-conformance' and event == 'pull_request'
                                        and job_id in ('hevc-vectors', 'profile-census'))
                        self.assertEqual(bool(evaluate(job['if'], context)), expected)


if __name__ == '__main__':
    unittest.main()
