"""Exercise frozen-input failure propagation in the unfiltered deny step."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from test_registered_candidate import load_workflow

ROOT = Path(__file__).resolve().parents[2]


class FrozenWorkflow(unittest.TestCase):
    def test_frozen_check_runs_unfiltered_before_checkout_writing_tests(self):
        workflow = load_workflow('deny.yml')
        for event in ('push', 'pull_request', 'merge_group'):
            self.assertIn(event, workflow['true'])
            self.assertNotIn('paths', workflow['true'][event])
            self.assertNotIn('paths-ignore', workflow['true'][event])
        steps = workflow['jobs']['deny-checks']['steps']
        step, = [step for step in steps if step.get('name') == 'Check the workflows against the required-check list']
        self.assertEqual(step.get('if'), "steps.result.outputs.reuse != 'true'")
        self.assertNotIn('continue-on-error', step)
        self.assertIn('python3 -I scripts/compat_sources.py check', step['run'])
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            executable = root / 'python3'
            executable.write_text(f'#!{sys.executable}\n'
                                  'import json, os, sys\n'
                                  'with open(os.environ["CALLS"], "a") as log:\n'
                                  '    log.write(json.dumps(sys.argv[1:]) + "\\n")\n'
                                  'if sys.argv[2] == "scripts/compat_frozen_check.py":\n'
                                  '    sys.exit(int(os.environ["FROZEN_EXIT"]))\n')
            executable.chmod(0o700)
            for code in (0, 17):
                with self.subTest(code=code):
                    log = root / f'calls-{code}.jsonl'
                    env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ['PATH'],
                               CALLS=str(log), FROZEN_EXIT=str(code))
                    result = subprocess.run(['bash', '-c', step['run']], cwd=root, env=env,
                                            capture_output=True, text=True, timeout=30)
                    self.assertEqual(result.returncode, code, result.stderr)
                    calls = [json.loads(line) for line in log.read_text().splitlines()]
                    self.assertEqual(calls[:3], [['-I', 'scripts/workflow_check.py'],
                                                ['-I', 'scripts/compat_sources.py', 'check'],
                                                ['-I', 'scripts/compat_frozen_check.py']])
                    if code:
                        self.assertEqual(len(calls), 3)
                    else:
                        self.assertEqual(calls[3], ['-I', 'scripts/oracle_pin.py', '--check'])
