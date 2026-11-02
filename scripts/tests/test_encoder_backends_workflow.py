"""Exercise the standalone backend workflow without compiling Rust."""
import os
import importlib.util
import pathlib
import re
import subprocess
import tempfile
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('backend_workflow_check', ROOT / 'scripts/workflow_check.py')
workflow_check = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(workflow_check)


def rust_build_patterns():
    workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace'].get('dependencies', {})
    seen = set()

    def visit(manifest, include_dev=False):
        manifest = manifest.resolve()
        if manifest in seen:
            return
        seen.add(manifest)
        value = tomllib.loads(manifest.read_text())
        sections = [value, *value.get('target', {}).values()]
        for section in sections:
            names = ('dependencies', 'build-dependencies', 'dev-dependencies') if include_dev else ('dependencies', 'build-dependencies')
            for name in names:
                for package, dependency in section.get(name, {}).items():
                    if not isinstance(dependency, dict):
                        continue
                    base = manifest.parent
                    if dependency.get('workspace'):
                        dependency = workspace[package]
                        base = ROOT
                    if isinstance(dependency, dict) and 'path' in dependency:
                        visit(base / dependency['path'] / 'Cargo.toml')

    visit(ROOT / 'crates/coreav-backend-apple/Cargo.toml', include_dev=True)
    return {'Cargo.toml'} | {path.parent.relative_to(ROOT).as_posix() + '/**' for path in seen}


def plane_initialization_problems(source):
    source = re.sub(r'//[^\n]*', '', source)
    problems = []
    if source.count('std::slice::from_raw_parts_mut(') != 2:
        problems.append('keep native plane slices in the two checked copy_frame paths')
    for plane in ('y', 'uv'):
        pattern = (rf'{plane}\.write_bytes\(0, {plane}_len\);\s*'
                   rf'let {plane}_plane = std::slice::from_raw_parts_mut\({plane}, {plane}_len\);')
        if not re.search(pattern, source):
            problems.append(f'initialize the full {plane} plane before forming its byte slice')
    return problems


class EncoderBackends(unittest.TestCase):
    def test_videotoolbox_smoke_tracks_its_rust_build_closure(self):
        workflow = workflow_check.workflow_yaml((ROOT / '.github/workflows/videotoolbox.yml').read_text())
        self.assertIsInstance(workflow, dict)
        required = rust_build_patterns() | {'deny.toml'}
        self.assertIn('crates/coreav-h264/**', required)
        self.assertIn('crates/coreav-kernels-h264/**', required)
        self.assertIn('crates/coreav-aac/**', required)
        for event in ('push', 'pull_request'):
            paths = workflow['on'][event]['paths']
            for pattern in sorted(required):
                self.assertIn(pattern, paths, f'add {pattern} to videotoolbox.yml {event} paths')

    def test_native_plane_storage_is_initialized_before_byte_slices(self):
        source = (ROOT / 'crates/coreav-backend-apple/src/videotoolbox/sys.rs').read_text()
        self.assertEqual(plane_initialization_problems(source), [])
        for plane in ('y', 'uv'):
            initialization = f'{plane}.write_bytes(0, {plane}_len);'
            old = source.replace(initialization, '', 1)
            self.assertIn(f'initialize the full {plane} plane before forming its byte slice',
                          plane_initialization_problems(old))

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
