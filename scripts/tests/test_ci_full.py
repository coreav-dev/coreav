import copy
import importlib.util
import io
import json
from pathlib import Path
import re
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('ci_full', ROOT / 'scripts/ci_full.py')
ci = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ci)
STAMP = '2026-11-06T01:00:00Z'
HEAD = 'a' * 40


class FakeApi:
    def __init__(self):
        self.runs = {}
        self.jobs = {}
        self.dispatches = {}
        self.issues = []
        self.comments = {}
        self.calls = []
        self.ambiguous_create = False
        self.race_create = False
        self.fail_lookup = False
        self.changed_head = False
        self.pull_reads = 0
        for index, (name, spec) in enumerate(ci.FULL_WORKFLOWS.items(), 42):
            run = {'id': index, 'run_attempt': 1, 'workflow_id': index + 100, 'name': name, 'path': '.github/workflows/' + spec['file'],
                   'event': 'push', 'head_branch': 'main', 'head_sha': HEAD, 'status': 'completed',
                   'conclusion': 'success', 'created_at': STAMP,
                   'repository': {'full_name': ci.REPOSITORY}, 'head_repository': {'full_name': ci.REPOSITORY}}
            self.runs[index] = run
            self.dispatches[spec['file']] = [copy.deepcopy(run)]
            self.jobs[index] = []
            for names in spec['areas'].values():
                for job in names:
                    self.jobs[index].append({'name': job, 'run_id': index, 'run_attempt': 1, 'head_sha': HEAD,
                                             'status': 'completed', 'conclusion': 'success',
                                             'steps': [{'name': step, 'status': 'completed', 'conclusion': 'success',
                                                        'started_at': STAMP, 'completed_at': STAMP}
                                                       for step in ci.PROOF_STEPS[name, job.split(' (', 1)[0]]]})

    def event(self, run_id=42):
        return {'action': 'completed', 'repository': {'full_name': ci.REPOSITORY},
                'workflow_run': copy.deepcopy(self.runs[run_id])}

    def fail(self, job, run_id=42):
        self.runs[run_id]['conclusion'] = 'failure'
        next(j for j in self.jobs[run_id] if j['name'] == job)['conclusion'] = 'failure'

    def manual(self):
        for run in self.runs.values():
            run.update(event='workflow_dispatch', head_branch='feature')

    def pages(self, path, key=None):
        self.calls.append(('PAGES', path))
        if '/attempts/' in path:
            return copy.deepcopy(self.jobs[int(path.split('/runs/')[1].split('/')[0])])
        if '/workflows/' in path:
            file = path.split('/workflows/')[1].split('/')[0]
            return copy.deepcopy(self.dispatches[file])
        if '/comments' in path:
            return copy.deepcopy(self.comments.get(int(path.split('/issues/')[1].split('/')[0]), []))
        if '/issues?' in path:
            if self.fail_lookup:
                raise ci.Failure('api_http_403')
            return copy.deepcopy([item for item in self.issues if item['state'] == 'open'])
        raise AssertionError(path)

    def request(self, method, path, data=None):
        self.calls.append((method, path, copy.deepcopy(data)))
        if method == 'GET' and '/attempts/' in path:
            return copy.deepcopy(self.runs[int(path.split('/runs/')[1].split('/')[0])])
        if method == 'GET' and '/pulls/' in path:
            self.pull_reads += 1
            sha = 'b' * 40 if self.changed_head and self.pull_reads > 1 else HEAD
            return {'state': 'open', 'head': {'sha': sha, 'ref': 'feature', 'repo': {'full_name': ci.REPOSITORY}}}
        if method == 'POST' and path.endswith('/issues'):
            if self.race_create:
                self.race_create = False
                other = dict(data, body=data['body'].replace('ci-full-run:42:1', 'ci-full-run:777:1'))
                self.create_issue(other)
            item = self.create_issue(data)
            if self.ambiguous_create:
                raise ci.Failure('api_transport_error')
            return copy.deepcopy(item)
        if method == 'POST' and path.endswith('/comments'):
            number = int(path.split('/issues/')[1].split('/')[0])
            comment = dict(data, id=1000 + sum(map(len, self.comments.values())), user={'login': ci.BOT})
            self.comments.setdefault(number, []).append(comment)
            return copy.deepcopy(comment)
        if method == 'PATCH':
            next(item for item in self.issues if item['number'] == int(path.rsplit('/', 1)[1])).update(data)
            return {}
        if method == 'DELETE':
            number = int(path.rsplit('/', 1)[1])
            for comments in self.comments.values():
                comments[:] = [comment for comment in comments if comment['id'] != number]
            return None
        raise AssertionError((method, path))

    def create_issue(self, data):
        item = dict(data, number=len(self.issues) + 1, state='open', user={'login': ci.BOT})
        self.issues.append(item)
        return item


class FailureReporting(unittest.TestCase):
    def test_real_report_path_opens_an_area_issue_and_deduplicates_attempts(self):
        api = FakeApi()
        api.fail('h264-full')
        self.assertEqual(ci.report(api.event(), api), [1])
        issue, = api.issues
        self.assertIn('Owning area: H.264 decoding', issue['body'])
        self.assertIn('within one day', issue['body'])
        self.assertIn('/actions/runs/42/attempts/1', issue['body'])
        self.assertEqual(set(issue['labels']), {'bug', 'area:ci', 'area:h264'})
        self.assertEqual(ci.report(api.event(), api), [1])
        self.assertEqual(len(api.issues), 1)
        self.assertEqual(api.comments, {})
        api.runs[42]['run_attempt'] = 2
        for job in api.jobs[42]:
            job['run_attempt'] = 2
        ci.report(api.event(), api)
        ci.report(api.event(), api)
        self.assertEqual(len(api.comments[1]), 1)
        self.assertIn('ci-full-run:42:2', api.comments[1][0]['body'])

    def test_each_reduced_full_counterpart_reports_its_owning_area(self):
        for run_id, name, area in [(42, 'h264-full', 'H.264 decoding'),
                                    (42, 'transcode-compat-shard (3)', 'Transcode compatibility'),
                                    (43, 'corpus (7)', 'VP9 decoding'),
                                    (44, 'interpolation-fuzz (macos-15)', 'VP9 decoding'),
                                    (45, 'hevc-vectors', 'HEVC decoding')] + [
                (45, f'profile-census ({index})', 'HEVC decoding') for index in range(4)]:
            with self.subTest(job=name):
                api = FakeApi()
                api.fail(name, run_id)
                ci.report(api.event(run_id), api)
                self.assertEqual(len(api.issues), 1)
                self.assertIn(area, api.issues[0]['body'])

    def test_ineligible_events_never_read_or_write_issues(self):
        for event, branch, repository in [('pull_request', 'feature', ci.REPOSITORY),
                                          ('merge_group', 'main', ci.REPOSITORY),
                                          ('workflow_dispatch', 'main', ci.REPOSITORY),
                                          ('push', 'feature', ci.REPOSITORY), ('push', 'main', 'fork/coreav')]:
            api = FakeApi()
            payload = api.event()
            payload['workflow_run'].update(event=event, head_branch=branch, head_repository={'full_name': repository})
            self.assertEqual(ci.report(payload, api), [])
            self.assertEqual(api.calls, [])

    def test_schedule_and_cancelled_or_missing_full_work_are_reportable(self):
        for result in ('failure', 'cancelled', 'timed_out', 'skipped'):
            api = FakeApi()
            api.fail('h264-full')
            api.runs[42]['event'] = 'schedule'
            api.jobs[42][1]['conclusion'] = result
            self.assertEqual(ci.report(api.event(), api), [1])
        for run_id, missing in ((42, 'transcode-compat-shard (2)'), (45, 'profile-census (0)')):
            api = FakeApi()
            api.jobs[run_id] = [job for job in api.jobs[run_id] if job['name'] != missing]
            ci.report(api.event(run_id), api)
            self.assertIn('CI infrastructure', api.issues[0]['body'])

    def test_failed_preparation_does_not_blame_skipped_decoder_or_transcode_work(self):
        api = FakeApi()
        api.fail('full-fixtures')
        for job in api.jobs[42][1:]:
            job['conclusion'] = 'skipped'
        ci.report(api.event(), api)
        self.assertEqual(len(api.issues), 1)
        self.assertIn('CI infrastructure', api.issues[0]['body'])
        self.assertNotIn('area:h264', api.issues[0]['labels'])

    def test_skipped_proof_cannot_hide_behind_a_successful_job(self):
        api = FakeApi()
        api.jobs[42][1]['steps'][0]['conclusion'] = 'skipped'
        ci.report(api.event(), api)
        self.assertIn('H.264 decoding', api.issues[0]['body'])

    def test_ambiguous_creation_is_read_back_without_reposting(self):
        api = FakeApi()
        api.fail('h264-full')
        api.ambiguous_create = True
        self.assertEqual(ci.report(api.event(), api), [1])
        self.assertEqual(len([call for call in api.calls if call[0] == 'POST' and call[1].endswith('/issues')]), 1)

    def test_competing_creations_preserve_both_runs_and_close_only_the_duplicate(self):
        api = FakeApi()
        api.fail('h264-full')
        api.race_create = True
        ci.report(api.event(), api)
        self.assertEqual([item['number'] for item in api.issues if item['state'] == 'open'], [1])
        retained = api.issues[0]['body'] + ''.join(comment['body'] for comment in api.comments[1])
        self.assertIn('ci-full-run:42:1', retained)
        self.assertIn('ci-full-run:777:1', retained)

    def test_lookup_failure_cannot_create_an_issue_or_become_green(self):
        api = FakeApi()
        api.fail('h264-full')
        api.fail_lookup = True
        with self.assertRaisesRegex(ci.Failure, 'api_http_403'):
            ci.report(api.event(), api)
        self.assertEqual(api.issues, [])

    def test_foreign_run_or_job_metadata_is_rejected_before_writes(self):
        for field, value in [('head_sha', 'b' * 40), ('run_attempt', 2), ('run_id', 9)]:
            api = FakeApi()
            api.jobs[42][0][field] = value
            with self.assertRaisesRegex(ci.Failure, 'wrong_job_attempt'):
                ci.report(api.event(), api)
            self.assertFalse(api.issues)
        api = FakeApi()
        payload = api.event()
        api.runs[42]['path'] = '.github/workflows/unknown.yml'
        with self.assertRaisesRegex(ci.Failure, 'wrong_workflow'):
            ci.report(payload, api)

    def test_untrusted_names_and_exception_arguments_never_enter_public_issue_text(self):
        api = FakeApi()
        api.runs[42]['conclusion'] = 'failure'
        api.jobs[42].append({'name': 'CREDENTIAL_MARKER_untrusted', 'status': 'completed', 'conclusion': 'failure',
                            'run_id': 42, 'run_attempt': 1, 'head_sha': HEAD})
        ci.report(api.event(), api)
        self.assertNotIn('CREDENTIAL_MARKER', json.dumps(api.issues))
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / 'event.json'
            event.write_text(json.dumps(api.event()))
            with mock.patch.object(sys, 'argv', ['ci_full.py', 'report', '--event', str(event)]), \
                 mock.patch.dict(ci.os.environ, {'GH_TOKEN': 'example-not-a-key'}), \
                 mock.patch.object(ci, 'Api', side_effect=RuntimeError('CREDENTIAL_MARKER_untrusted')), \
                 mock.patch('sys.stderr', new_callable=io.StringIO) as stderr:
                self.assertEqual(ci.main(), 1)
                self.assertEqual(stderr.getvalue(), 'ci-full: invalid_evidence\n')


class FinalHead(unittest.TestCase):
    def test_verifier_requires_every_unique_shard_and_complete_scalar_proof(self):
        for index in range(4):
            for mutation in ('missing', 'duplicate', 'skipped_proof'):
                with self.subTest(shard=index, mutation=mutation):
                    api = FakeApi()
                    api.manual()
                    name = f'profile-census ({index})'
                    job = next(job for job in api.jobs[45] if job['name'] == name)
                    if mutation == 'missing':
                        api.jobs[45].remove(job)
                    elif mutation == 'duplicate':
                        api.jobs[45].append(copy.deepcopy(job))
                    else:
                        job['steps'][0]['conclusion'] = 'skipped'
                    with self.assertRaisesRegex(ci.Failure, 'final_head_dispatch_not_green'):
                        ci.verify(26, api, ['hevc-conformance'])
        api = FakeApi()
        api.manual()
        job = next(job for job in api.jobs[45] if job['name'] == 'hevc-vectors')
        job['steps'] = [step for step in job['steps']
                        if step['name'] != 'Check complete scalar conformance output']
        with self.assertRaisesRegex(ci.Failure, 'final_head_dispatch_not_green'):
            ci.verify(26, api, ['hevc-conformance'])

    def test_cli_accepts_repeatable_explicit_suite_selection(self):
        with mock.patch.object(sys, 'argv', ['ci_full.py', 'verify', '--pr', '26',
                                             '--suite', 'hevc-conformance', '--suite', 'hevc-conformance']), \
             mock.patch.dict(ci.os.environ, {'GH_TOKEN': 'example-not-a-key'}), \
             mock.patch.object(ci, 'Api') as api, \
             mock.patch.object(ci, 'verify', return_value=(HEAD, [])) as verify, \
             mock.patch('sys.stdout', new_callable=io.StringIO):
            self.assertEqual(ci.main(), 0)
        verify.assert_called_once_with(26, api.return_value, ['hevc-conformance', 'hevc-conformance'])

    def test_suite_selection_is_explicit_repeatable_and_closed(self):
        for selection in ([], ['unknown'], ['hevc-conformance', 'unknown']):
            api = FakeApi()
            with self.assertRaisesRegex(ci.Failure, 'invalid_suite_selection'):
                ci.verify(26, api, selection)
            self.assertEqual(api.calls, [])
        self.assertEqual(ci.selected_suites(['hevc-conformance', 'hevc-conformance']),
                         ['hevc-conformance'])
        self.assertEqual(ci.selected_suites(), ['ci-full', 'vp9-corpus', 'vp9-conformance'])
        with mock.patch.dict(ci.FULL_WORKFLOWS['hevc-conformance'], final_head='always'):
            self.assertEqual(ci.selected_suites(), list(ci.FULL_WORKFLOWS))

    def test_verifier_requires_every_full_workflow_at_the_exact_unchanged_head(self):
        api = FakeApi()
        api.manual()
        head, evidence = ci.verify(448, api)
        self.assertEqual(head, HEAD)
        self.assertEqual({row[0] for row in evidence}, set(ci.selected_suites()))
        self.assertTrue(all(call[0] in ('GET', 'PAGES') for call in api.calls))

    def test_newest_failure_or_pending_dispatch_blocks_an_older_success(self):
        for status, conclusion in [('completed', 'failure'), ('in_progress', None)]:
            api = FakeApi()
            api.manual()
            newer = copy.deepcopy(api.runs[42])
            newer.update(id=99, created_at='2026-11-06T02:00:00Z', status=status, conclusion=conclusion)
            api.runs[99] = newer
            api.jobs[99] = [dict(job, run_id=99) for job in api.jobs[42]]
            api.dispatches['ci-full.yml'].append(newer)
            with self.assertRaisesRegex(ci.Failure, 'final_head_dispatch_not_green'):
                ci.verify(448, api)

    def test_wrong_head_branch_fork_or_attempt_cannot_supply_dispatch_evidence(self):
        for field, value in [('head_sha', 'b' * 40), ('head_branch', 'other'),
                             ('head_repository', {'full_name': 'fork/coreav'}), ('run_attempt', 2)]:
            api = FakeApi()
            api.manual()
            api.runs[42][field] = value
            with self.assertRaises(ci.Failure):
                ci.verify(448, api)

    def test_missing_matrix_leg_skipped_proof_or_duplicate_job_is_not_a_pass(self):
        for mutation in ('missing', 'skipped', 'duplicate'):
            api = FakeApi()
            api.manual()
            if mutation == 'missing':
                api.jobs[43].pop()
            elif mutation == 'skipped':
                api.jobs[43][1]['steps'][0]['conclusion'] = 'skipped'
            else:
                api.jobs[43].append(copy.deepcopy(api.jobs[43][1]))
            with self.assertRaisesRegex(ci.Failure, 'final_head_dispatch_not_green'):
                ci.verify(448, api)

    def test_missing_dispatch_invalid_timestamp_and_head_change_fail_closed(self):
        for mutation in ('missing', 'timestamp', 'head-change'):
            api = FakeApi()
            api.manual()
            if mutation == 'missing':
                api.dispatches['ci-full.yml'] = []
            elif mutation == 'timestamp':
                api.dispatches['ci-full.yml'][0]['created_at'] = 'not-a-time'
            else:
                api.changed_head = True
            with self.assertRaises(ci.Failure):
                ci.verify(448, api)

    def test_dispatch_or_rerun_started_during_verification_invalidates_old_evidence(self):
        for mutation in ('rerun', 'new-dispatch'):
            api = FakeApi()
            api.manual()
            pages = api.pages
            def racing_pages(path, key=None):
                result = pages(path, key)
                if '/runs/42/attempts/1/jobs' in path:
                    if mutation == 'rerun':
                        api.dispatches['ci-full.yml'][0].update(run_attempt=2, status='in_progress', conclusion=None)
                    else:
                        newer = dict(api.runs[42], id=99, created_at='2026-11-06T02:00:00Z',
                                     status='in_progress', conclusion=None)
                        api.dispatches['ci-full.yml'].append(newer)
                return result
            with mock.patch.object(api, 'pages', side_effect=racing_pages):
                with self.assertRaises(ci.Failure):
                    ci.verify(448, api)


class Pagination(unittest.TestCase):
    def test_hevc_registry_matches_existing_full_proof_steps_and_trusted_wiring(self):
        self.assertEqual(set(ci.AREAS), {'ci', 'h264', 'transcode', 'vp9', 'hevc'})
        self.assertEqual(set(ci.FULL_WORKFLOWS), {'ci-full', 'vp9-corpus', 'vp9-conformance', 'hevc-conformance'})
        self.assertEqual(ci.FULL_WORKFLOWS['hevc-conformance']['final_head'], 'area')
        self.assertEqual(ci.FULL_WORKFLOWS['hevc-conformance']['areas']['hevc'],
                         ('hevc-vectors', 'profile-census (0)', 'profile-census (1)',
                          'profile-census (2)', 'profile-census (3)'))
        self.assertIn('Check complete scalar conformance output',
                      ci.PROOF_STEPS['hevc-conformance', 'hevc-vectors'])
        source = (ROOT / '.github/workflows/hevc-conformance.yml').read_text()
        for (workflow, job), names in ci.PROOF_STEPS.items():
            if workflow != 'hevc-conformance':
                continue
            body = re.split(r'\n  \S', source.split('  ' + job + ':\n', 1)[1], maxsplit=1)[0]
            for name in names:
                self.assertIn('- name: ' + name + '\n', body)
        reporter = (ROOT / '.github/workflows/ci-full-failure.yml').read_text()
        self.assertIn('workflows: [' + ', '.join(ci.FULL_WORKFLOWS) + ']', reporter)
        self.assertIn('ref: ${{ github.workflow_sha }}', reporter)
        self.assertIn('persist-credentials: false', reporter)
        for permission in ('contents: read', 'actions: read', 'issues: write'):
            self.assertIn(permission, reporter)
        for forbidden in ('download-artifact', 'pull_request_target', 'head_sha }}', 'logs'):
            self.assertNotIn(forbidden, reporter)

    def test_job_pagination_is_complete_and_a_truncated_page_fails_closed(self):
        api = ci.Api('example-not-a-key')
        first = {'jobs': [{} for _ in range(100)], 'total_count': 101}
        last = {'jobs': [{}], 'total_count': 101}
        with mock.patch.object(api, 'request', side_effect=[first, last]) as request:
            self.assertEqual(len(api.pages('/repos/' + ci.REPOSITORY + '/jobs', 'jobs')), 101)
            self.assertEqual(request.call_count, 2)
        with mock.patch.object(api, 'request', return_value={'jobs': [], 'total_count': 1}):
            with self.assertRaisesRegex(ci.Failure, 'incomplete_api_pages'):
                api.pages('/repos/' + ci.REPOSITORY + '/jobs', 'jobs')

    def test_duplicate_json_fields_and_boolean_ids_are_refused(self):
        with self.assertRaisesRegex(ci.Failure, 'duplicate_api_field'):
            json.loads('{"head_sha":"a","head_sha":"b"}', object_pairs_hook=ci.object_unique)
        with self.assertRaises(ci.Failure):
            ci.attempt_evidence(FakeApi(), 'ci-full', True, 1)


if __name__ == '__main__':
    unittest.main()
