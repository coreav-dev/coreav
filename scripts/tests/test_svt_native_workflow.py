"""Exercise native build workflow orchestration without building SVT-AV1."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / '.github/workflows/svt-av1-native.yml'


class SvtNativeWorkflow(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        decoded = subprocess.run(
            ['ruby', '-rjson', '-ryaml', '-e',
             'd = YAML.load_file(ARGV[0]); d["on"] ||= d.delete(true); puts JSON.generate(d)',
             str(WORKFLOW)], check=True, capture_output=True, text=True, timeout=10,
        )
        cls.workflow = json.loads(decoded.stdout)
        cls.job = cls.workflow['jobs']['svt-native']
        cls.step = next(s for s in cls.job['steps'] if s.get('id') == 'native-build')

    def test_all_four_targets_have_native_runners(self):
        self.assertEqual(self.job['strategy'], {
            'fail-fast': False,
            'matrix': {'include': [
                {'target': 'x86_64-unknown-linux-musl', 'os': 'ubuntu-24.04'},
                {'target': 'aarch64-unknown-linux-musl', 'os': 'ubuntu-24.04-arm'},
                {'target': 'x86_64-apple-darwin', 'os': 'macos-15-intel'},
                {'target': 'aarch64-apple-darwin', 'os': 'macos-15'},
            ]},
        })
        self.assertEqual(self.job['runs-on'], '${{ matrix.os }}')
        self.assertEqual(self.job['env']['TARGET'], '${{ matrix.target }}')

    def test_triggers_and_diagnostics_cover_the_recipe(self):
        for event in ('push', 'pull_request'):
            self.assertIn('scripts/encoders/svt-av1/**', self.workflow['on'][event]['paths'])
            self.assertIn('.github/workflows/svt-av1-native.yml', self.workflow['on'][event]['paths'])
            self.assertIn('scripts/tests/test_svt_native_workflow.py', self.workflow['on'][event]['paths'])
        self.assertIn('workflow_dispatch', self.workflow['on'])
        uploads = [s for s in self.job['steps'] if 'upload-artifact@' in s.get('uses', '')]
        self.assertEqual(len(uploads), 1)
        self.assertEqual(uploads[0]['if'], 'always()')
        self.assertEqual(uploads[0]['with']['retention-days'], 7)
        self.assertIn('${{ runner.temp }}/svt-av1/manifest.json', uploads[0]['with']['path'])
        self.assertIn('${{ runner.temp }}/svt-native-logs/*.log', uploads[0]['with']['path'])
        self.assertFalse(any('cache@' in s.get('uses', '') for s in self.job['steps']))

    def test_fresh_build_probe_and_each_failure_propagate(self):
        for failure, expected, calls in (('', 0, 2), ('tests', 7, 1),
                                          ('build', 8, 2), ('probe', 9, 2)):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                bindir = root / 'tools'
                bindir.mkdir()
                python = bindir / 'python3'
                python.write_text(
                    '#!/bin/bash\nset -euo pipefail\n'
                    'echo "$*" >> "$CALLS"\n'
                    'if [[ "$1" == -m ]]; then\n'
                    '  echo recipe-tests\n'
                    '  if [[ "$FAILURE" == tests ]]; then exit 7; fi\n'
                    '  exit 0\nfi\n'
                    '[[ "$*" == "scripts/encoders/svt-av1/build.py --target x86_64-unknown-linux-musl --prefix $RUNNER_TEMP/svt-av1 --jobs 2" ]]\n'
                    '[[ ! -e "$RUNNER_TEMP/svt-av1" ]]\n'
                    'echo native-build\n'
                    'if [[ "$FAILURE" == build ]]; then exit 8; fi\n'
                    'mkdir -p "$RUNNER_TEMP/svt-av1/bin"\n'
                    'printf \'#!/bin/bash\\necho linked-version\\nif [[ "$FAILURE" == probe ]]; then exit 9; fi\\n\' > "$RUNNER_TEMP/svt-av1/bin/svt-av1-version"\n'
                    'chmod +x "$RUNNER_TEMP/svt-av1/bin/svt-av1-version"\n'
                )
                python.chmod(0o755)
                log = root / 'calls'
                result = subprocess.run(
                    ['bash', '-c', self.step['run']], cwd=root,
                    env={**os.environ, 'PATH': str(bindir) + os.pathsep + os.environ['PATH'],
                         'RUNNER_TEMP': str(root), 'TARGET': 'x86_64-unknown-linux-musl',
                         'CALLS': str(log), 'FAILURE': failure},
                    capture_output=True, text=True, timeout=10,
                )
                self.assertEqual(result.returncode, expected, result.stderr)
                self.assertEqual(len(log.read_text().splitlines()), calls)
                self.assertIn('recipe-tests', (root / 'svt-native-logs/tests.log').read_text())
                if failure != 'tests':
                    self.assertIn('native-build', (root / 'svt-native-logs/build.log').read_text())
                if failure in ('', 'probe'):
                    self.assertIn('linked-version', (root / 'svt-native-logs/version.log').read_text())


if __name__ == '__main__':
    unittest.main()
