"""Exercise the nightly workflow's seconds validation without fuzzing."""
import json
import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


class FuzzNightly(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = json.loads(subprocess.check_output([
            'ruby', '-ryaml', '-rjson', '-e',
            'puts JSON.generate(YAML.load_file(ARGV[0]))',
            str(ROOT / '.github/workflows/fuzz-nightly.yml'),
        ], text=True))
        cls.build = cls.workflow['jobs']['fuzz-build']
        cls.validation = next(step for step in cls.build['steps'] if step.get('id') == 'seconds')

    def validate(self, seconds, event='workflow_dispatch'):
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / 'output'
            output.touch()
            done = subprocess.run(
                ['bash', '-c', self.validation['run']],
                env=dict(os.environ, INPUT_SECONDS=seconds, EVENT_NAME=event, GITHUB_OUTPUT=str(output)),
                capture_output=True, text=True, timeout=10,
            )
            return done, output.read_text()

    def test_valid_manual_budgets_are_forwarded(self):
        for seconds in ('1', '899', '900'):
            with self.subTest(seconds=seconds):
                done, output = self.validate(seconds)
                self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
                self.assertEqual(output, f'seconds={seconds}\n')

    def test_schedule_keeps_the_default_budget(self):
        done, output = self.validate('', event='schedule')
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(output, 'seconds=900\n')
        maximum = int(output.split('=')[1])
        self.assertLessEqual(
            int(self.workflow['env']['SHARD_SIZE']) * maximum + 60 * 60,
            self.workflow['jobs']['fuzz-shard']['timeout-minutes'] * 60,
        )

    def test_invalid_budgets_fail_with_the_limit_and_no_output(self):
        for seconds in ('901', '1500', '99999999999999999999', '0', '-1', '1.5', '', 'invalid'):
            with self.subTest(seconds=seconds):
                done, output = self.validate(seconds)
                self.assertNotEqual(done.returncode, 0)
                self.assertIn('::error::', done.stdout + done.stderr)
                self.assertIn('900', done.stdout + done.stderr)
                self.assertEqual(output, '')

    def test_shards_only_receive_validated_seconds(self):
        self.assertIs(self.build['steps'][0], self.validation)
        self.assertEqual(self.validation['env']['INPUT_SECONDS'], '${{ inputs.seconds }}')
        self.assertEqual(self.validation['env']['EVENT_NAME'], '${{ github.event_name }}')
        self.assertEqual(self.build['outputs']['seconds'], '${{ steps.seconds.outputs.seconds }}')
        shard = self.workflow['jobs']['fuzz-shard']
        self.assertEqual(shard['needs'], 'fuzz-build')
        fuzz = next(step for step in shard['steps'] if step.get('id') == 'fuzz')
        self.assertEqual(fuzz['env']['FUZZ_SECONDS'], '${{ needs.fuzz-build.outputs.seconds }}')


if __name__ == '__main__':
    unittest.main()
