"""A main push can reuse only a recent successful queue run of the same workflow and tree."""
import copy
import contextlib
import datetime
import importlib.util
import io
import json
import os
import pathlib
import subprocess
import tempfile
import unittest
from unittest import mock
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


class PullRequestEvidence(unittest.TestCase):
    def setUp(self):
        self.base = 'c' * 40
        self.merge = 'd' * 40
        self.repo = {'full_name': 'owner/repo', 'id': 7}
        self.pull = {'state': 'open', 'draft': False, 'user': {'login': 'owner'},
                     'base': {'ref': 'main', 'sha': self.base, 'repo': self.repo},
                     'head': {'ref': 'topic', 'sha': SHA, 'repo': self.repo}}
        self.event = {'number': 17, 'pull_request': self.pull,
                      'merge_group': {'base_ref': 'refs/heads/main', 'base_sha': self.base,
                                      'head_sha': self.merge}}
        self.env = dict(ENV, GITHUB_EVENT_NAME='merge_group', GITHUB_RUN_ID='100',
                        GITHUB_RUN_ATTEMPT='1', GITHUB_SHA=self.merge,
                        GITHUB_REF=f'refs/heads/gh-readonly-queue/main/pr-{self.event["number"]}-' + SHA)
        self.run = dict(RUN, event='pull_request', head_branch='topic', run_attempt=1,
                        repository=self.repo)
        self.current = dict(RUN, id=100, event='merge_group', head_sha=self.merge,
                            head_commit={'id': self.merge, 'tree_id': TREE},
                            head_branch=self.env['GITHUB_REF'].removeprefix('refs/heads/'),
                            created_at='2026-11-01T02:30:00Z')
        self.artifact = {'id': 55, 'name': reuse.receipt_name(TREE, self.base, 1), 'expired': False,
                         'workflow_run': {'id': 42, 'head_sha': SHA, 'head_branch': 'topic',
                                          'repository_id': 7, 'head_repository_id': 7}}
        self.runs = [self.run]
        self.artifacts = [self.artifact]
        self.fresh_runs = None
        self.fresh_pull = None
        self.calls = []
        self.indices = self.pulls = 0

    def request(self, path):
        self.calls.append(path)
        if path.endswith('/actions/runs/100'):
            return copy.deepcopy(self.current)
        if '/pulls/' in path:
            self.pulls += 1
            return copy.deepcopy(self.fresh_pull if self.pulls > 1 and self.fresh_pull else self.pull)
        if '/actions/workflows/' in path:
            self.indices += 1
            runs = self.fresh_runs if self.indices > 1 and self.fresh_runs is not None else self.runs
            return {'total_count': len(runs), 'workflow_runs': copy.deepcopy(runs)}
        if '/artifacts?' in path:
            return {'total_count': len(self.artifacts), 'artifacts': copy.deepcopy(self.artifacts)}
        self.fail(path)

    def lookup(self, **kwargs):
        options = dict(event=self.event, parents=[self.base])
        options.update(kwargs)
        return reuse.result(self.env, self.request, TREE, NOW, 'ci.yml', **options)

    def test_actual_tested_tree_and_base_receipt_proves_a_successful_pr_run(self):
        run, message = self.lookup()
        self.assertEqual(run, 42)
        self.assertIn('successful pull_request', message)
        self.assertIn('base ' + self.base, message)
        self.assertEqual(len(self.calls), 6)
        self.assertIn('tested-tree-v1-' + TREE + '-' + self.base + '-1', self.calls[3])
        self.assertNotEqual(self.run['head_commit']['id'], self.merge)

    def test_tip_tree_alone_is_never_sufficient(self):
        self.artifacts = []
        self.assertIsNone(self.lookup()[0])

    def test_tree_base_attempt_and_artifact_ownership_are_exact(self):
        for change in ({'name': reuse.receipt_name('f' * 40, self.base, 1)},
                       {'name': reuse.receipt_name(TREE, 'f' * 40, 1)},
                       {'name': reuse.receipt_name(TREE, self.base, 2)},
                       {'expired': True}, {'expired': None}, {'id': 0},
                       *({'workflow_run': dict(self.artifact['workflow_run'], **{key: value})}
                         for key, value in [('id', 43), ('head_sha', 'f' * 40),
                                            ('head_branch', 'other'), ('repository_id', 8),
                                            ('head_repository_id', 8)])):
            with self.subTest(change=change):
                self.artifacts = [dict(self.artifact, **change)]
                self.assertIsNone(self.lookup()[0])
        self.artifacts = [self.artifact, self.artifact]
        self.assertIsNone(self.lookup()[0])
        self.assertIsNone(self.lookup(parents=['f' * 40])[0])

    def test_ineligible_pulls_and_runs_fail_closed(self):
        original_pull = copy.deepcopy(self.pull)
        original_run = copy.deepcopy(self.run)
        for pull in (dict(original_pull, state='closed'), dict(original_pull, draft=True),
                     dict(original_pull, user={'login': 'dependabot[bot]'}),
                     dict(original_pull, base=dict(original_pull['base'], ref='release')),
                     dict(original_pull, head=dict(original_pull['head'], repo={'full_name': 'fork/repo'}))):
            with self.subTest(pull=pull):
                self.pull = pull
                self.assertIsNone(self.lookup()[0])
        self.pull = original_pull
        for key, value in [('event', 'push'), ('head_sha', 'f' * 40), ('head_branch', 'other'),
                           ('path', '.github/workflows/other.yml'), ('repository', {'full_name': 'fork/repo'}),
                           ('head_repository', {'full_name': 'fork/repo'}), ('status', 'in_progress'),
                           ('conclusion', 'failure'), ('conclusion', 'cancelled'),
                           ('created_at', '2026-10-30T02:00:00Z'),
                           ('created_at', '2026-11-01T02:45:00Z')]:
            with self.subTest(key=key, value=value):
                self.runs = [dict(original_run, **{key: value})]
                self.assertIsNone(self.lookup()[0])

    def test_newer_pending_failure_and_rerun_block_old_success(self):
        for status, conclusion in [('in_progress', None), ('completed', 'failure')]:
            self.runs = [self.run, dict(self.run, id=43, status=status, conclusion=conclusion,
                                       created_at='2026-11-01T02:01:00Z')]
            self.assertIsNone(self.lookup()[0])
        self.runs = [dict(self.run, run_attempt=2)]
        self.assertIsNone(self.lookup()[0])

    def test_revalidation_rejects_new_dispatches_reruns_and_head_pushes(self):
        for change in ({'id': 43, 'created_at': '2026-11-01T02:01:00Z'},
                       {'run_attempt': 2}, {'status': 'in_progress', 'conclusion': None}):
            with self.subTest(change=change):
                self.setUp()
                self.fresh_runs = [dict(self.run, **change)]
                self.assertIsNone(self.lookup()[0])
        self.setUp()
        self.fresh_pull = dict(self.pull, head=dict(self.pull['head'], sha='f' * 40))
        self.assertIsNone(self.lookup()[0])

    def test_missing_malformed_and_incomplete_metadata_runs_normal_checks(self):
        for value in ({}, {'workflow_runs': [], 'total_count': 1},
                      {'workflow_runs': [self.run] * 21, 'total_count': 21}):
            with self.subTest(value=value):
                with mock.patch.object(self, 'request', return_value=value):
                    self.assertIsNone(self.lookup()[0])
        with mock.patch.object(self, 'request', side_effect=ValueError('SECRET')):
            run, message = self.lookup()
            self.assertIsNone(run)
            self.assertNotIn('SECRET', message)

    def test_recording_binds_the_checkout_merge_and_base_not_the_tip(self):
        env = dict(self.env, GITHUB_EVENT_NAME='pull_request', GITHUB_REF='refs/pull/17/merge')
        parents = [self.base, SHA]
        name = reuse.pr_receipt(env, self.event, TREE, self.merge, parents)
        self.assertEqual(name, reuse.receipt_name(TREE, self.base, 1))
        for change in ({'GITHUB_SHA': SHA}, {'GITHUB_REF': 'refs/heads/topic'},
                       {'GITHUB_EVENT_NAME': 'push'}, {'GITHUB_ACTOR': 'dependabot[bot]'},
                       {'GITHUB_TRIGGERING_ACTOR': 'dependabot[bot]'}, {'GITHUB_RUN_ATTEMPT': '0'}):
            self.assertIsNone(reuse.pr_receipt(dict(env, **change), self.event, TREE, self.merge, parents))
        self.assertIsNone(reuse.pr_receipt(env, self.event, TREE, self.merge, [self.base]))
        self.assertIsNone(reuse.pr_receipt(env, self.event, TREE, self.merge, []))
        for key, value in [('draft', True), ('state', 'closed'), ('user', {'login': 'dependabot[bot]'})]:
            event = dict(self.event, pull_request=dict(self.pull, **{key: value}))
            self.assertIsNone(reuse.pr_receipt(env, event, TREE, self.merge, parents))

    def test_cli_records_the_actual_checkout_receipt_without_api_requests(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            event = root / 'event.json'
            event.write_text(json.dumps(self.event))
            env = dict(self.env, GITHUB_EVENT_NAME='pull_request', GITHUB_REF='refs/pull/17/merge',
                       GITHUB_EVENT_PATH=str(event), GITHUB_OUTPUT=str(root / 'output'), RUNNER_TEMP=directory)
            git = [TREE + '\n', self.merge + '\n', self.merge + ' ' + self.base + ' ' + SHA + '\n']
            with mock.patch.dict(os.environ, env, clear=True), \
                    mock.patch.object(reuse.subprocess, 'check_output', side_effect=git), \
                    mock.patch('sys.argv', ['merge_group_result.py', '--workflow', 'ci.yml']), \
                    mock.patch.object(reuse.urllib.request, 'urlopen') as network, \
                    contextlib.redirect_stdout(io.StringIO()):
                reuse.main()
                network.assert_not_called()
            self.assertEqual((root / 'output').read_text(),
                             'reuse=false\nrecord_name=' + reuse.receipt_name(TREE, self.base, 1)
                             + '\nrecord_path=' + str(root / 'tested-tree.txt') + '\n')
            self.assertEqual((root / 'tested-tree.txt').read_text(),
                             f'tree={TREE}\nbase={self.base}\ncommit={self.merge}\n')

    def test_group_metadata_must_bind_the_checked_out_tree(self):
        for key, value in [('head_sha', SHA), ('event', 'push'),
                           ('head_branch', f'gh-readonly-queue/release/pr-{self.event["number"]}-' + SHA),
                           ('head_commit', {'id': self.merge, 'tree_id': 'f' * 40})]:
            with self.subTest(key=key):
                self.setUp()
                self.current[key] = value
                self.assertIsNone(self.lookup()[0])
