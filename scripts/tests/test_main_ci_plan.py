"""Evaluate the workflow conditions for queue-tested main trees and fallback events."""
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

    def plan(self, event, reused, code=True, h264=True):
        plans = {}
        for name, workflow in self.workflows.items():
            github = {'event_name': event, 'ref': 'refs/heads/main', 'run_id': '17', 'workflow': name,
                      'repository': 'owner/repo', 'event': {'pull_request': {'draft': False, 'labels': [],
                      'head': {'repo': {'full_name': 'owner/repo'}}}}}
            steps = {key: {'outputs': {'run': 'true' if selected else 'false'}}
                     for key, selected in (('filter', code), ('platforms', code),
                                            ('h264-conformance', h264), ('bench-guard', code))}
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
            concurrency = workflow['concurrency']
            keys = []
            for event, ref, run_id in (('merge_group', 'refs/heads/gh-readonly-queue/main/group', '1'),
                                       ('merge_group', 'refs/heads/gh-readonly-queue/main/group', '2'),
                                       ('pull_request', 'refs/pull/1/merge', '3'),
                                       ('push', 'refs/heads/main', '4')):
                context = {'github': {'event_name': event, 'ref': ref, 'run_id': run_id, 'workflow': name}}
                pieces = re.split(r'(\$\{\{.*?\}\})', concurrency['group'])
                key = ''.join(str(evaluate(piece, context)) if piece.startswith('${{') else piece for piece in pieces)
                keys.append(key)
                self.assertEqual(bool(evaluate(concurrency['cancel-in-progress'], context)),
                                 event == 'push' or (name != 'ci' and event == 'pull_request'))
            self.assertEqual(len(set(keys)), len(keys))
            for event in ('push', 'pull_request'):
                pair = []
                for run_id in ('5', '6'):
                    context = {'github': {'event_name': event, 'ref': 'refs/heads/main',
                                           'run_id': run_id, 'workflow': name}}
                    pair.append(''.join(str(evaluate(piece, context)) if piece.startswith('${{') else piece
                                        for piece in re.split(r'(\$\{\{.*?\}\})', concurrency['group'])))
                self.assertEqual(pair[0] == pair[1], event == 'push' or name != 'ci')
