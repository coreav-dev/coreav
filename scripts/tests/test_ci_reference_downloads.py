import hashlib
import io
import json
import os
import pathlib
import re
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import textwrap
import unittest
import zipfile


ROOT = pathlib.Path(__file__).resolve().parents[2]


class ReferenceDownloads(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.certificates = tempfile.TemporaryDirectory()
        cls.certs = pathlib.Path(cls.certificates.name)
        for name in ('root', 'extra'):
            cls.openssl('req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
                        '-subj', f'/CN=Synthetic-{name}', '-keyout', str(cls.certs / f'{name}.key'),
                        '-out', str(cls.certs / f'{name}.pem'))
        cls.openssl('req', '-newkey', 'rsa:2048', '-nodes', '-subj', '/CN=Synthetic-intermediate',
                    '-keyout', str(cls.certs / 'intermediate.key'), '-out', str(cls.certs / 'intermediate.csr'))
        (cls.certs / 'extensions').write_text('basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n')
        cls.openssl('x509', '-req', '-in', str(cls.certs / 'intermediate.csr'),
                    '-CA', str(cls.certs / 'root.pem'), '-CAkey', str(cls.certs / 'root.key'),
                    '-set_serial', '2', '-days', '1', '-extfile', str(cls.certs / 'extensions'),
                    '-out', str(cls.certs / 'intermediate.pem'))
        cls.der = cls.openssl('x509', '-in', str(cls.certs / 'intermediate.pem'), '-outform', 'DER').stdout

    @classmethod
    def tearDownClass(cls):
        cls.certificates.cleanup()

    @staticmethod
    def openssl(*args):
        return subprocess.run(['openssl', *args], check=True, capture_output=True, timeout=30)

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.scripts = self.root / 'scripts'
        self.scripts.mkdir()
        self.commands = self.root / 'commands'
        self.commands.mkdir()
        self.cache = self.root / 'cache'
        self.cache.mkdir()
        self.dest = self.root / 'installed'
        self.ca_digest = hashlib.sha256(self.der).hexdigest()
        self.ca_input = self.root / 'download.pem'
        self.ca_input.write_bytes((self.certs / 'intermediate.pem').read_bytes() + (self.certs / 'extra.pem').read_bytes())
        self.env = dict(os.environ, PATH=f'{self.commands}:{os.environ["PATH"]}',
                        COREAV_CI_ARCHIVE_DIR=str(self.cache), SSL_CERT_FILE=str(self.certs / 'root.pem'),
                        CA_INPUT=str(self.ca_input), CALLS=str(self.root / 'calls'),
                        CAPTURE=str(self.root / 'bundle.pem'), PYTHON=sys.executable,
                        FAKE_PLATFORM='Linux x86_64')
        for name in ('install-ci-binaryen.sh', 'install-ci-jm-decoder.sh'):
            shutil.copyfile(ROOT / 'scripts' / name, self.scripts / name)
        self.command('uname', '#!/bin/bash\nprintf "%s\\n" "$FAKE_PLATFORM"\n')
        self.command('curl', '''#!/bin/bash
printf 'curl %s\n' "$*" >> "$CALLS"
if [ "${CA_CURL_EXIT:-0}" != 0 ]; then exit "$CA_CURL_EXIT"; fi
while [ "$1" != -o ]; do shift; done
cp "$CA_INPUT" "$2"
''')
        self.command('make', '''#!/bin/bash
printf 'make %s\n' "$*" >> "$CALLS"
test "$1" = -C && test "$3" = -j2 && test "$4" = 'CC=cc -fcommon' || exit 61
printf '#!/bin/bash\nexit 0\n' > "$2/ldecod"
chmod +x "$2/ldecod"
''')
        self.command('cc', '#!/bin/bash\necho synthetic-compiler\n')
        self.command('sha256sum', '#!/bin/bash\nexec shasum -a 256 "$@"\n')
        (self.scripts / 'fetch-ci-archive.sh').write_text('''#!/bin/bash
exec "$PYTHON" - "$@" <<'PY'
import hashlib, json, os, pathlib, sys
url, digest, output, cache, *args = sys.argv[1:]
with open(os.environ['CALLS'], 'a') as log:
    log.write('fetch ' + json.dumps([url, digest, output, cache, *args]) + '\\n')
if args:
    assert args[0] == '--cacert'
    pathlib.Path(os.environ['CAPTURE']).write_bytes(pathlib.Path(args[1]).read_bytes())
if os.environ.get('FETCH_EXIT'):
    sys.exit(int(os.environ['FETCH_EXIT']))
cached = pathlib.Path(cache)
data = cached.read_bytes() if cached.is_file() else b''
if hashlib.sha256(data).hexdigest() != digest:
    data = pathlib.Path(os.environ['ARCHIVE_INPUT']).read_bytes()
if hashlib.sha256(data).hexdigest() != digest:
    sys.exit(41)
pathlib.Path(output).write_bytes(data)
try:
    cached.parent.mkdir(parents=True, exist_ok=True)
    cached.write_bytes(data)
except OSError:
    pass
PY
''')

    def command(self, name, text):
        target = self.commands / name
        target.write_text(text)
        target.chmod(0o755)

    def calls(self):
        path = self.root / 'calls'
        return path.read_text() if path.exists() else ''

    def use_real_helper(self):
        shutil.copyfile(ROOT / 'scripts/fetch-ci-archive.sh', self.scripts / 'fetch-ci-archive.sh')
        self.command('curl', '''#!/bin/bash
printf 'curl %s\n' "$*" >> "$CALLS"
input="$CA_INPUT"
while [ $# -gt 0 ]; do
  case "$1" in
    *releases/download/*|*/jm.zip) input="$ARCHIVE_INPUT" ;;
    -o|--output) output="$2"; shift ;;
    --cacert) cp "$2" "$CAPTURE"; shift ;;
  esac
  shift
done
cp "$input" "$output"
''')

    def run_installer(self, name, expected=0):
        result = subprocess.run(['bash', str(self.scripts / f'install-ci-{name}.sh'), str(self.dest)],
                                env=self.env, capture_output=True, text=True, timeout=20)
        if expected == 0:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stderr)
        return result

    def binaryen(self):
        archive = self.root / 'binaryen.tar.gz'
        with tarfile.open(archive, 'w:gz') as out:
            data = b'#!/bin/bash\necho synthetic-wasm-opt\n'
            entry = tarfile.TarInfo('binaryen-version_synthetic/bin/wasm-opt')
            entry.size = len(data)
            entry.mode = 0o755
            out.addfile(entry, io.BytesIO(data))
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.scripts / 'binaryen.env').write_text(f'BINARYEN_VERSION=synthetic\nBINARYEN_SHA256_X86_64_LINUX={digest}\nBINARYEN_SHA256_ARM64_MACOS={digest}\n')
        self.env['ARCHIVE_INPUT'] = str(archive)
        return archive, self.cache / f'binaryen-synthetic-{digest}.archive'

    def jm(self):
        archive = self.root / 'jm.zip'
        with zipfile.ZipFile(archive, 'w') as out:
            out.writestr('synthetic/ldecod/Makefile', '')
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.scripts / 'jm-decoder.env').write_text(f'JM_VERSION=synthetic\nJM_URL=https://synthetic.invalid/jm.zip\nJM_SHA256={digest}\nJM_CA_URL=https://synthetic.invalid/intermediate.pem\nJM_CA_SHA256={self.ca_digest}\n')
        self.env['ARCHIVE_INPUT'] = str(archive)
        return archive, self.cache / f'jm-synthetic-{digest}.archive'

    def test_binaryen_delegates_both_platform_pins_and_extracts_fresh_on_hits(self):
        archive, cache = self.binaryen()
        for platform, suffix in [('Linux x86_64', 'x86_64-linux'), ('Darwin arm64', 'arm64-macos')]:
            with self.subTest(platform=platform):
                self.env['FAKE_PLATFORM'] = platform
                result = self.run_installer('binaryen')
                opt = pathlib.Path(result.stdout.strip())
                self.assertTrue(opt.is_file())
                self.assertIn(f'binaryen-version_synthetic-{suffix}.tar.gz', self.calls())
                stale = opt.parent / 'stale'
                stale.write_text('not in the archive')
                archive.unlink()
                self.run_installer('binaryen')
                self.assertFalse(stale.exists())
                archive.write_bytes(cache.read_bytes())

    def test_binaryen_corrupt_cache_is_not_extracted_and_fetch_failure_propagates(self):
        archive, cache = self.binaryen()
        cache.write_bytes(b'corrupt cached archive')
        self.run_installer('binaryen')
        self.assertEqual(cache.read_bytes(), archive.read_bytes())
        cache.write_bytes(b'corrupt')
        archive.write_bytes(b'wrong download')
        self.run_installer('binaryen', expected=1)
        self.env['FETCH_EXIT'] = '37'
        self.assertEqual(self.run_installer('binaryen', expected=1).returncode, 37)

    def test_binaryen_unsupported_platform_refuses_before_fetch(self):
        self.binaryen()
        self.env['FAKE_PLATFORM'] = 'Linux aarch64'
        self.run_installer('binaryen', expected=1)
        self.assertEqual(self.calls(), '')

    def test_binaryen_real_helper_rechecks_hits_and_refuses_corrupt_downloads(self):
        archive, cache = self.binaryen()
        self.use_real_helper()
        self.run_installer('binaryen')
        archive.unlink()
        self.run_installer('binaryen')
        self.assertEqual(self.calls().count('curl '), 1)
        cache.write_bytes(b'corrupt cache')
        archive.write_bytes(b'wrong download')
        result = self.run_installer('binaryen', expected=1)
        self.assertIn('reason=ci_archive_digest_mismatch', result.stderr)
        self.assertEqual(self.calls().count('curl '), 2)

    def test_jm_caches_der_and_rebuilds_fresh_on_both_cache_hits(self):
        archive, cache = self.jm()
        self.run_installer('jm-decoder')
        ca_cache = self.cache / f'jm-ca-synthetic-{self.ca_digest}.archive'
        self.assertEqual(ca_cache.read_bytes(), self.der)
        self.assertEqual(cache.read_bytes(), archive.read_bytes())
        self.ca_input.unlink()
        archive.unlink()
        (self.dest / 'ldecod').write_text('stale binary')
        self.run_installer('jm-decoder')
        self.assertNotEqual((self.dest / 'ldecod').read_text(), 'stale binary')
        calls = self.calls()
        self.assertEqual(calls.count('curl '), 1)
        self.assertEqual(calls.count('make '), 2)
        self.assertIn('--cacert', calls)
        self.assertNotIn('--insecure', calls)
        self.assertIn("BUILD_COMMAND=make -C JM/ldecod -j2 CC='cc -fcommon'", (self.dest / 'jm-build.txt').read_text())

    def test_jm_ca_transport_uses_the_shared_bounded_retry_policy(self):
        self.jm()
        self.run_installer('jm-decoder')
        arguments = shlex.split(self.calls().splitlines()[0])
        expected = ['curl', '--fail', '--silent', '--show-error', '--location', '--retry', '8',
                    '--retry-all-errors', '--retry-connrefused', '--retry-max-time', '360',
                    '--connect-timeout', '20', '--max-time', '300', '--speed-limit', '1024', '--speed-time', '30']
        self.assertEqual(arguments[:len(expected)], expected)

    def test_jm_corrupt_der_cache_is_refetched_before_archive(self):
        self.jm()
        ca_cache = self.cache / f'jm-ca-synthetic-{self.ca_digest}.archive'
        ca_cache.write_bytes(b'corrupt certificate')
        self.run_installer('jm-decoder')
        self.assertEqual(ca_cache.read_bytes(), self.der)
        self.assertLess(self.calls().index('curl '), self.calls().index('fetch '))

    def test_jm_real_helper_rechecks_zip_hits_and_refuses_corrupt_downloads(self):
        archive, cache = self.jm()
        self.use_real_helper()
        self.run_installer('jm-decoder')
        archive.unlink()
        self.ca_input.unlink()
        self.run_installer('jm-decoder')
        self.assertEqual(self.calls().count('curl '), 2)
        self.assertEqual(self.calls().count('make '), 2)
        cache.write_bytes(b'corrupt ZIP cache')
        archive.write_bytes(b'wrong ZIP download')
        result = self.run_installer('jm-decoder', expected=1)
        self.assertIn('reason=ci_archive_digest_mismatch', result.stderr)
        self.assertEqual(self.calls().count('make '), 2)
        self.assertIn('--cacert', self.calls())

    def test_jm_wrong_certificate_pin_refuses_before_archive_or_publication(self):
        self.jm()
        self.ca_input.write_bytes((self.certs / 'extra.pem').read_bytes())
        self.run_installer('jm-decoder', expected=1)
        self.assertNotIn('fetch ', self.calls())
        self.assertEqual(self.calls().count('curl '), 1)
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_jm_downloaded_certificate_root_failure_is_not_retried(self):
        self.jm()
        self.env['SSL_CERT_FILE'] = str(self.certs / 'extra.pem')
        self.run_installer('jm-decoder', expected=1)
        self.assertEqual(self.calls().count('curl '), 1)
        self.assertNotIn('fetch ', self.calls())
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_jm_cached_certificate_still_requires_existing_root_chain(self):
        self.jm()
        ca_cache = self.cache / f'jm-ca-synthetic-{self.ca_digest}.archive'
        ca_cache.write_bytes(self.der)
        self.env['SSL_CERT_FILE'] = str(self.certs / 'extra.pem')
        self.run_installer('jm-decoder', expected=1)
        self.assertNotIn('fetch ', self.calls())
        self.assertNotIn('curl ', self.calls())

    def test_jm_bundle_never_trusts_trailing_unpinned_certificate(self):
        self.jm()
        for cached in (False, True):
            with self.subTest(cached=cached):
                self.run_installer('jm-decoder')
                bundle = self.root / 'bundle.pem'
                self.assertEqual(bundle.read_bytes().count(b'-----BEGIN CERTIFICATE-----'), 2)
                self.openssl('verify', '-CAfile', str(bundle), str(self.certs / 'intermediate.pem'))
                result = subprocess.run(['openssl', 'verify', '-CAfile', str(bundle), str(self.certs / 'extra.pem')],
                                        capture_output=True, timeout=10)
                self.assertNotEqual(result.returncode, 0, 'trailing certificate became trusted')
                self.ca_input.unlink(missing_ok=True)

    def test_jm_unavailable_advisory_cache_does_not_prevent_verified_install(self):
        self.jm()
        blocked = self.root / 'not-a-directory'
        blocked.write_text('blocked')
        self.env['COREAV_CI_ARCHIVE_DIR'] = str(blocked)
        self.run_installer('jm-decoder')
        self.assertTrue((self.dest / 'ldecod').is_file())

    def test_jm_ca_or_archive_failure_never_builds(self):
        self.jm()
        self.env['CA_CURL_EXIT'] = '28'
        self.assertEqual(self.run_installer('jm-decoder', expected=1).returncode, 28)
        self.assertNotIn('fetch ', self.calls())
        del self.env['CA_CURL_EXIT']
        self.env['FETCH_EXIT'] = '39'
        self.assertEqual(self.run_installer('jm-decoder', expected=1).returncode, 39)
        self.assertNotIn('make ', self.calls())

    def test_default_cli_cache_honors_xdg_without_changing_identity(self):
        self.binaryen()
        self.env.pop('COREAV_CI_ARCHIVE_DIR')
        self.env['XDG_CACHE_HOME'] = str(self.root / 'xdg')
        self.run_installer('binaryen')
        self.assertIn(str(self.root / 'xdg/coreav-ci-archives/binaryen-synthetic-'), self.calls())


class ReferenceWorkflow(unittest.TestCase):
    def test_workflow_inputs_and_unconditional_verified_installers(self):
        for workflow, job, installer, pin, tools in (
            ('wasm', 'wasm-module', 'binaryen', 'scripts/binaryen.env', ('binaryen',)),
            ('h264-encoder', 'h264-encoder-conformance', 'jm-decoder', 'scripts/jm-decoder.env', ('jm', 'jm-ca')),
        ):
            with self.subTest(workflow=workflow):
                text = (ROOT / f'.github/workflows/{workflow}.yml').read_text()
                for event in ('push', 'pull_request'):
                    trigger = re.split(r'\n  [a-z]', text.split(f'  {event}:\n', 1)[1], maxsplit=1)[0]
                    for path in (f'scripts/install-ci-{installer}.sh', 'scripts/fetch-ci-archive.sh',
                                 '.github/actions/ci-archive-cache/action.yml', 'scripts/tests/test_ci_reference_downloads.py', pin):
                        self.assertIn(f'      - {path}\n', trigger + '\n')
                body = re.split(r'\n  [a-z]', text.split(f'  {job}:\n', 1)[1], maxsplit=1)[0]
                self.assertIn(f'bash scripts/install-ci-{installer}.sh', body)
                self.assertNotIn(f'scripts/install-{installer}.sh', body)
                for tool in tools:
                    self.assertIn(f'tool: {tool}\n', body)
                self.assertEqual(body.count('uses: ./.github/actions/ci-archive-cache'), len(tools))
                install = re.search(rf'      - name: Install (?:the pinned optimizer|black-box decoder oracles)\n((?:        .*\n|          .*\n)+)', body)
                self.assertIsNotNone(install)
                self.assertNotIn('        if:', install[1])
                self.assertLess(body.index('uses: ./.github/actions/ci-archive-cache'), body.index(f'bash scripts/install-ci-{installer}.sh'))
                self.assertIn("-p 'test_ci_reference_downloads.py' -v", text)

    def test_npm_filters_and_installer_include_verified_archive_inputs(self):
        text = (ROOT / '.github/workflows/npm.yml').read_text()
        filters = text.split('          paths: |\n', 1)[1].split('\n\n', 1)[0]
        for path in ('scripts/binaryen.env', 'scripts/install-binaryen.sh', 'scripts/install-ci-binaryen.sh',
                     'scripts/fetch-ci-archive.sh', 'scripts/tests/test_ci_reference_downloads.py',
                     '.github/actions/ci-archive-cache/action.yml'):
            self.assertIn(f'            {path}\n', filters + '\n')
        package = text.split('\n  package:\n', 1)[1].split('\n  smoke:', 1)[0]
        self.assertIn('uses: ./.github/actions/ci-archive-cache\n', package)
        self.assertIn('tool: binaryen\n', package)
        self.assertIn('version: ${{ steps.binaryen.outputs.version }}\n', package)
        self.assertIn('sha256: ${{ steps.binaryen.outputs.sha256 }}\n', package)
        self.assertIn('opt="$(bash scripts/install-ci-binaryen.sh "$RUNNER_TEMP/binaryen")"', package)
        self.assertNotIn('scripts/install-binaryen.sh', package)
        install = package.split('      - name: Install binaryen\n', 1)[1].split('      - name:', 1)[0]
        self.assertNotIn('        if:', install)

    def test_workflow_pin_outputs_are_read_from_the_existing_env_owner(self):
        for workflow, step, env_file, expected in (
            ('wasm', 'binaryen', 'binaryen.env', {'version': 'synthetic', 'sha256': 'synthetic-linux-hash'}),
            ('npm', 'binaryen', 'binaryen.env', {'version': 'synthetic', 'sha256': 'synthetic-linux-hash'}),
            ('h264-encoder', 'jm', 'jm-decoder.env', {'version': 'synthetic', 'sha256': 'synthetic-zip-hash', 'ca-sha256': 'synthetic-ca-hash'}),
        ):
            text = (ROOT / f'.github/workflows/{workflow}.yml').read_text()
            match = re.search(rf'        id: {step}\n        run: \|\n((?:          .*\n)+)', text)
            self.assertIsNotNone(match)
            with tempfile.TemporaryDirectory() as temp:
                root = pathlib.Path(temp)
                (root / 'scripts').mkdir()
                (root / 'scripts' / env_file).write_text('BINARYEN_VERSION=synthetic\nBINARYEN_SHA256_X86_64_LINUX=synthetic-linux-hash\nJM_VERSION=synthetic\nJM_SHA256=synthetic-zip-hash\nJM_CA_SHA256=synthetic-ca-hash\n')
                output = root / 'output'
                subprocess.run(['bash', '-euo', 'pipefail', '-c', textwrap.dedent(match[1])], cwd=root,
                               env=dict(os.environ, GITHUB_OUTPUT=str(output)), check=True, capture_output=True, timeout=10)
                self.assertEqual(dict(line.split('=', 1) for line in output.read_text().splitlines()), expected)


if __name__ == '__main__':
    unittest.main()
