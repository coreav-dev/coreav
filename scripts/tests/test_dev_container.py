"""Check the local Linux wrapper without a Docker daemon."""
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('dev_container', ROOT / 'scripts/dev-container.py')
DEV = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DEV)


class DevContainer(unittest.TestCase):
    @unittest.skipUnless(sys.platform == 'linux' and shutil.which('sha256sum'),
                         'requires the Linux image checksum utility')
    def test_corrupt_bootstrap_is_not_executed(self):
        dockerfile = (ROOT / 'scripts/dev/Dockerfile').read_text()
        bootstrap = 'case "$TARGETARCH" in' + dockerfile.split('RUN case "$TARGETARCH" in', 1)[1].split('\n\n', 1)[0]
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory)
            installer = scratch / 'rustup-init'
            marker = scratch / 'executed'
            curl = scratch / 'curl'
            curl.write_text('#!/bin/sh\nfor arg; do output=$arg; done\nprintf \'#!/bin/sh\\ntouch "%s"\\n\' "$MARKER" > "$output"\n')
            curl.chmod(0o755)
            for architecture in ('amd64', 'arm64'):
                with self.subTest(architecture=architecture):
                    result = subprocess.run(['sh', '-c', bootstrap.replace('/tmp/rustup-init', str(installer))],
                                            env={**os.environ, 'PATH': str(scratch) + os.pathsep + os.environ['PATH'],
                                                 'TARGETARCH': architecture, 'MARKER': str(marker)},
                                            capture_output=True, text=True, timeout=10)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn('FAILED', result.stdout, result.stderr)
                    self.assertFalse(marker.exists())

    def run_wrapper(self, arguments, status=0):
        with patch.object(sys, 'argv', ['dev-container.py', *arguments]), \
                patch.object(DEV.shutil, 'which', return_value='/usr/local/bin/docker'), \
                patch.object(DEV.Path, 'cwd', return_value=ROOT), \
                patch.object(DEV.Path, 'is_file', lambda path: path.name == '.git' or (path.exists() and not path.is_dir())), \
                patch.object(sys.stdin, 'isatty', return_value=False), \
                patch.object(DEV.subprocess, 'run', return_value=subprocess.CompletedProcess([], status)) as run:
            result = DEV.main()
        return result, run.call_args_list

    def test_default_boundary_and_arguments(self):
        result, calls = self.run_wrapper(['--', 'printf', '%s', 'a b', '$(false)'], status=19)
        self.assertEqual(result, 19)
        self.assertEqual(len(calls), 1)
        command = calls[0].args[0]
        self.assertEqual(command[-4:], ['printf', '%s', 'a b', '$(false)'])
        self.assertIn('-i', command)
        self.assertNotIn('-t', command)
        for flag, value in (('--network', 'none'), ('--user', '1000:1000'), ('--memory', '6g'),
                            ('--memory-swap', '6g'), ('--pids-limit', '512'), ('--cpus', '2'),
                            ('--pull', 'never'), ('--cap-drop', 'ALL'), ('--security-opt', 'no-new-privileges')):
            self.assertEqual(command[command.index(flag) + 1], value)
        mounts = [command[i + 1] for i, item in enumerate(command) if item == '--mount']
        self.assertEqual(len(mounts), 4)
        self.assertEqual([m for m in mounts if m.startswith('type=bind,')],
                         [f'type=bind,source={ROOT},target=/work',
                          f'type=bind,source={ROOT}/.git,target=/work/.git,readonly'])
        self.assertTrue(any(m.startswith('type=volume,source=coreav-dev-target-amd64-') for m in mounts))
        self.assertIn('type=volume,source=coreav-dev-cargo-amd64,target=/home/dev/.cargo', mounts)
        self.assertNotIn('--env', command)
        self.assertNotIn('-e', command)
        self.assertNotIn('--env-file', command)

    def test_network_is_explicit_and_architectures_do_not_share_targets(self):
        _, amd64 = self.run_wrapper(['--', 'true'])
        _, arm64 = self.run_wrapper(['--platform', 'linux/arm64', '--network', '--', 'true'])
        command = arm64[0].args[0]
        self.assertEqual(command[command.index('--network') + 1], 'bridge')
        self.assertEqual(command[command.index('--platform') + 1], 'linux/arm64')
        self.assertNotEqual([x for x in amd64[0].args[0] if 'source=coreav-dev-target-' in x],
                            [x for x in command if 'source=coreav-dev-target-' in x])

    def test_build_context_has_only_declared_inputs(self):
        observed = []

        def invoke(command, **kwargs):
            if command[1] == 'build':
                self.assertEqual(command[command.index('--build-arg') + 1], 'TARGETARCH=amd64')
                context = Path(command[-1])
                observed.extend(sorted(str(p.relative_to(context)) for p in context.rglob('*') if p.is_file()))
                self.assertEqual(kwargs['env']['BUILDX_CONFIG'], str(context / 'buildx'))
            return subprocess.CompletedProcess(command, 0)

        with patch.object(sys, 'argv', ['dev-container.py', '--build', '--', 'true']), \
                patch.object(DEV.shutil, 'which', return_value='docker'), \
                patch.object(DEV.Path, 'cwd', return_value=ROOT), \
                patch.object(DEV.Path, 'is_file', lambda path: path.name == '.git' or (path.exists() and not path.is_dir())), \
                patch.object(sys.stdin, 'isatty', return_value=False), \
                patch.object(DEV.subprocess, 'run', side_effect=invoke):
            self.assertEqual(DEV.main(), 0)
        self.assertEqual(observed, ['Dockerfile', 'rust-toolchain.toml', 'scripts/install-oracle-ffmpeg.sh',
                                    'scripts/oracle-ffmpeg.env', 'scripts/oracle_pin.py'])

    def test_outside_worktree_is_refused_before_docker(self):
        with patch.object(sys, 'argv', ['dev-container.py', '--', 'true']), \
                patch.object(DEV.Path, 'cwd', return_value=ROOT.parent), \
                patch.object(DEV.subprocess, 'run') as run:
            with self.assertRaises(SystemExit):
                DEV.main()
            run.assert_not_called()

    def test_primary_checkout_metadata_is_not_exposed(self):
        with patch.object(sys, 'argv', ['dev-container.py', '--', 'true']), \
                patch.object(DEV.Path, 'cwd', return_value=ROOT), \
                patch.object(DEV.Path, 'is_file', return_value=False), \
                patch.object(DEV.subprocess, 'run') as run:
            with self.assertRaises(SystemExit):
                DEV.main()
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
