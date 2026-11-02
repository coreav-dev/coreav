"""Notice-carrying CLI distributions need the pinned compiler's sources."""
import copy
import json
import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
BUILD = 'packages/cli/scripts/platform.sh'
FIX = 'Install rust-src for the toolchain in scripts/rust-dist.env before building a CLI distribution'


class DistributionWorkflows(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        workflows = json.loads(subprocess.check_output([
            'ruby', '-ryaml', '-rjson', '-e',
            'puts JSON.generate(ARGV.to_h { |p| [File.basename(p), YAML.load_file(p, aliases: true)] })',
            *map(str, sorted((ROOT / '.github/workflows').glob('*.y*ml')))], text=True))
        cls.jobs = {(name, key): job for name, workflow in workflows.items()
                    for key, job in workflow['jobs'].items()
                    if any(BUILD in step.get('run', '') for step in job.get('steps', []))}

    def assert_sources_installed(self, job, pin=True):
        steps = job['steps']
        build = next(i for i, step in enumerate(steps) if BUILD in step.get('run', ''))
        readers = [(i, step) for i, step in enumerate(steps[:build])
                   if 'scripts/rust-dist.env' in step.get('run', '')]
        self.assertEqual(len(readers), 1, FIX)
        position, reader = readers[0]
        self.assertNotIn('if', reader, FIX)
        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp)
            (directory / 'scripts').mkdir()
            if pin:
                (directory / 'scripts/rust-dist.env').write_text('RUST_DIST_TOOLCHAIN=9.8.7\n')
            rustup = directory / 'rustup'
            rustup.write_text('#!/bin/sh\nprintf "%s\\n" "$@" > "$INSTALL_ARGS"\n')
            rustup.chmod(0o755)
            output, environment, args = (directory / name for name in ('output', 'environment', 'args'))
            done = subprocess.run(['bash', '-e', '-c', reader['run']], cwd=directory,
                                  env={**os.environ, 'PATH': tmp + ':' + os.environ['PATH'],
                                       'GITHUB_OUTPUT': str(output), 'GITHUB_ENV': str(environment),
                                       'INSTALL_ARGS': str(args), 'RUST_DIST_TOOLCHAIN': 'wrong'},
                                  capture_output=True, text=True, timeout=10)
            self.assertEqual(done.returncode, 0, FIX + '\n' + done.stderr)
            version = '9.8.7' if pin else 'stable'
            if args.exists():
                installed = args.read_text().splitlines()
                self.assertEqual(installed[:3], ['toolchain', 'install', version], FIX)
                self.assertIn('--component', installed, FIX)
                self.assertEqual(installed[installed.index('--component') + 1], 'rust-src', FIX)
            else:
                values = dict(line.split('=', 1) for line in output.read_text().splitlines())
                self.assertEqual(values.get('version'), version, FIX)
                self.assertIn(f'RUSTUP_TOOLCHAIN={version}\n', environment.read_text(), FIX)
                installers = [step for step in steps[position + 1:build]
                              if step.get('uses', '').startswith('dtolnay/rust-toolchain@')]
                self.assertEqual(len(installers), 1, FIX)
                installer = installers[0]
                self.assertNotIn('if', installer, FIX)
                self.assertEqual(installer['with']['toolchain'],
                                 '${{ steps.' + reader['id'] + '.outputs.version }}', FIX)
                self.assertIn('rust-src', installer['with'].get('components', '').replace(',', ' ').split(), FIX)

    def test_every_distribution_build_installs_pinned_rust_src(self):
        self.assertIn(('codec-coverage.yml', 'coverage-build'), self.jobs)
        for identity, job in self.jobs.items():
            with self.subTest(job=identity):
                self.assert_sources_installed(job)

    def test_registered_candidates_before_notices_keep_the_stable_toolchain(self):
        self.assert_sources_installed(self.jobs['codec-coverage.yml', 'coverage-build'], pin=False)

    def test_missing_sources_or_unpinned_compiler_names_the_fix(self):
        for field, value in (('components', ''), ('toolchain', 'stable')):
            job = copy.deepcopy(self.jobs['codec-coverage.yml', 'coverage-build'])
            installer = next(step for step in job['steps']
                             if step.get('uses', '').startswith('dtolnay/rust-toolchain@'))
            installer['with'][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, FIX):
                self.assert_sources_installed(job)


if __name__ == '__main__':
    unittest.main()
