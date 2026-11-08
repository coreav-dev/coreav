import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import threading
import unittest

ROOT = Path(__file__).resolve().parents[2]


def yaml(path):
    return json.loads(subprocess.check_output(
        ['ruby', '-ryaml', '-rjson', '-e',
         'puts JSON.generate(YAML.load_file(ARGV[0], aliases: true))', str(path)]))


class VerifiedDownload(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.body = b'verified synthetic archive\n'
        self.digest = hashlib.sha256(self.body).hexdigest()
        self.statuses = []
        self.requests = []
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                status = owner.statuses.pop(0) if owner.statuses else 200
                owner.requests.append(status)
                self.send_response(status)
                self.end_headers()
                self.wfile.write(owner.body if status == 200 else b'unavailable')

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
        self.url = f'http://127.0.0.1:{server.server_port}/archive'
        self.output = self.root / 'output'
        self.cache = self.root / 'cache' / 'archive'
        self.env = {'PATH': os.environ['PATH'], 'HOME': str(self.root), 'NO_PROXY': '127.0.0.1'}

    def fetch(self, digest=None):
        return subprocess.run(['bash', str(ROOT / 'scripts/fetch-ci-archive.sh'), self.url,
                               digest or self.digest, str(self.output), str(self.cache)],
                              env=self.env, capture_output=True, text=True, timeout=30)

    def test_real_curl_recovers_from_http_503_and_504(self):
        self.statuses = [503, 504, 200]
        result = self.fetch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.requests, [503, 504, 200])
        self.assertEqual(self.output.read_bytes(), self.body)
        self.assertEqual(self.cache.read_bytes(), self.body)

    def test_cache_hit_rechecks_bytes_without_network(self):
        self.cache.parent.mkdir()
        self.cache.write_bytes(self.body)
        self.output.write_bytes(b'unverified prior output')
        self.statuses = [404]
        result = self.fetch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.output.read_bytes(), self.body)
        self.assertEqual(self.requests, [])

    def test_corrupt_cache_downloads_and_verifies_a_replacement(self):
        self.cache.parent.mkdir()
        self.cache.write_bytes(b'corrupt cache')
        result = self.fetch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('ci_archive_cache_rejected', result.stderr)
        self.assertEqual(self.requests, [200])
        self.assertEqual(self.output.read_bytes(), self.body)
        self.assertEqual(self.cache.read_bytes(), self.body)

    def test_mismatched_download_is_neither_published_nor_installed(self):
        result = self.fetch('0' * 64)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('reason=ci_archive_digest_mismatch', result.stderr)
        self.assertFalse(self.output.exists())
        self.assertFalse(self.cache.exists())

    def test_unwritable_cache_is_advisory(self):
        self.cache.parent.write_text('not a directory')
        result = self.fetch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.output.read_bytes(), self.body)
        self.assertIn('ci_archive_cache_write_failed', result.stderr)

    def test_failed_cache_publication_cleans_the_partial_file(self):
        tools = self.root / 'tools'
        tools.mkdir()
        (tools / 'mv').write_text('#!/bin/sh\nexit 1\n')
        (tools / 'mv').chmod(0o755)
        self.env['PATH'] = str(tools) + os.pathsep + self.env['PATH']
        result = self.fetch()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.output.read_bytes(), self.body)
        self.assertEqual(list(self.cache.parent.iterdir()), [])

    def test_failed_transfer_has_finite_retry_and_stall_limits(self):
        tools = self.root / 'tools'
        tools.mkdir()
        calls = self.root / 'calls'
        (tools / 'curl').write_text(
            '#!/usr/bin/env python3\nimport json, sys\n'
            f'open({str(calls)!r}, "w").write(json.dumps(sys.argv[1:]))\n'
            'sys.exit(22)\n')
        (tools / 'curl').chmod(0o755)
        self.env['PATH'] = str(tools) + os.pathsep + self.env['PATH']
        result = self.fetch()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('ci_archive_download_unavailable', result.stderr)
        arguments = json.loads(calls.read_text())
        for flag, value in (('--retry', '8'), ('--retry-max-time', '360'),
                            ('--max-time', '300'), ('--connect-timeout', '20'),
                            ('--speed-limit', '1024'), ('--speed-time', '30')):
            self.assertEqual(arguments[arguments.index(flag) + 1], value)
        self.assertIn('--retry-all-errors', arguments)
        self.assertIn('--retry-connrefused', arguments)
        self.assertFalse(self.output.exists())


class CheckToolInstallation(unittest.TestCase):
    def test_both_tools_reinstall_from_verified_archives(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scripts = root / 'scripts'
            scripts.mkdir()
            for name in ('fetch-ci-archive.sh', 'install-ci-check-tool.sh'):
                shutil.copyfile(ROOT / 'scripts' / name, scripts / name)
            tools = root / 'tools'
            tools.mkdir()
            (tools / 'uname').write_text('#!/bin/sh\necho "Linux x86_64"\n')
            (tools / 'uname').chmod(0o755)
            cache = root / 'cache'
            cache.mkdir()
            pins = {}
            bodies = {}
            for tool, prefix, member in (('actionlint', 'ACTIONLINT', 'actionlint'),
                                         ('cargo-deny', 'CARGO_DENY', 'cargo-deny-1.2.3-x86_64-unknown-linux-musl/cargo-deny')):
                program = b'#!/bin/sh\necho "1.2.3"\n'
                data = io.BytesIO()
                with tarfile.open(fileobj=data, mode='w:gz') as archive:
                    item = tarfile.TarInfo(member)
                    item.size, item.mode = len(program), 0o755
                    archive.addfile(item, io.BytesIO(program))
                digest = hashlib.sha256(data.getvalue()).hexdigest()
                pins[prefix + '_VERSION'] = '1.2.3'
                pins[prefix + '_SHA256'] = digest
                (cache / f'{tool}-1.2.3-{digest}.archive').write_bytes(data.getvalue())
                bodies[tool] = program
            (scripts / 'ci-tools.env').write_text(''.join(f'{key}={value}\n' for key, value in pins.items()))
            env = {'PATH': str(tools) + os.pathsep + os.environ['PATH'],
                   'COREAV_CI_ARCHIVE_DIR': str(cache), 'HOME': str(root)}
            for tool, expected in bodies.items():
                with self.subTest(tool=tool):
                    dest = root / 'bin'
                    dest.mkdir(exist_ok=True)
                    (dest / tool).write_bytes(b'unverified loose binary')
                    result = subprocess.run(['bash', str(scripts / 'install-ci-check-tool.sh'), tool, str(dest)],
                                            env=env, capture_output=True, text=True, timeout=30)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual((dest / tool).read_bytes(), expected)


class DownloadWorkflowWiring(unittest.TestCase):
    def test_deny_names_every_new_download_regression_suite(self):
        steps = yaml(ROOT / '.github/workflows/deny.yml')['jobs']['deny-checks']['steps']
        run = '\n'.join(step.get('run', '') for step in steps)
        for name in ('test_ci_downloads.py', 'test_ci_reference_downloads.py', 'test_maturin_downloads.py'):
            self.assertIn(name, run)
        self.assertIn('python3 -I -m unittest discover -s scripts/tests -p "$suite" -v', run)

    def test_cache_identity_is_version_and_digest_without_fallback_keys(self):
        action = ROOT / '.github/actions/ci-archive-cache'
        read, cache = yaml(action / 'action.yml')['runs']['steps']
        self.assertEqual(cache['with']['key'],
                         'ci-tool-archive-${{ inputs.tool }}-${{ inputs.version }}-${{ inputs.sha256 }}')
        self.assertNotIn('restore-keys', cache['with'])
        with tempfile.TemporaryDirectory() as directory:
            output, env = Path(directory) / 'output', Path(directory) / 'env'
            result = subprocess.run(['bash', '-e', '-c', read['run']], env=os.environ | {
                'TOOL': 'tool', 'VERSION': '1.2.3', 'SHA256': 'a' * 64, 'RUNNER_TEMP': directory,
                'GITHUB_OUTPUT': str(output), 'GITHUB_ENV': str(env)}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(output.read_text(), f'path={directory}/coreav-ci-archives/tool-1.2.3-{"a" * 64}.archive\n')
            self.assertEqual(env.read_text(), f'COREAV_CI_ARCHIVE_DIR={directory}/coreav-ci-archives\n')

    def test_check_tool_actions_never_skip_installation_on_cache_hits(self):
        steps = yaml(ROOT / '.github/actions/ci-check-tool/action.yml')['runs']['steps']
        cache, = [i for i, step in enumerate(steps) if step.get('uses') == './.github/actions/ci-archive-cache']
        install, = [i for i, step in enumerate(steps) if 'install-ci-check-tool.sh' in step.get('run', '')]
        self.assertLess(cache, install)
        self.assertNotIn('if', steps[install])
        deny = yaml(ROOT / '.github/workflows/deny.yml')['jobs']['deny-checks']['steps']
        for tool in ('actionlint', 'cargo-deny'):
            self.assertTrue(any(step.get('uses') == './.github/actions/ci-check-tool' and
                                step.get('with', {}).get('tool') == tool for step in deny))
        self.assertFalse(any(step.get('uses', '').startswith('EmbarkStudios/cargo-deny-action@') for step in deny))
        policy, = [step for step in deny if step.get('name') == 'Check the workspace dependency policy']
        self.assertIn('cargo-deny --log-level warn --manifest-path ./Cargo.toml --all-features check', policy['run'])

    def test_standalone_licenses_use_the_same_pinned_tool_and_trigger_inputs(self):
        workflow = yaml(ROOT / '.github/workflows/videotoolbox.yml')
        steps = workflow['jobs']['verify']['steps']
        self.assertTrue(any(step.get('uses') == './.github/actions/ci-check-tool' and
                            step.get('with', {}).get('tool') == 'cargo-deny' for step in steps))
        check, = [step for step in steps if step.get('name') == 'Check the standalone dependency licenses']
        self.assertIn('--manifest-path crates/coreav-backend-apple/Cargo.toml --all-features check licenses', check['run'])
        triggers = workflow.get('on', workflow.get('true'))
        for event in ('push', 'pull_request'):
            for path in ('scripts/ci-tools.env', 'scripts/fetch-ci-archive.sh', 'scripts/install-ci-check-tool.sh',
                         '.github/actions/ci-archive-cache/action.yml', '.github/actions/ci-check-tool/action.yml'):
                self.assertIn(path, triggers[event]['paths'])


if __name__ == '__main__':
    unittest.main()
