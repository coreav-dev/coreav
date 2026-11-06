"""Small contracts for the core smoke and independent full tier."""
import copy
import json
import os
import pathlib
import shlex
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import h264_ci
import test_main_ci_plan


ROOT = pathlib.Path(__file__).resolve().parents[2]


def load(path):
    return json.loads(subprocess.check_output([
        'ruby', '-rjson', '-rpsych', '-e',
        'd = Psych.load_file(ARGV[0]); d["on"] ||= d.delete(true); puts JSON.generate(d)',
        str(ROOT / path),
    ], text=True))


class Smoke(unittest.TestCase):
    def report(self, mode='fast'):
        return {'streams': 8, 'passed': 8, 'failed': 0, 'threads': '1,2,8', 'mode': mode,
                'results': [{'name': name, 'passed': True,
                             'bars': {'C1': 'pass exact pixels', 'C5': 'pass keyframes'}}
                            for name in h264_ci.SMOKE]}

    def test_registered_selection_and_all_99_remain_admitted(self):
        manifest = json.loads((ROOT / h264_ci.h264_conformance.MANIFEST).read_text())
        self.assertEqual(sum(row['in_scope'] for row in manifest['streams']), 99)
        h264_ci.validate_selection(h264_ci.SMOKE, manifest)
        for names in (h264_ci.SMOKE[:-1], (*h264_ci.SMOKE, 'unknown'),
                      (*h264_ci.SMOKE[:-1], 'unknown'), (*h264_ci.SMOKE[:-1], h264_ci.SMOKE[0])):
            with self.assertRaises(ValueError):
                h264_ci.validate_selection(names, manifest)
        manifest['streams'] = [row for row in manifest['streams'] if row['name'] != h264_ci.SMOKE[0]]
        with self.assertRaises(ValueError):
            h264_ci.validate_selection(h264_ci.SMOKE, manifest)

    def test_missing_extra_unknown_duplicate_failed_or_wrong_mode_results_fail(self):
        h264_ci.validate_report(self.report(), 'fast')
        mutations = [
            lambda r: r['results'].pop(),
            lambda r: r['results'].append(copy.deepcopy(r['results'][0])),
            lambda r: r['results'][0].update(name='unknown'),
            lambda r: r['results'][0].update(name=h264_ci.SMOKE[1]),
            lambda r: r.update(streams=7), lambda r: r.update(passed=7),
            lambda r: r.update(failed=1), lambda r: r.update(threads='1'),
            lambda r: r.update(mode='safe'),
            lambda r: r['results'][0].update(passed=False),
            lambda r: r['results'][0]['bars'].pop('C5'),
            lambda r: r['results'][0]['bars'].update(C1='FAIL pixels'),
        ]
        for mutate in mutations:
            report = self.report()
            mutate(report)
            with self.assertRaises(ValueError):
                h264_ci.validate_report(report, 'fast')

    def test_entrypoint_preserves_arguments_and_requires_new_exact_report(self):
        with tempfile.TemporaryDirectory() as directory:
            report = pathlib.Path(directory) / 'report.json'
            args = ['h264_ci.py', '--only', *h264_ci.SMOKE, '--zips', directory,
                    '--work', directory, '--threads', '1,2,8', '--report', str(report)]
            for mode in ('fast', 'safe'):
                argv = args + (['--mode', 'safe'] if mode == 'safe' else [])
                def run():
                    self.assertEqual(sys.argv, argv)
                    self.assertFalse(report.exists())
                    report.write_text(json.dumps(self.report(mode)))
                    raise SystemExit(0)
                with mock.patch.object(sys, 'argv', argv), mock.patch.object(
                        h264_ci.h264_conformance, 'main', side_effect=run):
                    report.write_text('stale')
                    h264_ci.main()
            with mock.patch.object(sys, 'argv', args), mock.patch.object(h264_ci.h264_conformance, 'main'):
                with self.assertRaises(FileNotFoundError):
                    h264_ci.main()

    def test_isolated_entrypoint_refuses_unknown_names_before_acquisition(self):
        done = subprocess.run([sys.executable, '-I', str(ROOT / 'scripts/h264_ci.py'),
                               '--only', 'unknown', '--download-only', '--zips', '/nonexistent'],
                              capture_output=True, text=True, cwd=ROOT)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('exactly the eight approved names', done.stderr)


class Tiers(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.ci = load('.github/workflows/ci.yml')
        cls.full = load('.github/workflows/ci-full.yml')
        cls.action = load('.github/actions/ci-fixtures/action.yml')

    def test_full_events_jobs_and_no_queue_reuse(self):
        self.assertEqual(self.full['name'], 'ci-full')
        self.assertEqual(set(self.full['on']), {'push', 'schedule', 'workflow_dispatch'})
        self.assertEqual(self.full['on']['push']['branches'], ['main'])
        self.assertEqual(set(self.full['jobs']), {'full-coverage', 'full-fixtures', 'h264-full', 'conformance-arm64',
                         'transcode-compat-shard', 'transcode-compat', 'ci-full-report'})
        self.assertNotIn('concurrency', self.full)
        self.assertIn('merge-group-result', json.dumps(self.full['jobs']['full-coverage']))
        self.assertEqual(self.full['jobs']['full-coverage']['if'],
                         "github.event_name == 'schedule' && github.event.schedule == '30 3 * * *'")
        for name in ('h264-full', 'conformance-arm64', 'transcode-compat-shard', 'transcode-compat'):
            self.assertNotIn(name, self.ci['jobs'])
        for event in ('schedule', 'workflow_dispatch', 'push'):
            plans = test_main_ci_plan.MainPlan()
            plans.workflows = {'ci-full': self.full}
            self.assertEqual(sum(plans.plan(event, True)['ci-full'].values()), 9 if event == 'schedule' else 8)
        self.assertEqual(plans.plan('schedule', True, full_covered=True)['ci-full'], {'full-coverage': 1})

    def test_full_report_rejects_every_non_success(self):
        job = self.full['jobs']['ci-full-report']
        self.assertEqual(job['if'], "always() && needs.full-coverage.outputs.reuse != 'true'")
        self.assertEqual(set(job['needs']), set(self.full['jobs']) - {'ci-full-report'})
        self.assertNotIn('concurrency', job)
        for result in ('success', 'failure', 'cancelled', 'skipped', 'unknown'):
            needs = {name: {'result': 'success'} for name in job['needs']}
            needs['h264-full']['result'] = result
            with tempfile.TemporaryDirectory() as directory:
                done = subprocess.run(['bash', '-c', job['steps'][0]['run']], capture_output=True,
                                      env={**os.environ, 'RESULTS': json.dumps(needs),
                                           'GITHUB_STEP_SUMMARY': str(pathlib.Path(directory) / 'summary')})
            self.assertEqual(done.returncode == 0, result == 'success')

    def test_smoke_selects_identical_eight_for_acquisition_fast_and_safe(self):
        steps = self.ci['jobs']['conformance']['steps']
        commands = [shlex.split(s['run'].replace('\\\n', '')) for s in steps
                    if 'scripts/h264_ci.py' in s.get('run', '')]
        self.assertEqual(len(commands), 3)
        for command in commands:
            start = command.index('--only') + 1
            end = next(i for i in range(start, len(command)) if command[i].startswith('--'))
            self.assertEqual(command[start:end], list(h264_ci.SMOKE))
        full = self.full['jobs']['h264-full']['steps']
        commands = [s['run'] for s in full if 'scripts/h264_conformance.py' in s.get('run', '')]
        self.assertEqual(len(commands), 3)
        self.assertTrue(all('--only' not in c for c in commands))
        self.assertEqual(sum('--threads 1,2,8' in c for c in commands), 2)
        self.assertIn('cargo clippy -p coreav-h264 --no-default-features --all-targets -- -D warnings', json.dumps(full))

    def test_shared_fixtures_are_produced_once_and_consumed_without_generation(self):
        for name in ('isobmff-fixtures', 'isobmff-command-compat'):
            job = self.ci['jobs'][name]
            self.assertIn(name, self.ci['jobs']['ci-report']['needs'])
            downloads = [s for s in job['steps'] if s.get('uses', '').startswith('actions/download-artifact@')]
            self.assertEqual([s['with']['name'] for s in downloads], ['isobmff-fixture-set'])
            self.assertNotIn('scripts/gen-fixtures.sh', json.dumps(job))
        steps = self.action['runs']['steps']
        generate, = [s for s in steps if s.get('name') == 'Generate all required fixtures']
        for fresh, hit, expected in (('true', 'true', True), ('false', 'true', False), ('false', 'false', True)):
            context = {'steps': {'fixtures': {'outputs': {'cache-hit': hit}}}}
            expression = generate['if'].replace('inputs.fresh', repr(fresh))
            self.assertEqual(test_main_ci_plan.evaluate(expression, context), expected)
        for step in steps:
            if 'composed' in step.get('name', ''):
                self.assertIn("inputs.composed == 'true'", step['if'])
        publisher, = [s for s in self.ci['jobs']['ci-changes']['steps'] if s.get('uses') == './.github/actions/ci-fixtures']
        self.assertEqual(publisher['with'], {'fresh': "${{ steps.result.outputs.reuse != 'true' }}"})
        publisher = self.full['jobs']['full-fixtures']['steps'][-1]
        self.assertEqual(publisher['with'], {'fresh': 'true', 'composed': 'true'})

    def test_command_branch_keeps_builds_comparators_and_informational_r1(self):
        base = self.ci['jobs']['isobmff-fixtures']['steps']
        commands = self.ci['jobs']['isobmff-command-compat']['steps']
        self.assertIn('Audit the text known divergences', [s.get('name') for s in base])
        stages = {s.get('name'): s for s in commands}
        self.assertEqual(stages['Build the coreav binary']['run'],
                         'cargo build --locked --release -p coreav-cli')
        self.assertIn('--baseline tools/compat-corpus/baseline-isobmff.json',
                      stages['Run the command-coverage set through coreav ffprobe']['run'])
        self.assertIn('--baseline tools/compat-corpus/baseline-frames.json',
                      stages['List frames through coreav ffprobe']['run'])
        self.assertIn('--features native-h264 --target-dir target/native',
                      stages['List frames through coreav ffprobe']['run'])
        self.assertIn('[ "$code" -le 2 ]', stages['R1 of the 0.1.0 round']['run'])
        self.assertIn("[ -s \"$RUNNER_TEMP/coreav-r1.json\" ]", stages['R1 of the 0.1.0 round']['run'])
        for name in ('Cache pinned black-box binaries', 'Install pinned FFmpeg, ffprobe and MediaInfo',
                     'Use pinned binaries'):
            self.assertEqual(stages[name], next(s for s in base if s.get('name') == name))
