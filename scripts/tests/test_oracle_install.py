"""Run the Linux installer with local archives and real HTTP failures."""
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import oracle_pin
import workflow_check


class OracleInstall(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.scripts = self.root / 'scripts'
        self.scripts.mkdir()
        for name in ('install-ci-oracle-ffmpeg.sh', 'oracle_pin.py'):
            shutil.copyfile(ROOT / 'scripts' / name, self.scripts / name)
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        uname = self.tools / 'uname'
        uname.write_text('#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n')
        uname.chmod(0o755)
        self.dest = self.root / 'bin'
        self.cache = self.root / 'cache' / 'archive.tar.xz'
        self.pin = oracle_pin.load()
        self.pin['ARCHIVE'] = 'oracle-fixture'
        self.programs = {}
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode='w:xz') as tar:
            for name in ('ffmpeg', 'ffprobe'):
                data = f'#!/bin/sh\necho "{name} version {self.pin["BUILD"]}"\n'.encode()
                self.programs[name] = data
                member = tarfile.TarInfo(f'{self.pin["ARCHIVE"]}/bin/{name}')
                member.size, member.mode = len(data), 0o755
                tar.addfile(member, io.BytesIO(data))
        self.archive = archive.getvalue()
        self.pin['SHA256'] = hashlib.sha256(self.archive).hexdigest()
        self.statuses = []
        self.requests = []
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                status = owner.statuses.pop(0) if owner.statuses else 200
                owner.requests.append(status)
                self.send_response(status)
                self.end_headers()
                self.wfile.write(owner.archive if status == 200 else b'unavailable')

            def log_message(self, *_args):
                pass

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()

        def stop():
            server.shutdown()
            server.server_close()
            thread.join()

        self.addCleanup(stop)
        self.pin['URL'] = f'http://127.0.0.1:{server.server_port}/archive'
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(('R2_', 'COREAV_ORACLE_'))}
        self.env.update(PATH=str(self.tools) + os.pathsep + os.environ['PATH'],
                        COREAV_ORACLE_ARCHIVE=str(self.cache), NO_PROXY='127.0.0.1')

    def install(self):
        (self.scripts / 'oracle-ffmpeg.env').write_text(''.join(f'{k}={v}\n' for k, v in self.pin.items()))
        return subprocess.run(['bash', str(self.scripts / 'install-ci-oracle-ffmpeg.sh'), str(self.dest)],
                              env=self.env, capture_output=True, text=True, timeout=45)

    def assert_programs(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for name, data in self.programs.items():
            self.assertEqual((self.dest / name).read_bytes(), data)
            self.assertIn(f'{name} version {self.pin["BUILD"]}', result.stdout)

    def assert_installed(self, result):
        self.assert_programs(result)
        self.assertEqual(self.cache.read_bytes(), self.archive)

    def test_http_503_504_and_404_recover_with_real_curl_backoff(self):
        self.statuses = [503, 504, 404, 200]
        result = self.install()
        self.assert_installed(result)
        self.assertEqual(self.requests, [503, 504, 404, 200])
        self.assertNotIn('R2', result.stderr)

    def test_cache_hit_avoids_a_404_and_replaces_independent_binary_cache(self):
        self.cache.parent.mkdir()
        self.cache.write_bytes(self.archive)
        self.dest.mkdir()
        for name in self.programs:
            (self.dest / name).write_text('unverified binary cache')
        self.statuses = [404]
        self.assert_installed(self.install())
        self.assertEqual(self.requests, [])

    def test_second_install_in_one_job_reuses_verified_bytes_without_credentials(self):
        self.assert_installed(self.install())
        self.statuses = [503, 404]
        for name in self.programs:
            (self.dest / name).write_bytes(b'unverified replacement')
        self.assert_installed(self.install())
        self.assertEqual(self.requests, [200])
        self.assertEqual(self.statuses, [503, 404])

    def test_corrupt_cache_is_replaced_only_with_verified_download(self):
        self.cache.parent.mkdir()
        self.cache.write_bytes(b'corrupt cache')
        result = self.install()
        self.assert_installed(result)
        self.assertEqual(self.requests, [200])
        self.assertIn('discarding an unverified cached archive', result.stderr)

    def test_unreadable_cache_falls_back_to_a_verified_download(self):
        self.cache.parent.mkdir()
        self.cache.write_bytes(self.archive)
        tool = self.tools / 'cp'
        tool.write_text('#!/bin/sh\n[ "$1" != "$COREAV_ORACLE_ARCHIVE" ] || exit 1\n' +
                        f'exec "{shutil.which("cp")}" "$@"\n')
        tool.chmod(0o755)
        self.assert_installed(self.install())
        self.assertEqual(self.requests, [200])

    def test_unwritable_cache_does_not_fail_verified_installation(self):
        self.cache.parent.write_bytes(b'not a directory')
        result = self.install()
        self.assert_programs(result)
        self.assertIn('could not update the archive cache', result.stderr)
        self.assertEqual(self.requests, [200])
        self.assertEqual(self.cache.parent.read_bytes(), b'not a directory')

    def test_failed_cache_publication_removes_its_partial_file(self):
        tool = self.tools / 'mv'
        tool.write_text('#!/bin/sh\nexit 1\n')
        tool.chmod(0o755)
        result = self.install()
        self.assert_programs(result)
        self.assertIn('could not update the archive cache', result.stderr)
        self.assertEqual(list(self.cache.parent.iterdir()), [])

    def test_wrong_download_hash_fails_without_r2_or_publishing(self):
        self.archive += b'wrong bytes'
        result = self.install()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('oracle_download_unavailable', result.stderr)
        self.assertNotIn('R2', result.stderr)
        self.assertFalse(self.cache.exists())
        self.assertFalse((self.dest / 'ffmpeg').exists())

    def test_bad_build_in_a_checksum_valid_cache_is_not_installed(self):
        self.cache.parent.mkdir()
        self.cache.write_bytes(self.archive)
        self.pin['BUILD'] = 'unregistered'
        result = self.install()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('expected the', result.stderr)
        self.assertEqual(self.requests, [])
        self.assertFalse((self.dest / 'ffmpeg').exists())

    def test_credentialed_r2_override_still_checks_the_archive(self):
        fixture = self.root / 'r2-archive'
        fixture.write_bytes(self.archive)
        (self.scripts / 'oracle_from_r2.py').write_text(
            'import os, shutil, sys\nshutil.copyfile(os.environ["R2_FIXTURE"], sys.argv[1])\n')
        self.env.update(COREAV_ORACLE_FROM_R2='1', R2_ACCESS_KEY_ID='fixture',
                        R2_SECRET_ACCESS_KEY='fixture', R2_ENDPOINT='fixture', R2_FIXTURE=str(fixture))
        self.assert_installed(self.install())
        self.cache.unlink()
        fixture.write_bytes(b'corrupt mirror')
        result = self.install()
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse(self.cache.exists())
        self.assertEqual(self.requests, [])


class OracleArchiveWorkflow(unittest.TestCase):
    def test_native_h264_cli_uses_the_shared_verified_archive(self):
        data = self.data(ROOT / '.github/workflows/h264-encoder.yml')
        job = data['jobs']['native-h264-cli']
        candidate = job['defaults']['run']['working-directory']
        self.assertTrue(any(step.get('with', {}).get('path') == candidate and
                            'ref' not in step.get('with', {}) for step in job['steps']))
        owner, = [step for step in job['steps'] if 'sparse-checkout' in step.get('with', {})]
        self.assertNotIn('path', owner['with'])
        self.assertNotIn('ref', owner['with'])
        self.assertFalse(owner['with']['sparse-checkout-cone-mode'])
        self.assertEqual(owner['with']['sparse-checkout'].splitlines(),
                         ['.github/actions/oracle-cache', 'scripts/oracle-ffmpeg.env'])
        cache, = [index for index, step in enumerate(job['steps'])
                  if step.get('uses') == './.github/actions/oracle-cache']
        install, = [index for index, step in enumerate(job['steps']) if any(
            workflow_check.calls_installer(line, workflow_check.CI_ORACLE_PIN[1])
            for line in step.get('run', '').splitlines())]
        self.assertLess(cache, install)
        self.assertNotIn('if', job['steps'][install])
        run = job['steps'][install]['run']
        self.assertIn('scripts/oracle_pin.py --verify-bin "$RUNNER_TEMP/cli-oracles"', run)
        self.assertLess(run.index('--verify-bin'), run.index('scripts/gen-fixtures.sh'))
        self.assertFalse(any(workflow_check.calls_installer(line) for line in run.splitlines()))
        triggers = data.get('on', data.get('true'))
        for event in ('push', 'pull_request'):
            for path in workflow_check.CI_ORACLE_PIN:
                self.assertIn(path, triggers[event]['paths'])

    def test_local_actions_cannot_keep_the_frozen_installer(self):
        consumers = 0
        for path in (ROOT / '.github/actions').glob('*/action.y*ml'):
            steps = self.data(path).get('runs', {}).get('steps', [])
            with self.subTest(action=str(path.relative_to(ROOT))):
                self.assertFalse(any(workflow_check.calls_installer(line)
                                     for step in steps for line in step.get('run', '').splitlines()))
                for index, step in enumerate(steps):
                    if not any(workflow_check.calls_installer(line, workflow_check.CI_ORACLE_PIN[1])
                               for line in step.get('run', '').splitlines()):
                        continue
                    cache, = [s for s in steps[:index] if s.get('uses') == './.github/actions/oracle-cache']
                    self.assertEqual(cache.get('if'), step.get('if'))
                    self.assertNotIn('ffmpeg-cache', step.get('if', ''))
                    consumers += 1
        self.assertGreater(consumers, 0)

    def test_fixture_producers_compute_the_same_cache_key(self):
        action = self.data(ROOT / '.github/actions/ci-fixtures/action.yml')['runs']['steps']
        wasm = self.data(ROOT / '.github/workflows/wasm.yml')['jobs']['wasm-module']['steps']
        keys = []
        for steps, name in ((action, 'key'), (wasm, 'fixtures-key')):
            script, = [step['run'] for step in steps if step.get('id') == name]
            with tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / 'output'
                run = subprocess.run(['bash', '-euo', 'pipefail', '-c', script], cwd=ROOT,
                                     env=os.environ | {'GITHUB_OUTPUT': str(output)},
                                     capture_output=True, text=True, timeout=30)
                self.assertEqual(run.returncode, 0, run.stderr)
                key, = [line for line in output.read_text().splitlines() if line.startswith('key=')]
                keys.append(key)
        self.assertEqual(*keys)

    def test_hevc_consumers_share_the_wasm_archive_cache(self):
        wasm = self.data(ROOT / '.github/workflows/wasm.yml')['jobs']['wasm-module']['steps']
        shared, = [step for step in wasm if step.get('uses') == './.github/actions/oracle-cache']
        self.assertFalse(any(step.get('uses', '').startswith('actions/cache@') and
                             'coreav-oracle-bin' in str(step.get('with', {}).get('path', ''))
                             for step in wasm))
        jobs = self.data(ROOT / '.github/workflows/hevc-conformance.yml')['jobs']
        for name in ('hevc-smoke', 'hevc-vectors', 'profile-census'):
            with self.subTest(job=name):
                steps = jobs[name]['steps']
                cache, = [step for step in steps if step.get('uses') == shared['uses']]
                self.assertEqual(cache, shared)
                installer, = [step for step in steps if any(
                    workflow_check.calls_installer(line, workflow_check.CI_ORACLE_PIN[1])
                    for line in step.get('run', '').splitlines())]
                self.assertLess(steps.index(cache), steps.index(installer))
                self.assertNotIn('cache-hit', installer.get('if', ''))
                self.assertNotIn('secrets.', str(installer))

    def test_image_controls_use_the_secretless_ci_installer(self):
        data = self.data(ROOT / '.github/workflows/png-bmp-yuv.yml')
        steps = data['jobs']['image-synthetic-controls']['steps']
        cache = next(i for i, step in enumerate(steps) if step.get('uses') == './.github/actions/oracle-cache')
        installer = next(i for i, step in enumerate(steps) if any(
            workflow_check.calls_installer(line, workflow_check.CI_ORACLE_PIN[1])
            for line in step.get('run', '').splitlines()))
        self.assertLess(cache, installer)
        self.assertFalse(any(name.startswith('R2_') for name in steps[installer].get('env', {})))

    def test_videotoolbox_modes_share_one_provisioned_archive_cache(self):
        data = self.data(ROOT / '.github/workflows/videotoolbox.yml')
        steps = data['jobs']['verify']['steps']
        cache = next(i for i, step in enumerate(steps) if step.get('uses') == './.github/actions/oracle-cache')
        installer = next(i for i, step in enumerate(steps) if any(
            workflow_check.calls_installer(line, workflow_check.CI_ORACLE_PIN[1])
            for line in step.get('run', '').splitlines()))
        replay = next(i for i, step in enumerate(steps)
                      if 'scripts/encoders/macos/smoke.py verify' in step.get('run', ''))
        self.assertLess(cache, installer)
        self.assertLess(installer, replay)
        self.assertIn('for mode in hardware software any', steps[replay]['run'])
        self.assertNotIn('--install-oracle', steps[replay]['run'])
        triggers = data.get('on', data.get('true'))
        for event in ('push', 'pull_request'):
            for path in workflow_check.CI_ORACLE_PIN:
                self.assertIn(path, triggers[event]['paths'])

    def test_historical_measurements_keep_the_frozen_installer(self):
        rosters = {'github-shapes.yml': ('measure',),
                   'codec-coverage.yml': ('coverage-v1', 'coverage-v1-fixtures',
                                          'coverage-v2-v3', 'coverage-v2-images')}
        for workflow, jobs in rosters.items():
            data = self.data(ROOT / '.github/workflows' / workflow)
            for job in jobs:
                with self.subTest(workflow=workflow, job=job):
                    steps = data['jobs'][job]['steps']
                    self.assertFalse(any(s.get('uses') == './.github/actions/oracle-cache' for s in steps))
                    run = '\n'.join(s.get('run', '') for s in steps)
                    self.assertIn('scripts/install-oracle-ffmpeg.sh', run)
                    self.assertNotIn(workflow_check.CI_ORACLE_PIN[1], run)

    def test_the_ci_installer_cannot_duplicate_the_pin(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'scripts').mkdir()
            shutil.copyfile(ROOT / oracle_pin.PIN, root / oracle_pin.PIN)
            installer = root / workflow_check.CI_ORACLE_PIN[1]
            installer.write_text(f'URL={oracle_pin.load()["URL"]}\n')
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
            found = list(oracle_pin.problems(root))
            self.assertTrue(any(workflow_check.CI_ORACLE_PIN[1] in problem for problem in found), found)

    @staticmethod
    def data(path):
        return json.loads(subprocess.check_output(
            ['ruby', '-ryaml', '-rjson', '-e',
             'puts JSON.generate(YAML.load_file(ARGV[0], aliases: true))', str(path)]))

    def test_archive_cache_is_keyed_only_by_the_pin_digest(self):
        action = ROOT / '.github/actions/oracle-cache'
        data = self.data(action / 'action.yml')
        pin, cache = data['runs']['steps']
        self.assertEqual(cache['with']['key'], 'oracle-archive-${{ steps.pin.outputs.sha256 }}')
        self.assertEqual(cache['with']['path'],
                         '${{ runner.temp }}/coreav-oracle-archive/${{ steps.pin.outputs.sha256 }}.tar.xz')
        self.assertNotIn('restore-keys', cache['with'])
        with tempfile.TemporaryDirectory() as directory:
            output, env = Path(directory) / 'output', Path(directory) / 'env'
            subprocess.run(['bash', '-e', '-c', pin['run']], check=True, env=os.environ | {
                'GITHUB_ACTION_PATH': str(action), 'GITHUB_OUTPUT': str(output),
                'GITHUB_ENV': str(env), 'RUNNER_TEMP': directory})
            sha = oracle_pin.load()['SHA256']
            self.assertEqual(output.read_text(), f'sha256={sha}\n')
            self.assertEqual(env.read_text(),
                             f'COREAV_ORACLE_ARCHIVE={directory}/coreav-oracle-archive/{sha}.tar.xz\n')

    def test_cache_users_always_reinstall_from_the_verified_archive(self):
        count = 0
        for path in (ROOT / '.github/workflows').glob('*.yml'):
            for job in self.data(path).get('jobs', {}).values():
                steps = job.get('steps', [])
                caches = [i for i, s in enumerate(steps)
                          if s.get('uses') == './.github/actions/oracle-cache']
                installers = [s for s in steps if any(workflow_check.calls_installer(line, workflow_check.CI_ORACLE_PIN[1])
                              for line in s.get('run', '').splitlines())]
                if installers:
                    self.assertTrue(caches, str(path))
                for i in caches:
                    installer = next((s for s in steps[i + 1:] if any(
                        workflow_check.calls_installer(line, workflow_check.CI_ORACLE_PIN[1]) for line in s.get('run', '').splitlines())), None)
                    self.assertIsNotNone(installer, str(path))
                    self.assertNotIn('cache-hit', installer.get('if', ''), str(path))
                    self.assertEqual(steps[i].get('if'), installer.get('if'), str(path))
                    count += 1
        self.assertGreater(count, 30)


if __name__ == '__main__':
    unittest.main()
