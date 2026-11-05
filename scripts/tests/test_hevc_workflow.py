import json
import os
import pathlib
import shlex
import subprocess
import sys
import tempfile
import textwrap
import unittest


class HevcWorkflow(unittest.TestCase):
    def test_census_uses_its_own_public_recipient_without_changing_shared_corpus_sealing(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        workflow = (root / '.github/workflows/hevc-conformance.yml').read_text()
        self.assertIn('-inkey "$GITHUB_WORKSPACE/bench/hevc-conformance/evidence-public.pem"', workflow)
        self.assertNotIn('-inkey "$GITHUB_WORKSPACE/tools/compat-corpus/evidence-public.pem"', workflow)
        public = root / 'bench/hevc-conformance/evidence-public.pem'
        self.assertTrue(public.read_text().startswith('-----BEGIN PUBLIC KEY-----\n'))
        self.assertNotIn('PRIVATE KEY', public.read_text())
        subprocess.run(['openssl', 'pkey', '-pubin', '-in', str(public), '-noout'],
                       check=True, capture_output=True, timeout=10)

    def test_intra_workflow_tracks_kernels_and_the_g2_driver(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        workflow = (root / '.github/workflows/hevc-conformance.yml').read_text()
        filters = [json.loads(line.split(':', 1)[1]) for line in workflow.splitlines()
                   if line.strip().startswith('paths:')]
        self.assertEqual(len(filters), 2)
        for paths in filters:
            for required in ('crates/coreav-hevc/**', 'crates/coreav-kernels-hevc/**',
                             'scripts/hevc_intra.py', 'scripts/tests/test_hevc*.py'):
                self.assertIn(required, paths)
        step = workflow.split('- name: Check complete headers and every selected intra reference\n', 1)[1]
        command = step.split('run: |\n', 1)[1].split('\n      - ', 1)[0]
        arguments = shlex.split(command.replace('\\\n', ''))
        self.assertEqual(arguments[:3], ['python3', '-I', 'scripts/hevc_intra.py'])
        self.assertEqual(int(arguments[arguments.index('--max-memory-bytes') + 1]), 256 << 20)
        self.assertEqual(int(arguments[arguments.index('--max-bins') + 1]), 1 << 28)
        self.assertIn('--filters', arguments, 'Run original G3 comparisons in the shared driver')
        self.assertNotIn('scripts/hevc_conformance.py run', workflow)
        build = next(line for line in workflow.splitlines() if 'cargo build --locked --release' in line)
        self.assertIn('--example headers --example intra', build)

    def test_development_intra_checks_cannot_be_dropped_from_the_metadata_job(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        workflow = (root / '.github/workflows/hevc-conformance.yml').read_text()
        job = workflow.split('  profile-census:\n',1)[1]
        command = job.split('python3 -I scripts/hevc_conformance.py corpus-online',1)[1].split('\n      - name:',1)[0]
        arguments = shlex.split(command.replace('\\\n',''))
        self.assertIn('--native', arguments, 'Restore the development first-IRAP decoder')
        self.assertIn('--parser', arguments, 'Restore original first-IRAP header validation')
        self.assertIn('--example headers --example intra', job)
        self.assertIn("summary['development_intra']", job)

    def test_feature_isolation_step_rejects_every_native_dependency_leak(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        workflow = (root / '.github/workflows/hevc-conformance.yml').read_text()
        step = workflow.split('- name: Keep HEVC out of feature-disabled and default consumers\n', 1)[1]
        script = textwrap.dedent(step.split('run: |\n', 1)[1].split('\n      - ', 1)[0])
        with tempfile.TemporaryDirectory() as directory:
            temp = pathlib.Path(directory)
            cargo = temp / 'cargo'
            cargo.write_text('''#!/bin/sh
case " $* " in
  *" -p coreav-hevc "*)
    printf '%s\\n' 'coreav-hevc v0.0.1'
    if [ "$LEAK" = off ]; then printf '%s\\n' 'sha2 v0.10'; fi ;;
  *" -p coreav-cli "*)
    printf '%s\\n' 'coreav-cli v0.0.1'
    case "$LEAK" in
      decoder) printf '%s\\n' 'coreav-hevc v0.0.1' ;;
      kernel) printf '%s\\n' 'coreav-kernels-hevc v0.0.1' ;;
    esac ;;
  *) exit 2 ;;
esac
''')
            cargo.chmod(0o700)
            for leak in ('none', 'off', 'decoder', 'kernel'):
                environment = dict(PATH=str(temp) + os.pathsep + os.defpath,
                                   RUNNER_TEMP=str(temp), LEAK=leak)
                result = subprocess.run(['bash', '-c', script], env=environment,
                                        capture_output=True, text=True, timeout=10)
                with self.subTest(leak=leak):
                    self.assertEqual(result.returncode == 0, leak == 'none', result.stderr)

    def test_documented_census_prepares_and_consumes_synthetic_references(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        readme = (root / 'bench/hevc-conformance/README.md').read_text()
        script = readme.split('```sh\n', 1)[1].split('```', 1)[0]
        commands = [shlex.split(line) for line in script.replace('\\\n', '').splitlines()
                    if line.strip()]
        verify = next((args for args in commands if 'scripts/hevc_fixture.py' in args), None)
        self.assertIsNotNone(verify, 'Verify synthetic references before running the full census')
        self.assertIn('verify', verify)
        run = next(args for args in commands if 'scripts/hevc_conformance.py' in args
                   and 'run' in args)
        self.assertLess(commands.index(verify), commands.index(run))
        self.assertIn('--synthetic-data', run, 'Pass verified synthetic references to the census')
        self.assertEqual(verify[verify.index('--output') + 1],
                         run[run.index('--synthetic-data') + 1])

    def test_agent_scopes_name_hevc_rules(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        index = (root / 'AGENTS.md').read_text().splitlines()
        for area, subjects in (('compat-corpus', ('HEVC conformance census', 'oracle preparation')),
                               ('codecs', ('HEVC decoding',))):
            path = f'docs/agents/{area}.md'
            intro = (root / path).read_text().split('\n\n')[1]
            scope = next(line for line in index if line.startswith(f'| [{path}]'))
            for location, text in (('area introduction', intro), ('AGENTS.md scope', scope)):
                for subject in subjects:
                    with self.subTest(area=area, location=location, subject=subject):
                        self.assertIn(subject, text, f'Name {subject} in the {area} {location}')

    def test_public_profile_projection_omits_per_file_records(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        workflow = (root / '.github/workflows/hevc-conformance.yml').read_text()
        step = workflow.split('- name: Retain only aggregate profile counts publicly\n', 1)[1]
        script = textwrap.dedent(step.split("<<'PY'\n", 1)[1].split('\n          PY', 1)[0])
        with tempfile.TemporaryDirectory() as directory:
            temp = pathlib.Path(directory)
            (temp / 'private').mkdir()
            record = dict(roster_sha256='a' * 64, complete=True,
                          counts=[dict(partition='development', profile='Main', bit_depth=8,
                                       status='inspected', files=1)],
                          files=[dict(source='private-canary', url='private-canary')])
            source = temp / 'private/corpus-census.json'
            source.write_text(json.dumps(record))
            subprocess.run([sys.executable, '-I', '-', str(temp)], input=script, text=True,
                           check=True, capture_output=True, timeout=10)
            text = (temp / 'profile-summary.json').read_text()
            summary = json.loads(text)
            self.assertNotIn('private-canary', text)
            self.assertEqual(summary['files'], 1)
            self.assertEqual(set(summary), {'roster_sha256', 'complete', 'counts', 'files'})
            self.assertEqual(json.loads(source.read_text()), record)


if __name__ == '__main__':
    unittest.main()
