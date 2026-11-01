"""A main push can reuse only a recent successful queue run of the same workflow and tree."""
import copy
import datetime
import importlib.util
import os
import pathlib
import subprocess
import tempfile
import unittest
import urllib.error

SPEC = importlib.util.spec_from_file_location(
    'merge_group_result', pathlib.Path(__file__).resolve().parents[1] / 'merge_group_result.py')
reuse = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(reuse)

NOW = datetime.datetime(2026, 11, 1, 3, tzinfo=datetime.timezone.utc)
TREE = 'a' * 40
SHA = 'b' * 40
ENV = {'GITHUB_EVENT_NAME': 'push', 'GITHUB_REF': 'refs/heads/main', 'GITHUB_REPOSITORY': 'owner/repo'}
RUN = {'id': 42, 'event': 'merge_group', 'status': 'completed', 'conclusion': 'success',
       'head_branch': 'gh-readonly-queue/main/pr-test', 'head_sha': SHA,
       'head_repository': {'full_name': 'owner/repo'}, 'path': '.github/workflows/ci.yml',
       'head_commit': {'id': SHA, 'tree_id': TREE},
       'created_at': '2026-11-01T02:00:00Z'}


class Evidence(unittest.TestCase):
    def check(self, runs=None, tree=TREE, env=None, error=None):
        self.calls = []

        def request(path):
            self.calls.append(path)
            if error:
                raise error
            if '/actions/workflows/' in path:
                run = copy.deepcopy(RUN)
                run['head_commit']['tree_id'] = tree
                return {'workflow_runs': [run] if runs is None else runs}
            self.fail(f'unexpected API request: {path}')

        return reuse.result(ENV if env is None else env, request, TREE, NOW, 'ci.yml')

    def test_squash_commit_can_differ_but_the_tree_must_match(self):
        run, message = self.check()
        self.assertEqual(run, 42)
        self.assertIn('/actions/runs/42', message)
        self.assertIn('event=merge_group', self.calls[0])
        self.assertEqual(len(self.calls), 1)
        self.assertIsNone(self.check(tree='c' * 40)[0])

    def test_non_main_events_never_read_or_reuse_evidence(self):
        for event, ref in (('pull_request', 'refs/pull/1/merge'),
                           ('merge_group', 'refs/heads/gh-readonly-queue/main/pr-test'),
                           ('workflow_dispatch', 'refs/heads/main'), ('push', 'refs/heads/topic')):
            with self.subTest(event=event, ref=ref):
                self.assertIsNone(self.check(env=dict(ENV, GITHUB_EVENT_NAME=event, GITHUB_REF=ref))[0])
                self.assertEqual(self.calls, [])

    def test_wrong_event_workflow_repository_branch_status_or_age_cannot_pass(self):
        for field, value in (('event', 'pull_request'), ('status', 'in_progress'),
                             ('conclusion', 'failure'), ('conclusion', 'cancelled'),
                             ('path', '.github/workflows/other.yml'),
                             ('head_repository', {'full_name': 'other/repo'}),
                             ('head_branch', 'gh-readonly-queue/release/pr-test'),
                             ('head_sha', 'invalid'),
                             ('head_commit', None),
                             ('head_commit', {'id': 'c' * 40, 'tree_id': TREE}),
                             ('created_at', '2026-10-01T02:00:00Z'),
                             ('created_at', '2026-11-02T02:00:00Z')):
            with self.subTest(field=field, value=value):
                self.assertIsNone(self.check(runs=[dict(RUN, **{field: value})])[0])
                self.assertEqual(len(self.calls), 1)

    def test_missing_evidence_and_api_errors_run_full_checks(self):
        self.assertIsNone(self.check(runs=[])[0])
        for error in (urllib.error.URLError('offline'), ValueError('bad JSON'), KeyError('missing')):
            with self.subTest(error=error):
                run, message = self.check(error=error)
                self.assertIsNone(run)
                self.assertIn('run the normal checks', message)

    def test_search_is_bounded(self):
        run, _ = self.check(runs=[dict(RUN, event='pull_request')] * 20 + [RUN])
        self.assertIsNone(run)
        self.assertEqual(len(self.calls), 1)

    def test_a_newer_matching_failure_or_pending_run_blocks_older_success(self):
        for newer in (dict(RUN, conclusion='failure'), dict(RUN, status='in_progress')):
            with self.subTest(newer=newer):
                self.assertIsNone(self.check(runs=[newer, RUN])[0])
                self.assertEqual(len(self.calls), 1)

    def test_cli_reports_the_no_reuse_output_without_action_permissions(self):
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / 'output'
            summary = pathlib.Path(directory) / 'summary'
            env = dict(os.environ, GITHUB_EVENT_NAME='pull_request', GITHUB_REF='refs/pull/1/merge',
                       GITHUB_OUTPUT=str(output), GITHUB_STEP_SUMMARY=str(summary))
            env.pop('GH_TOKEN', None)
            command = subprocess.run(['python3', '-I', str(SPEC.origin), '--workflow', 'ci.yml'],
                                     env=env, capture_output=True, text=True,
                                     cwd=pathlib.Path(__file__).resolve().parents[2], check=True)
            self.assertEqual(output.read_text(), 'reuse=false\n')
            self.assertIn('Not a push to main', command.stdout)
            self.assertEqual(summary.read_text(), command.stdout)
