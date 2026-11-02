"""Exercise the standalone backend workflow without compiling Rust."""
import os
import pathlib
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


class EncoderBackends(unittest.TestCase):
    def run_backend(self, backend, state, failed_command=''):
        workflow = (ROOT / '.github/workflows/encoder-backends.yml').read_text()
        block = workflow.split('        run: |\n', 1)[1]
        script = '\n'.join(line[10:] for line in block.splitlines())
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            manifest = pathlib.Path('crates') / f'coreav-backend-{backend}' / 'Cargo.toml'
            if state != 'absent':
                (root / manifest.parent).mkdir(parents=True)
                name = 'moved.toml' if state == 'moved' else 'Cargo.toml'
                (root / manifest.parent / name).touch()
            bindir = root / 'bin'
            bindir.mkdir()
            cargo = bindir / 'cargo'
            cargo.write_text('#!/bin/bash\n'
                             'printf "%s\\n" "$*" >> "$LOG"\n'
                             'if [[ "$1" == "$FAILED_COMMAND" ]]; then exit 7; fi\n')
            cargo.chmod(0o755)
            log = root / 'cargo.log'
            done = subprocess.run(
                ['bash', '-c', script], cwd=root,
                env={**os.environ, 'PATH': str(bindir) + os.pathsep + os.environ['PATH'],
                     'MANIFEST': str(manifest), 'LOG': str(log),
                     'FAILED_COMMAND': failed_command},
                capture_output=True, text=True, timeout=10,
            )
            calls = log.read_text().splitlines() if log.exists() else []
            return done, calls, str(manifest)

    def test_absent_crate_can_skip(self):
        for backend in ('apple', 'svt-av1'):
            with self.subTest(backend=backend):
                done, calls, _ = self.run_backend(backend, 'absent')
                self.assertEqual(done.returncode, 0, done.stderr)
                self.assertEqual(calls, [])

    def test_moved_manifest_fails_instead_of_skipping(self):
        for backend in ('apple', 'svt-av1'):
            with self.subTest(backend=backend):
                done, calls, manifest = self.run_backend(backend, 'moved')
                self.assertNotEqual(done.returncode, 0)
                self.assertIn(manifest, done.stderr)
                self.assertEqual(calls, [])

    def test_present_manifest_runs_locked_tests_and_clippy(self):
        for backend in ('apple', 'svt-av1'):
            with self.subTest(backend=backend):
                done, calls, manifest = self.run_backend(backend, 'present')
                self.assertEqual(done.returncode, 0, done.stderr)
                self.assertEqual(calls, [
                    f'test --locked --manifest-path {manifest}',
                    f'clippy --locked --manifest-path {manifest} --all-targets -- -D warnings',
                ])

    def test_cargo_failures_fail_the_job(self):
        for backend in ('apple', 'svt-av1'):
            for command in ('test', 'clippy'):
                with self.subTest(backend=backend, command=command):
                    done, calls, _ = self.run_backend(backend, 'present', command)
                    self.assertEqual(done.returncode, 7)
                    self.assertEqual(len(calls), 1 if command == 'test' else 2)


if __name__ == '__main__':
    unittest.main()
