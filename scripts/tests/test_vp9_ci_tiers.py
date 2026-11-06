import json
import itertools
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
sys.path.insert(0, str(ROOT / 'scripts/tests'))
import ci_full
import test_main_ci_plan


def load(name):
    return json.loads(subprocess.check_output([
        'ruby', '-rjson', '-rpsych', '-e',
        'd = Psych.load_file(ARGV[0]); d["on"] ||= d.delete(true); puts JSON.generate(d)',
        str(ROOT / '.github/workflows' / name),
    ], text=True))


class Tiers(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.corpus = load('vp9-corpus.yml')
        cls.conformance = load('vp9-conformance.yml')

    def test_corpus_smoke_runs_on_every_relevant_revision_but_full_work_is_not_per_push(self):
        for event in ('push', 'pull_request'):
            paths = self.corpus['on'][event]['paths']
            self.assertIn('scripts/vp9_smoke.py', paths)
            self.assertIn('scripts/tests/test_vp9_smoke.py', paths)
        self.assertIn('schedule', self.corpus['on'])
        smoke = self.corpus['jobs']['contract']
        self.assertEqual(smoke['if'], "!cancelled() && needs.full-coverage.outputs.reuse != 'true'")
        command, = [step['run'] for step in smoke['steps'] if step.get('name') == 'Compare the routine original-file smoke subset']
        self.assertIn('scripts/vp9_smoke.py --native target/release/vp9-corpus-oracle', command)
        self.assertNotIn('vp9_corpus_smoke.py', command)
        self.assertEqual(self.corpus['jobs']['corpus']['strategy']['matrix']['shard'], list(range(8)))
        for event, branch, expected in [('pull_request', 'feature', False), ('merge_group', 'main', False),
                                        ('push', 'feature', False), ('push', 'main', True),
                                        ('schedule', 'main', True), ('workflow_dispatch', 'feature', True)]:
            context = {'github': {'repository': ci_full.REPOSITORY, 'event_name': event, 'ref': 'refs/heads/' + branch},
                       'needs': {'contract': {'result': 'success'}}}
            self.assertEqual(test_main_ci_plan.evaluate(self.corpus['jobs']['corpus']['if'], context), expected)

    def test_interpolation_smoke_retains_full_daily_main_and_manual_budgets(self):
        self.assertIn('schedule', self.conformance['on'])
        step, = [step for step in self.conformance['jobs']['interpolation-fuzz']['steps']
                 if step.get('name') == 'Compare automatic and scalar interpolation for C3']
        self.assertIn('"$FUZZ_SECONDS"', step['run'])
        for event, seconds in [('pull_request', '30'), ('push', '600'), ('schedule', '600'), ('workflow_dispatch', '600')]:
            expression = step['env']['FUZZ_SECONDS'].removeprefix('${{').removesuffix('}}').strip()
            self.assertEqual(test_main_ci_plan.evaluate(expression, {'github': {'event_name': event}}), seconds)
        vectors = self.conformance['jobs']['vectors']
        self.assertEqual(vectors['strategy']['matrix']['os'], ['ubuntu-24.04', 'macos-15'])
        listing, = [s['run'] for s in vectors['steps'] if s.get('name') == 'List the vectors and their checksums']
        self.assertIn('for v in m["vectors"]:', listing)
        self.assertIn('crates/coreav-vp9/tests/data/vectors.json', listing)
        self.assertEqual(sum(s.get('run', '').count('--include-ignored') for s in vectors['steps']), 4)

    def test_full_evidence_names_are_wired_to_real_nonempty_workflow_steps(self):
        for workflow, spec in ci_full.FULL_WORKFLOWS.items():
            document = load(spec['file'])
            for event in ('push', 'schedule', 'workflow_dispatch'):
                context = {'github': {'event_name': event, 'ref': 'refs/heads/main',
                                      'repository': ci_full.REPOSITORY},
                           'needs': {key: {'result': 'success', 'outputs': {'reuse': 'false'}}
                                     for key in document['jobs']}}
                expanded = set()
                for job_id, job in document['jobs'].items():
                    if job_id in ('ci-full-report', 'full-coverage'):
                        continue
                    if workflow == 'hevc-conformance' and job_id in ('headers', 'hevc-smoke'):
                        continue
                    condition = job.get('if', 'true').replace('inputs.mode', repr('conformance'))
                    if not test_main_ci_plan.evaluate(condition, context):
                        continue
                    matrix = job.get('strategy', {}).get('matrix')
                    if matrix:
                        expanded.update(job_id + ' (' + ', '.join(map(str, values)) + ')'
                                        for values in itertools.product(*matrix.values()))
                    else:
                        expanded.add(job_id)
                self.assertEqual(expanded, {name for group in spec['areas'].values() for name in group})
            for expected in spec['areas'].values():
                for name in expected:
                    job_id = name.split(' (', 1)[0]
                    job = document['jobs'][job_id]
                    for proof in ci_full.PROOF_STEPS[workflow, job_id]:
                        step, = [step for step in job['steps'] if step.get('name') == proof]
                        self.assertTrue(step.get('run') or step.get('uses'))
                        self.assertNotIn('continue-on-error', step)

    def test_reporter_uses_trusted_code_and_only_metadata_without_cancelling_older_reports(self):
        reporter = load('ci-full-failure.yml')
        self.assertEqual(set(reporter['on']), {'workflow_run'})
        self.assertEqual(set(reporter['on']['workflow_run']['workflows']), set(ci_full.FULL_WORKFLOWS))
        self.assertNotIn('concurrency', reporter)
        job = reporter['jobs']['full-failure-issues']
        self.assertNotIn('concurrency', job)
        self.assertEqual(job['permissions'], {'contents': 'read', 'actions': 'read', 'issues': 'write'})
        checkout, = [step for step in job['steps'] if step.get('uses', '').startswith('actions/checkout@')]
        self.assertEqual(checkout['with']['ref'], '${{ github.workflow_sha }}')
        self.assertNotIn('download-artifact', json.dumps(job))
        self.assertNotIn('workflow_run.head_sha', json.dumps(job['steps']))


if __name__ == '__main__':
    unittest.main()
