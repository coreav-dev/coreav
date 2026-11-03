"""Media-free execution of the registered image dispatch and build contract."""
import hashlib
import json
import os
import pathlib
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SHA = 'a' * 40


def workflow():
    return json.loads(subprocess.check_output([
        'ruby', '-ryaml', '-rjson', '-e', 'puts JSON.generate(YAML.load_file(ARGV[0], aliases: true))',
        str(ROOT / '.github/workflows/codec-coverage.yml')], text=True, timeout=30))


class Dispatch(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.jobs = workflow()['jobs']

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = pathlib.Path(self.tmp.name)
        (self.root / 'scripts').mkdir()
        shutil.copyfile(ROOT / 'scripts/png_bmp_yuv_plan.py', self.root / 'scripts/png_bmp_yuv_plan.py')
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.program('python3', '#!/bin/sh\nexec ' + shlex.quote(sys.executable) + ' "$@"\n')
        self.program('git', '#!' + sys.executable + '\n'
                     'import os,sys\n'
                     'args=sys.argv[1:]\n'
                     'if "status" in args: print(os.environ.get("FAKE_DIRTY", ""), end="")\n'
                     'elif "rev-parse" in args: print("a"*40)\n'
                     'elif "merge-base" in args: sys.exit(int(os.environ.get("FAKE_NOT_MAIN", "0")))\n'
                     'elif "cat-file" in args: sys.exit(int(os.environ.get("FAKE_MISSING", "0")))\n'
                     'else: sys.exit(1)\n')
        self.env = {**os.environ, 'PATH': str(self.bin) + os.pathsep + os.environ['PATH'],
                    'GITHUB_OUTPUT': str(self.root / 'outputs'), 'GITHUB_EVENT_NAME': 'workflow_dispatch',
                    'GITHUB_REF': 'refs/heads/main', 'EVENT': 'workflow_dispatch', 'SET': 'v2',
                    'REGISTRATION': 'png-bmp-yuv-v1', 'COMMIT': SHA, 'SHA': 'b' * 40,
                    'R2_ENDPOINT': '', 'R2_ACCESS_KEY_ID': '', 'R2_SECRET_ACCESS_KEY': '', 'R2_BUCKET': ''}

    def program(self, name, text):
        path = self.bin / name
        path.write_text(text)
        path.chmod(0o755)

    def step(self, identifier):
        return next(step for step in self.jobs['coverage-plan']['steps'] if step.get('id') == identifier)

    def run_step(self, identifier, **env):
        output = self.root / 'outputs'
        output.unlink(missing_ok=True)
        result = subprocess.run(['bash', '-e', '-c', self.step(identifier)['run']], cwd=self.root,
                                env={**self.env, **env}, capture_output=True, text=True, timeout=30)
        values = dict(line.split('=', 1) for line in output.read_text().splitlines()) if output.exists() else {}
        return result, values

    def test_manual_main_image_selection_needs_no_corpus_credentials(self):
        result, candidate = self.run_step('candidate')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(candidate, {'commit': SHA, 'registration': 'png-bmp-yuv-v1'})
        result, planned = self.run_step('plan', COMMIT=candidate['commit'], REGISTRATION=candidate['registration'])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(planned, {'commit': SHA, 'registration': 'png-bmp-yuv-v1',
                                   'set': 'v2', 'count': '0', 'shards': '[]'})

    def test_invalid_image_dispatch_stops_before_emitting_a_candidate(self):
        cases = ({'GITHUB_EVENT_NAME': 'pull_request'}, {'GITHUB_REF': 'refs/heads/other'},
                 {'SET': 'v1'}, {'COMMIT': ''}, {'COMMIT': 'main'}, {'COMMIT': SHA[:12]},
                 {'COMMIT': 'A' * 40}, {'FAKE_DIRTY': '?? changed\n'}, {'FAKE_NOT_MAIN': '1'},
                 {'FAKE_MISSING': '1'})
        for env in cases:
            with self.subTest(env=env):
                result, output = self.run_step('candidate', **env)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output, {})
                self.assertIn('image-dispatch:', result.stderr)

    def test_pull_requests_cannot_select_the_judged_image_path(self):
        result, output = self.run_step('plan', EVENT='pull_request', COMMIT='', SET='v2')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output['set'], 'fixtures')
        self.assertEqual(output['registration'], 'legacy')
        self.assertEqual(output['commit'], 'b' * 40)
        condition = self.jobs['coverage-v2-images']['if']
        expected = "github.event_name == 'workflow_dispatch' && needs.coverage-plan.outputs.registration == 'png-bmp-yuv-v1' && needs.coverage-plan.outputs.set == 'v2'"
        self.assertEqual(condition, expected)
        self.assertEqual(next(s for s in self.jobs['coverage-build']['steps'] if s.get('name') ==
                              'Build the full image command and capture harness')['if'],
                         "needs.coverage-plan.outputs.registration == 'png-bmp-yuv-v1'")
        self.assertEqual(self.step('candidate')['if'], "github.event_name != 'pull_request'")
        check = next(step for step in self.jobs['coverage-build']['steps'] if step.get('name') ==
                     'Check the full capture feature combination without acquisition')
        self.assertEqual(check['if'], "github.event_name == 'pull_request'")
        self.assertIn('cargo check --locked -p coreav-cli --all-targets --features release-full,input-capture',
                      check['run'])

    def test_legacy_selection_does_not_gain_a_standalone_v2_bypass(self):
        result, output = self.run_step('plan', REGISTRATION='legacy', SET='v2')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(output, {})

    def test_image_job_rechecks_source_and_keeps_every_evidence_file(self):
        job = self.jobs['coverage-v2-images']
        self.assertEqual(job['runs-on'], 'ubuntu-24.04')
        checkout = job['steps'][0]['with']
        self.assertEqual(checkout['fetch-depth'], 0)
        self.assertFalse(checkout['persist-credentials'])
        scripts = [step['run'] for step in job['steps'] if 'run' in step]
        self.assertIn('png_bmp_yuv_plan.py', scripts[0])
        self.assertIn('git rev-parse HEAD', scripts[0])
        self.assertIn('uname -m', scripts[0])
        self.assertIn('x86_64', scripts[0])
        acquisition = next(text for text in scripts if 'png_bmp_yuv.py run' in text)
        for arg in ('--registration "$REGISTRATION"', '--image-harness', '--build-manifest', '--records',
                    '--summary', '--manifest'):
            self.assertIn(arg, acquisition)
        retained = next(step for step in job['steps'] if step.get('name') ==
                        'Retain complete image capture and process evidence')
        self.assertEqual(retained['if'], 'always()')
        self.assertIn('tar --sparse', retained['run'])
        self.assertIn('image-work', retained['run'])
        self.assertIn('fixture-check', retained['run'])
        self.assertEqual(job['steps'][-1]['if'], 'always()')

    def test_image_build_manifest_binds_the_three_actual_full_builds(self):
        step = next(step for step in self.jobs['coverage-build']['steps'] if step.get('name') ==
                    'Build the full image command and capture harness')
        (self.root / 'Cargo.lock').write_bytes(b'synthetic lock identity')
        dist = self.root / 'dist'
        dist.mkdir()
        (dist / 'ffmpeg-harness').write_bytes(b'ordinary harness')
        self.program('rustc', '#!/bin/sh\nprintf "rustc synthetic\\nhost: x86_64-unknown-linux-gnu\\n"\n')
        self.program('cargo', '#!' + sys.executable + '\n'
                     'import json,pathlib,sys\n'
                     'root=pathlib.Path.cwd(); target=root/"target/release"; target.mkdir(parents=True,exist_ok=True)\n'
                     'if sys.argv[1]=="build": (target/"coreav").write_bytes(b"ordinary command")\n'
                     'else:\n'
                     ' p=target/"image-harness"; p.write_bytes(b"capture harness")\n'
                     ' print(json.dumps({"reason":"compiler-artifact","target":{"name":"image_capture"},"executable":str(p)}))\n')
        result = subprocess.run(['bash', '-e', '-c', step['run']], cwd=self.root,
                                env={**self.env, 'RUNNER_TEMP': str(self.root), 'CODECS_GNU': str(self.root / 'codecs')},
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        manifest = json.loads((dist / 'manifest-image-build.json').read_text())
        self.assertEqual(manifest['schema'], 'coreav.image-build/1')
        self.assertEqual(manifest['source_sha'], SHA)
        self.assertEqual(manifest['target'], 'x86_64-unknown-linux-gnu')
        self.assertEqual(manifest['cargo_lock_sha256'], hashlib.sha256(b'synthetic lock identity').hexdigest())
        for name, filename in (('coreav', 'coreav'), ('harness', 'ffmpeg-harness'), ('image_harness', 'image-harness')):
            self.assertEqual(manifest['binaries'][name]['sha256'], hashlib.sha256((dist / filename).read_bytes()).hexdigest())
            self.assertEqual(manifest['binaries'][name]['features'],
                             ['release-full'] + (['input-capture'] if name == 'image_harness' else []))
        self.assertIn('--features release-full --bin coreav', step['run'])
        self.assertIn('--features release-full,input-capture', step['run'])
        self.assertNotRegex(step['run'], re.compile(r'--features\s+native-h264\b'))


if __name__ == '__main__':
    unittest.main()
