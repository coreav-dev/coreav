#!/usr/bin/env python3
"""Run trusted Linux development commands in Docker, without host credentials."""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', choices=('linux/amd64', 'linux/arm64'), default='linux/amd64')
    parser.add_argument('--build', action='store_true', help='build the image before running')
    parser.add_argument('--network', action='store_true', help='allow runtime networking for dependency acquisition')
    parser.add_argument('command', nargs=argparse.REMAINDER, help='command after --; defaults to bash')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    cwd = Path.cwd().resolve()
    if not cwd.is_relative_to(root):
        parser.error('run from inside this worktree')
    if Path.home().resolve().is_relative_to(root) or ',' in str(root):
        parser.error('the worktree must not contain the home directory or a comma')
    docker = shutil.which('docker')
    if not docker:
        parser.error('docker is not on PATH; start Docker Desktop and add its CLI to PATH')
    arch = args.platform.split('/')[1]
    inputs = ('rust-toolchain.toml', 'scripts/dev/Dockerfile',
              'scripts/install-oracle-ffmpeg.sh', 'scripts/oracle-ffmpeg.env',
              'scripts/oracle_pin.py')
    identity = hashlib.sha256(b''.join((root / name).read_bytes() for name in inputs)).hexdigest()[:16]
    image = f'coreav-dev:{arch}-{identity}'
    if args.build:
        with tempfile.TemporaryDirectory(prefix='coreav-dev-build-') as directory:
            context = Path(directory)
            (context / 'scripts').mkdir()
            for name in ('install-oracle-ffmpeg.sh', 'oracle-ffmpeg.env', 'oracle_pin.py'):
                shutil.copyfile(root / 'scripts' / name, context / 'scripts' / name)
            shutil.copyfile(root / 'rust-toolchain.toml', context / 'rust-toolchain.toml')
            shutil.copyfile(root / 'scripts/dev/Dockerfile', context / 'Dockerfile')
            environment = {**os.environ, 'BUILDX_CONFIG': str(context / 'buildx')}
            subprocess.run([docker, 'build', '--platform', args.platform, '-t', image, str(context)],
                           check=True, env=environment)
    key = hashlib.sha256(os.fsencode(root)).hexdigest()[:16]
    command = args.command
    if command[:1] == ['--']:
        command = command[1:]
    run = [docker, 'run', '--rm', '-i', '--init', '--pull', 'never', '--platform', args.platform,
           '--user', '1000:1000', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
           '--memory', '6g', '--memory-swap', '6g', '--pids-limit', '512', '--cpus', '2',
           '--network', 'bridge' if args.network else 'none',
           '--mount', f'type=bind,source={root},target=/work',
           '--mount', f'type=volume,source=coreav-dev-cargo-{arch},target=/home/dev/.cargo',
           '--mount', f'type=volume,source=coreav-dev-target-{arch}-{key},target=/target',
           '--workdir', str(Path('/work') / cwd.relative_to(root))]
    if sys.stdin.isatty():
        run += ['-t']
    run += [image, *(command or ['bash'])]
    return subprocess.run(run).returncode


if __name__ == '__main__':
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as error:
        sys.exit(error.returncode)
