import itertools
import json
import os
import pathlib
import subprocess
import tempfile
import unittest

from test_main_ci_plan import evaluate


ROOT = pathlib.Path(__file__).resolve().parents[2]
ACTION = './.github/actions/ci-fixtures'


def read_yaml(path):
    return json.loads(subprocess.check_output(
        ['ruby', '-rjson', '-rpsych', '-e', 'puts JSON.generate(Psych.load_file(ARGV[0]))', str(path)],
        text=True))


class SharedFixtures(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = read_yaml(ROOT / '.github/workflows/ci.yml')
        cls.action = read_yaml(ROOT / '.github/actions/ci-fixtures/action.yml')

    def test_consumers_start_beside_the_other_checks(self):
        jobs = self.workflow['jobs']
        self.assertNotIn('isobmff-fixture-set', jobs)
        for name in ('isobmff-writers', 'isobmff-decode', 'b9-shard'):
            with self.subTest(job=name):
                self.assertEqual(jobs[name]['needs'], 'ci-changes')
                downloads = [s for s in jobs[name]['steps']
                             if s.get('uses', '').startswith('actions/download-artifact@')]
                fixture, = [s for s in downloads if s.get('with', {}).get('name') == 'isobmff-fixture-set']
                self.assertEqual(fixture['with']['path'], '${{ runner.temp }}/coreav-fixtures')
                self.assertFalse(any('gen-fixtures.sh' in s.get('run', '') for s in jobs[name]['steps']))
        report = jobs['ci-report']
        self.assertEqual(set(report['needs']), set(jobs) - {'ci-report'})
        self.assertEqual(report['if'], 'always()')

    def test_prepare_runs_for_ready_fixtures_including_reused_main(self):
        gate = self.workflow['jobs']['ci-changes']
        prepare, = [s for s in gate['steps'] if s.get('uses') == ACTION]
        for event, draft, selected in itertools.product(
                ('pull_request', 'merge_group', 'push', 'workflow_dispatch'), (False, True), (False, True)):
            context = {'github': {'event_name': event, 'event': {'pull_request': {'draft': draft}}},
                       'steps': {'filter': {'outputs': {'run': str(selected).lower()}}}}
            with self.subTest(event=event, draft=draft, selected=selected):
                self.assertEqual(bool(evaluate(prepare['if'], context)),
                                 selected and (event != 'pull_request' or not draft))

    def test_cache_misses_generate_once_before_publication_and_only_main_saves(self):
        steps = self.action['runs']['steps']
        restore, = [s for s in steps if s.get('id') == 'fixtures']
        self.assertNotIn('restore-keys', restore['with'])
        generate, = [s for s in steps if s.get('name') == 'Generate all required fixtures']
        save, = [s for s in steps if s.get('name') == 'Save the fixtures']
        publish, = [s for s in steps if s.get('uses', '').startswith('actions/upload-artifact@')]
        for ref, hit in itertools.product(('refs/heads/main', 'refs/pull/1/merge',
                                           'refs/heads/gh-readonly-queue/main/group'), ('true', 'false')):
            context = {'github': {'ref': ref}, 'steps': {'fixtures': {'outputs': {'cache-hit': hit}}}}
            self.assertEqual(bool(evaluate(generate['if'], context)), hit != 'true')
            self.assertEqual(bool(evaluate(save['if'], context)), hit != 'true' and ref == 'refs/heads/main')
        self.assertNotIn('if', publish)
        self.assertEqual(publish['with']['name'], 'isobmff-fixture-set')
        self.assertEqual(publish['with']['if-no-files-found'], 'error')
        self.assertEqual(publish['with']['path'], restore['with']['path'])
        self.assertLess(steps.index(generate), steps.index(publish))
        self.assertEqual(generate['env']['COREAV_REQUIRE_ALL_FIXTURES'], '1')
        self.assertIn('oracle_pin.py --verify-bin', generate['run'])
        self.assertEqual(save['with']['key'], restore['with']['key'])
        self.assertEqual(save['with']['path'], restore['with']['path'])

    def test_key_changes_with_the_pin_generator_and_discovered_helpers(self):
        key, = [s for s in self.action['runs']['steps'] if s.get('id') == 'key']
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            scripts = root / 'scripts'
            scripts.mkdir()
            contents = {'gen-fixtures.sh': '$SCRIPTS/craft.py\n', 'craft.py': 'first\n',
                        'install-oracle-ffmpeg.sh': 'installer\n', 'oracle-ffmpeg.env': 'pin\n'}
            for name, content in contents.items():
                (scripts / name).write_text(content)

            def compute():
                output = root / 'output'
                output.write_text('')
                subprocess.run(['bash', '-e', '-c', key['run']], cwd=root, check=True,
                               env=os.environ | {'GITHUB_OUTPUT': str(output)})
                return output.read_text()

            original = compute()
            self.assertEqual(original, compute())
            for name, content in contents.items():
                (scripts / name).write_text(content + '\n')
                self.assertNotEqual(original, compute(), name)
                (scripts / name).write_text(content)
            (scripts / 'extra.py').write_text('extra\n')
            (scripts / 'gen-fixtures.sh').write_text(contents['gen-fixtures.sh'] + '$SCRIPTS/extra.py\n')
            added = compute()
            (scripts / 'extra.py').write_text('changed\n')
            self.assertNotEqual(added, compute())


if __name__ == '__main__':
    unittest.main()
