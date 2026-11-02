"""Artifact-download failures must not suppress the installation result ledger."""
import json
import os
import pathlib
import shlex
import shutil
import subprocess
import tempfile
import unittest

from test_registered_candidate import load_workflow

ROOT = pathlib.Path(__file__).resolve().parents[2]


class PackageRehearsal(unittest.TestCase):
    def test_failed_downloads_still_attempt_other_downloads_and_retain_all_outcomes(self):
        steps = load_workflow('package-rehearsal.yml')['jobs']['installed-packages']['steps']
        downloads = [step for step in steps if step.get('uses', '').startswith('actions/download-artifact@')]
        exercise = next(step for step in steps if step.get('name') == 'Install and exercise the artifacts')
        upload = next(step for step in steps if step.get('name') == 'Retain each package outcome')
        platform = subprocess.check_output(['node', '-p', '`${process.platform}-${process.arch}`'], text=True).strip()
        for failed in range(len(downloads)):
            with self.subTest(failed=failed), tempfile.TemporaryDirectory() as tmp:
                directory = pathlib.Path(tmp)
                attempted = []
                failed_step = False
                for index, step in enumerate(downloads):
                    expected = ("${{ !cancelled() && matrix.platform != 'linux-x64' }}"
                                if step['with']['path'] == 'artifacts/npm-native' else '${{ !cancelled() }}')
                    self.assertEqual(step.get('if'), expected)
                    self.assertFalse(step.get('continue-on-error', False))
                    attempted.append(step['with']['name'])
                    if index == failed:
                        failed_step = True
                    else:
                        (directory / step['with']['path']).mkdir(parents=True)
                self.assertTrue(failed_step)
                self.assertEqual(len(attempted), 4)
                self.assertEqual(exercise.get('if'), '${{ !cancelled() }}')
                script = directory / 'checkout with spaces/packages/cli/test/package-rehearsal.mjs'
                script.parent.mkdir(parents=True)
                shutil.copyfile(ROOT / 'packages/cli/test/package-rehearsal.mjs', script)
                shutil.copyfile(ROOT / 'packages/cli/package.json', script.parent.parent / 'package.json')
                command = shlex.split(exercise['run'])
                self.assertEqual(command[1], 'packages/cli/test/package-rehearsal.mjs')
                command[1] = str(script)
                done = subprocess.run(command, cwd=directory,
                                      env={**os.environ, 'PLATFORM': platform, 'TARGET': 'missing-target'},
                                      capture_output=True, timeout=30)
                self.assertEqual(done.returncode, 1, done.stderr.decode())
                self.assertEqual(upload['if'], 'always()')
                results = json.loads((directory / upload['with']['path']).read_text())
                self.assertEqual({row['package'] for row in results},
                                 {'archive-default', 'archive-full', 'npm-native', 'npm-wasi', 'wasm-node'})
                self.assertTrue(all(row['result'] == 'fail' and row['detail'] for row in results))

    def test_no_downloaded_directory_still_retains_results(self):
        with tempfile.TemporaryDirectory() as tmp:
            artifacts = pathlib.Path(tmp) / 'absent'
            platform = subprocess.check_output(['node', '-p', '`${process.platform}-${process.arch}`'], text=True).strip()
            done = subprocess.run(['node', str(ROOT / 'packages/cli/test/package-rehearsal.mjs'), str(artifacts)],
                                  env={**os.environ, 'PLATFORM': platform}, capture_output=True, timeout=30)
            self.assertEqual(done.returncode, 1, done.stderr.decode())
            self.assertEqual(len(json.loads((artifacts / 'results.json').read_text())), 5)
