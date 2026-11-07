import gzip
import hashlib
import io
import itertools
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from test_main_ci_plan import evaluate


ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import composed_audio_fixtures as fixtures
import test_oracle

ACTION = './.github/actions/ci-fixtures'
STAND_IN = ROOT / '.github/actions/ci-fixtures/comparator-curl.sh'
REGISTRATION = ROOT / 'docs/design/composed-opus-aac-v2.json'
ARCHIVE_URL = 'https://example.invalid/opus.tar.gz'


def comparator_archive():
    """A tar.gz holding only the comparator source the workflows extract."""
    source = b'int main(void) { return 0; }\n'
    tar = io.BytesIO()
    with tarfile.open(fileobj=tar, mode='w') as archive:
        member = tarfile.TarInfo('opus-1.6.1/src/opus_compare.c')
        member.size = len(source)
        archive.addfile(member, io.BytesIO(source))
    compressed = io.BytesIO()
    with gzip.GzipFile(fileobj=compressed, mode='wb', mtime=0) as stream:
        stream.write(tar.getvalue())
    return compressed.getvalue()


def stand_in_curl(directory, body, exit_code=0):
    """Put a curl on PATH that writes body to the --output file, records every call and exits as told."""
    (directory / 'bin').mkdir(exist_ok=True)
    (directory / 'body').write_bytes(body)
    script = directory / 'bin/curl'
    script.write_text('#!/usr/bin/env bash\n'
                      f'printf "%s\\n" "$*" >> "{directory}/calls"\n'
                      'while [ "$#" -gt 1 ]; do case "$1" in -o|--output) target="$2"; shift ;; esac; shift; done\n'
                      f'cp "{directory}/body" "$target"\n'
                      f'exit {exit_code}\n')
    script.chmod(0o755)
    return directory / 'calls'


def run_block(script, cwd, env):
    return subprocess.run(['bash', '-c', script], cwd=cwd, env=os.environ | env,
                          capture_output=True, text=True)


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
        for name in ('isobmff-writers', 'isobmff-decode', 'transcode-compat-shard'):
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
        publish, = [s for s in steps if s.get('uses', '').startswith('actions/upload-artifact@')
                    and s.get('with', {}).get('name') == 'isobmff-fixture-set']
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
        composed, = [s for s in steps if s.get('id') == 'composed']
        composed_save, = [s for s in steps if s.get('name') == 'Save the registered composed-audio fixtures']
        self.assertEqual(composed['with']['key'], '${{ steps.key.outputs.composed }}')
        self.assertEqual(composed_save['with']['key'], composed['with']['key'])

    def test_key_changes_with_the_pin_generator_and_discovered_helpers(self):
        key, = [s for s in self.action['runs']['steps'] if s.get('id') == 'key']
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            scripts = root / 'scripts'
            scripts.mkdir()
            contents = {'gen-fixtures.sh': '$SCRIPTS/craft.py\n', 'craft.py': 'first\n',
                        'install-oracle-ffmpeg.sh': 'installer\n', 'oracle-ffmpeg.env': 'pin\n'}
            contents.update({name: (ROOT / 'scripts' / name).read_text()
                             for name in test_oracle.SUPPORT})
            for name, content in contents.items():
                (scripts / name).write_text(content)
            composed_inputs = {
                'docs/design/composed-opus-aac-v2.json': '{"opus_compare": {"sha256": "0f"}}\n',
                'scripts/composed_audio.py': 'composed source\n',
                'scripts/composed_audio_fixtures.py': 'fixture source\n',
            }
            for name, content in composed_inputs.items():
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content)

            def compute():
                output = root / 'output'
                output.write_text('')
                subprocess.run(['bash', '-e', '-c', key['run']], cwd=root, check=True,
                               env=os.environ | {'GITHUB_OUTPUT': str(output)})
                return output.read_text()

            original = compute()
            self.assertEqual(original, compute())
            original_keys = dict(line.split('=', 1) for line in original.splitlines())
            self.assertIn('composed', original_keys)
            self.assertEqual(original_keys['comparator'], 'comparator-archive-0f')
            for name, content in contents.items():
                (scripts / name).write_text(content + '\n')
                changed = compute()
                self.assertNotEqual(original, changed, name)
                if name in test_oracle.SUPPORT or name == 'install-oracle-ffmpeg.sh':
                    self.assertNotEqual(original_keys['composed'],
                                        dict(line.split('=', 1) for line in changed.splitlines())['composed'], name)
                (scripts / name).write_text(content)
            for name, content in composed_inputs.items():
                path = root / name
                path.write_text(content + '\n')
                changed = dict(line.split('=', 1) for line in compute().splitlines())
                self.assertEqual(original_keys['key'], changed['key'], name)
                self.assertNotEqual(original_keys['composed'], changed['composed'], name)
                self.assertEqual(original_keys['comparator'], changed['comparator'], name)
                path.write_text(content)
            registration = root / 'docs/design/composed-opus-aac-v2.json'
            registration.write_text('{"opus_compare": {"sha256": "1e"}}\n')
            self.assertEqual(dict(line.split('=', 1) for line in compute().splitlines())['comparator'],
                             'comparator-archive-1e')
            (scripts / 'extra.py').write_text('extra\n')
            (scripts / 'gen-fixtures.sh').write_text(contents['gen-fixtures.sh'] + '$SCRIPTS/extra.py\n')
            added = compute()
            (scripts / 'extra.py').write_text('changed\n')
            self.assertNotEqual(added, compute())

    def test_comparator_archive_is_cached_by_its_registered_digest_on_every_branch(self):
        steps = self.action['runs']['steps']
        composed, = [s for s in steps if s.get('id') == 'composed']
        cache, = [s for s in steps if s.get('name') == 'Cache the pinned comparator archive']
        fetch, = [s for s in steps if s.get('name') == 'Fetch the pinned comparator archive on a cache miss']
        generate, = [s for s in steps if s.get('name') == 'Generate the registered composed-audio fixtures']
        self.assertTrue(cache['uses'].startswith('actions/cache@'), 'restore and save in one step')
        self.assertEqual(cache['with']['key'], '${{ steps.key.outputs.comparator }}')
        self.assertEqual(cache['with']['path'], '${{ runner.temp }}/coreav-comparator-archive')
        self.assertNotIn('restore-keys', cache['with'])
        for ref, hit in itertools.product(('refs/heads/main', 'refs/pull/1/merge',
                                           'refs/heads/gh-readonly-queue/main/group'), ('true', 'false')):
            context = {'github': {'ref': ref}, 'steps': {'composed': {'outputs': {'cache-hit': hit}}}}
            self.assertEqual(bool(evaluate(cache['if'], context)), hit != 'true', ref)
            self.assertEqual(bool(evaluate(fetch['if'], context)), hit != 'true', ref)
        self.assertLess(steps.index(composed), steps.index(cache))
        self.assertLess(steps.index(cache), steps.index(fetch))
        self.assertLess(steps.index(fetch), steps.index(generate))
        archive = '"$RUNNER_TEMP/coreav-comparator-archive/opus.tar.gz"'
        self.assertIn(f'archive={archive}', fetch['run'])
        self.assertIn(f'COREAV_COMPARATOR_ARCHIVE={archive}', generate['run'])
        self.assertIn('env PATH="$RUNNER_TEMP/coreav-comparator-curl:$PATH" python3 -I scripts/composed_audio_fixtures.py',
                      generate['run'])
        for name in ('COREAV_COMPARATOR_URL', 'COREAV_COMPARATOR_ARCHIVE'):
            self.assertIn(f'"${name}"', STAND_IN.read_text(), name)
        pin = json.loads(REGISTRATION.read_text())['opus_compare']
        key, = [s for s in steps if s.get('id') == 'key']
        with tempfile.NamedTemporaryFile('r') as output:
            subprocess.run(['bash', '-e', '-c', key['run']], cwd=ROOT, check=True,
                           env=os.environ | {'GITHUB_OUTPUT': output.name})
            keys = dict(line.split('=', 1) for line in output.read().splitlines())
        self.assertEqual(keys['comparator'], 'comparator-archive-' + pin['sha256'])
        opus = read_yaml(ROOT / '.github/workflows/audio-conformance.yml')['jobs']['opus']
        self.assertEqual(opus['env']['LIBOPUS_URL'], pin['url'])
        self.assertEqual(opus['env']['LIBOPUS_SHA256'], pin['sha256'])
        shared, = [s for s in opus['steps'] if s.get('name') == 'Cache the pinned comparator archive']
        self.assertEqual(shared['uses'], cache['uses'])
        self.assertEqual(shared['with']['path'], cache['with']['path'])
        self.assertNotIn('if', shared)
        self.assertEqual(shared['with']['key'].replace('${{ env.LIBOPUS_SHA256 }}', pin['sha256']), keys['comparator'])
        build, = [s for s in opus['steps'] if s.get('name') == 'Build opus_compare']
        self.assertIn(f'archive={archive}', build['run'])
        self.assertLess(opus['steps'].index(shared), opus['steps'].index(build))

    def fetch_step(self, directory, cached, body, exit_code=0):
        """Run the fetch step in a checkout stand-in and return (calls, archive, stand-in curl) paths."""
        step, = [s for s in self.action['runs']['steps']
                 if s.get('name') == 'Fetch the pinned comparator archive on a cache miss']
        root = pathlib.Path(directory)
        shutil.rmtree(root, ignore_errors=True)
        (root / 'docs/design').mkdir(parents=True)
        (root / '.github/actions/ci-fixtures').mkdir(parents=True)
        shutil.copy(STAND_IN, root / '.github/actions/ci-fixtures/comparator-curl.sh')
        archive = root / 'temp/coreav-comparator-archive/opus.tar.gz'
        (root / 'temp').mkdir()
        if cached is not None:
            archive.parent.mkdir()
            archive.write_bytes(cached)
        (root / 'docs/design/composed-opus-aac-v2.json').write_text(json.dumps(
            {'opus_compare': {'url': ARCHIVE_URL, 'sha256': hashlib.sha256(comparator_archive()).hexdigest()}}))
        calls = stand_in_curl(root, body, exit_code)
        result = run_block(step['run'], root, {'RUNNER_TEMP': str(root / 'temp'),
                                               'PATH': f"{root / 'bin'}:{os.environ['PATH']}"})
        self.assertEqual(result.returncode, 0, result.stderr)
        return result, calls, archive, root / 'temp/coreav-comparator-curl/curl'

    def test_fetch_keeps_only_a_digest_verified_archive_and_installs_the_stand_in_beside_it(self):
        registered = comparator_archive()
        with tempfile.TemporaryDirectory() as directory:
            result, calls, archive, stand_in = self.fetch_step(directory, None, registered)
            self.assertEqual(archive.read_bytes(), registered)
            self.assertEqual(len(calls.read_text().splitlines()), 1)
            self.assertIn('--retry 3 --retry-all-errors', calls.read_text())
            self.assertIn(ARCHIVE_URL, calls.read_text())
            self.assertEqual(stand_in.read_text(), STAND_IN.read_text())
            self.assertTrue(os.access(stand_in, os.X_OK))
            self.assertNotIn('::warning::', result.stdout)

            result, calls, archive, stand_in = self.fetch_step(directory, registered, b'never fetched')
            self.assertEqual(archive.read_bytes(), registered)
            self.assertFalse(calls.exists())
            self.assertTrue(stand_in.exists())

            result, calls, archive, stand_in = self.fetch_step(directory, b'another archive', registered)
            self.assertEqual(archive.read_bytes(), registered)
            self.assertEqual(len(calls.read_text().splitlines()), 1)
            self.assertIn('::warning::the cached comparator archive has another digest', result.stdout)

            for body, exit_code in ((b'another archive', 0), (registered, 7)):
                result, calls, archive, stand_in = self.fetch_step(directory, None, body, exit_code)
                self.assertFalse(archive.parent.exists(), 'nothing unverified enters the cached directory')
                self.assertFalse(stand_in.parent.exists(), 'the generator downloads on its own')
                self.assertFalse(list(archive.parents[1].glob('*.download')))
                self.assertIn('::warning::comparator archive fetch failed', result.stdout)

    def test_stand_in_serves_only_the_generators_request_for_the_registered_archive(self):
        registered = comparator_archive()
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            cached = root / 'opus.tar.gz'
            cached.write_bytes(registered)
            env = {'COREAV_COMPARATOR_URL': ARCHIVE_URL, 'COREAV_COMPARATOR_ARCHIVE': str(cached),
                   'PATH': os.environ['PATH']}
            pin = {'url': ARCHIVE_URL, 'sha256': hashlib.sha256(registered).hexdigest(),
                   'source': 'opus-1.6.1/src/opus_compare.c'}
            served = []

            class Extracting(Exception):
                pass

            def bounded(cmd, environment, **kwargs):
                if cmd[0] != 'curl':
                    raise Extracting(cmd)
                served.append(subprocess.run([str(STAND_IN), *map(str, cmd[1:])], env=env,
                                             capture_output=True, text=True))

            with patch.object(fixtures.composed, 'registration', return_value=({'opus_compare': pin}, {})), \
                    patch.object(fixtures.composed, 'bounded', bounded):
                with self.assertRaises(Extracting) as stopped:
                    fixtures.build_comparator(root / 'tools', env)
                self.assertEqual(stopped.exception.args[0][0], 'tar', 'the digest check passed on the served bytes')
                self.assertEqual([run.returncode for run in served], [0])
                cached.write_bytes(b'another archive')
                with self.assertRaisesRegex(fixtures.composed.Invalid, 'digest differs'):
                    fixtures.build_comparator(root / 'tools-again', env)
            request = ['--fail', '--silent', '--location', '-o', str(root / 'out.tar.gz'), ARCHIVE_URL]
            for refused in (request[:-1] + ['https://example.invalid/other.tar.gz'],
                            request[:3] + [ARCHIVE_URL], request[:-1] + ['--output', ARCHIVE_URL]):
                result = subprocess.run([str(STAND_IN), *refused], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 2, refused)
                self.assertIn('only the registered comparator archive is served', result.stderr)
                self.assertFalse((root / 'out.tar.gz').exists(), refused)

    def test_audio_conformance_builds_the_comparator_from_the_cached_archive(self):
        build, = [s for s in read_yaml(ROOT / '.github/workflows/audio-conformance.yml')['jobs']['opus']['steps']
                  if s.get('name') == 'Build opus_compare']
        registered = comparator_archive()
        digest = hashlib.sha256(registered).hexdigest()
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            archive = root / 'temp/coreav-comparator-archive/opus.tar.gz'
            for cached, body, exit_code, fetched in ((registered, b'never fetched', 1, 0),
                                                     (None, registered, 0, 1),
                                                     (b'another archive', registered, 0, 1)):
                shutil.rmtree(root / 'temp', ignore_errors=True)
                (root / 'temp').mkdir()
                if cached is not None:
                    archive.parent.mkdir()
                    archive.write_bytes(cached)
                calls = stand_in_curl(root, body, exit_code)
                calls.unlink(missing_ok=True)
                result = run_block(build['run'], root, {
                    'RUNNER_TEMP': str(root / 'temp'), 'LIBOPUS_URL': ARCHIVE_URL, 'LIBOPUS_SHA256': digest,
                    'PATH': f"{root / 'bin'}:{os.environ['PATH']}"})
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(archive.read_bytes(), registered)
                self.assertEqual(len(calls.read_text().splitlines()) if calls.exists() else 0, fetched)
                self.assertEqual(subprocess.run([root / 'temp/opus-compare/opus_compare']).returncode, 0)
                self.assertFalse((root / 'temp/opus-compare/opus-1.6.1').exists())
            calls = stand_in_curl(root, b'another archive')
            calls.unlink(missing_ok=True)
            shutil.rmtree(root / 'temp')
            (root / 'temp').mkdir()
            result = run_block(build['run'], root, {
                'RUNNER_TEMP': str(root / 'temp'), 'LIBOPUS_URL': ARCHIVE_URL, 'LIBOPUS_SHA256': digest,
                'PATH': f"{root / 'bin'}:{os.environ['PATH']}"})
            self.assertNotEqual(result.returncode, 0, 'a fetched archive with another digest fails the job')
            self.assertFalse(archive.parent.exists(), 'and never enters the cached directory')

    def test_all_fixture_support_changes_start_both_wasm_events(self):
        workflow = read_yaml(ROOT / '.github/workflows/wasm.yml')
        events = workflow.get('on', workflow.get('true'))
        for event in ('push', 'pull_request'):
            for name in test_oracle.SUPPORT:
                self.assertIn('scripts/' + name, events[event]['paths'], (event, name))


if __name__ == '__main__':
    unittest.main()
