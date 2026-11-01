"""Evaluate the workflow conditions for queue-tested main trees and fallback events."""
import itertools
import json
import math
import pathlib
import re
import subprocess
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


def evaluate(expression, context):
    def value(match):
        item = context
        for key in match.group().split('.'):
            item = item.get(key, '') if isinstance(item, dict) else ''
        return repr(item)

    expression = expression.strip().removeprefix('${{').removesuffix('}}').strip()
    expression = re.sub(r'\b(?:github|steps|needs|strategy)\.[\w.*-]+', value, expression)
    expression = re.sub(r'!(?!=)', ' not ', expression.replace('&&', ' and ').replace('||', ' or '))
    return eval(expression, {'__builtins__': {}},
                {'always': lambda: True, 'contains': lambda values, entry: entry in values,
                 'startsWith': lambda value, prefix: value.startswith(prefix),
                 'true': True, 'false': False})


class MainPlan(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflows = {}
        for name in ('ci', 'deny', 'python', 'npm'):
            text = subprocess.check_output(['ruby', '-rjson', '-rpsych', '-e',
                                            'puts JSON.generate(Psych.load_file(ARGV[0]))',
                                            str(ROOT / '.github/workflows' / (name + '.yml'))], text=True)
            cls.workflows[name] = json.loads(text)

    def concurrency(self, setting, workflow, run_id, event='push', ref='refs/heads/main', matrix_index=0):
        context = {'github': {'event_name': event, 'ref': ref, 'run_id': run_id, 'workflow': workflow},
                   'strategy': {'job-index': matrix_index}}
        pieces = re.split(r'(\$\{\{.*?\}\})', setting['group'])
        key = ''.join(str(evaluate(piece, context)) if piece.startswith('${{') else piece for piece in pieces)
        return key, bool(evaluate(setting['cancel-in-progress'], context))

    def test_docs_push_never_drops_an_earlier_source_pushs_main_obligations(self):
        for old_reused, new_reused in ((True, True), (False, False), (True, False), (False, True)):
            old = self.plan('push', old_reused)
            new = self.plan('push', new_reused, code=False, h264=False)
            for name, workflow in self.workflows.items():
                with self.subTest(old_reused=old_reused, new_reused=new_reused, workflow=name):
                    retained = set(old[name])
                    if 'concurrency' in workflow:
                        old_key, _ = self.concurrency(workflow['concurrency'], name, '1')
                        new_key, cancel = self.concurrency(workflow['concurrency'], name, '2')
                        if cancel and old_key == new_key:
                            retained.clear()
                    for job in new[name]:
                        setting = workflow['jobs'][job].get('concurrency')
                        if setting:
                            old_key, _ = self.concurrency(setting, name, '1')
                            new_key, cancel = self.concurrency(setting, name, '2')
                            if cancel and old_key == new_key:
                                retained.discard(job)
                    covered = retained | set(new[name])
                    self.assertTrue(set(old[name]) <= covered,
                                    f'needed work dropped: {set(old[name]) - covered}')

    def test_main_work_survives_every_three_push_gate_ordering(self):
        """Reserve one of four runner slots for gates; children run or all wait."""
        events = [(kind, run) for run in range(3) for kind in ('push', 'gate')]
        orders = [order for order in itertools.permutations(events)
                  if all(order.index(('push', run)) < order.index(('gate', run)) for run in range(3))]
        self.assertEqual(len(orders), 90)
        failures = []
        checked = 0
        for evidence in itertools.product((False, True), repeat=3):
            plans = [self.plan('push', evidence[run], code=run != 1, h264=run != 1)
                     for run in range(3)]
            for name in ('ci', 'python', 'npm'):
                workflow = self.workflows[name]
                self.assertNotIn('concurrency', workflow)
                gate = name + '-changes'
                selected = [[(job, index) for job, count in plan[name].items()
                             if not job.endswith('-report') for index in range(count)] for plan in plans]
                expected = {(run, job, index) for run in range(3) for job, index in selected[run]}
                for order in orders:
                    for blocked in (False, True):
                        checked += 1
                        running, pending, completed = {}, {}, set()
                        for kind, run in order:
                            if kind == 'push':
                                arriving = [(gate, 0)]
                            else:
                                key, _ = self.concurrency(workflow['jobs'][gate]['concurrency'], name, str(run))
                                token = pending.pop(key, None)
                                if token != (run, gate, 0):
                                    continue
                                completed.add(token)
                                arriving = [(job, index) for job, index in selected[run] if job != gate]
                            for job, index in arriving:
                                setting = workflow['jobs'][job]['concurrency']
                                key, cancel = self.concurrency(setting, name, str(run), matrix_index=index)
                                pending.pop(key, None)
                                if cancel:
                                    running.pop(key, None)
                                pending[key] = (run, job, index)
                                if job != gate and not blocked and key not in running and len(running) < 3:
                                    running[key] = pending.pop(key)
                        retained = completed | set(running.values()) | set(pending.values())
                        if not expected <= retained:
                            failures.append((name, evidence, order, blocked, expected - retained))
        self.assertEqual(checked, 4320)
        self.assertEqual(len(failures), 0, f'{len(failures)} lost-work orderings; first: {failures[:1]}')

    def plan(self, event, reused, code=True, h264=True, oracle_installer=False):
        plans = {}
        for name, workflow in self.workflows.items():
            github = {'event_name': event, 'ref': 'refs/heads/main', 'run_id': '17', 'workflow': name,
                      'repository': 'owner/repo', 'event': {'pull_request': {'draft': False, 'labels': [],
                      'head': {'repo': {'full_name': 'owner/repo'}}}}}
            steps = {key: {'outputs': {'run': 'true' if selected else 'false'}}
                     for key, selected in (('filter', code), ('platforms', code),
                                            ('h264-conformance', h264), ('bench-guard', code),
                                            ('oracle-installer', oracle_installer))}
            steps['result'] = {'outputs': {'reuse': 'true' if reused else 'false'}}
            steps['fuzz-plan'] = {'outputs': {'plan': ''}}
            context = {'github': github, 'steps': steps, 'needs': {}}
            jobs = workflow['jobs']
            selected = {}
            for job, fields in jobs.items():
                needs = fields.get('needs', [])
                needs = [needs] if isinstance(needs, str) else needs
                condition = fields.get('if', 'true')
                dependencies = all(context['needs'][dependency]['result'] == 'success' for dependency in needs)
                run = (dependencies or 'always()' in condition) and evaluate(condition, context)
                result = {'result': 'success' if run else 'skipped', 'outputs': {}}
                if run:
                    for key, expression in fields.get('outputs', {}).items():
                        value = evaluate(expression, context)
                        result['outputs'][key] = str(value).lower() if isinstance(value, bool) else value
                    matrix = fields.get('strategy', {}).get('matrix', {})
                    count = math.prod(len(values) for key, values in matrix.items() if key not in ('include', 'exclude'))
                    selected[job] = count
                context['needs'][job] = result
            plans[name] = selected
        return plans

    def test_reused_main_keeps_only_main_maintenance_and_required_reports(self):
        plan = self.plan('push', True)
        self.assertEqual(set(plan['ci']), {'ci-changes', 'ci-report', 'isobmff-fixture-set',
                                          'main-cache', 'conformance-arm64', 'bench-guard'})
        self.assertEqual(set(plan['python']), {'python-changes', 'python-report'})
        self.assertEqual(set(plan['npm']), {'npm-changes', 'npm-report'})
        self.assertEqual(set(plan['deny']), {'deny'})
        self.assertEqual(sum(sum(jobs.values()) for jobs in plan.values()), 11)

    def test_installer_only_proof_is_reused_without_starting_the_h264_group(self):
        for event, reused in (('pull_request', False), ('merge_group', False),
                              ('push', False), ('push', True)):
            with self.subTest(event=event, reused=reused):
                ci = self.plan(event, reused, h264=False, oracle_installer=True)['ci']
                self.assertEqual('modes-identity' in ci, not reused)
                for job in ('conformance', 'conformance-arm64', 'training-identity',
                            'kernels-arm64', 'vector-width'):
                    self.assertNotIn(job, ci)
                if reused:
                    self.assertEqual(set(ci), {'ci-changes', 'ci-report', 'bench-guard',
                                               'isobmff-fixture-set', 'main-cache'})

    def test_unproven_main_and_merge_groups_do_not_skip_the_suite(self):
        main = self.plan('push', False)
        group = self.plan('merge_group', False)
        for plan in (main, group):
            self.assertIn('check', plan['ci'])
            self.assertIn('isobmff-writers', plan['ci'])
            self.assertIn('b9-shard', plan['ci'])
            self.assertIn('linux-x86_64', plan['python'])
            self.assertIn('package', plan['npm'])
            self.assertNotIn('main-cache', plan['ci'])
        self.assertEqual(sum(sum(jobs.values()) for jobs in main.values()), 34)
        self.assertNotIn('conformance-arm64', group['ci'])

    def test_documentation_only_main_keeps_the_seven_lightweight_jobs(self):
        plan = self.plan('push', True, code=False, h264=False)
        self.assertEqual(sum(sum(jobs.values()) for jobs in plan.values()), 7)

    def test_representative_non_h264_merge_drops_from_28_to_10_jobs(self):
        counts = [sum(sum(jobs.values()) for jobs in self.plan('push', reused, h264=False).values())
                  for reused in (False, True)]
        self.assertEqual(counts, [28, 10])

    def test_concurrency_never_cancels_or_joins_merge_groups(self):
        for name, workflow in self.workflows.items():
            settings = [('workflow', workflow['concurrency'])] if 'concurrency' in workflow else [
                (job, fields['concurrency']) for job, fields in workflow['jobs'].items()
                if 'concurrency' in fields]
            for job, setting in settings:
                with self.subTest(workflow=name, job=job):
                    unfiltered = name == 'deny'
                    keys = []
                    for event, ref, run_id in (('merge_group', 'refs/heads/gh-readonly-queue/main/group', '1'),
                                               ('merge_group', 'refs/heads/gh-readonly-queue/main/group', '2'),
                                               ('pull_request', 'refs/pull/1/merge', '3'),
                                               ('push', 'refs/heads/main', '4')):
                        key, cancel = self.concurrency(setting, name, run_id, event, ref)
                        keys.append(key)
                        self.assertEqual(cancel, event == 'pull_request' or (event == 'push' and unfiltered))
                    self.assertEqual(len(set(keys)), len(keys))
                    for event in ('push', 'pull_request'):
                        pair = [self.concurrency(setting, name, run_id, event)[0] for run_id in ('5', '6')]
                        self.assertEqual(pair[0] == pair[1], event == 'pull_request' or unfiltered)
                    if 'strategy' in workflow['jobs'].get(job, {}):
                        pair = [self.concurrency(setting, name, '5', matrix_index=index)[0] for index in (0, 1)]
                        self.assertNotEqual(*pair)
