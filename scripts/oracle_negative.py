"""Require a complete suite to reject the system FFmpeg 6.1.1 selection."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import tempfile


def check(command, directory, log):
    versions = []
    for name in ('ffmpeg', 'ffprobe'):
        binary = directory / name
        result = subprocess.run([str(binary), '-version'], capture_output=True,
                                text=True, check=True, timeout=10)
        line = result.stdout.splitlines()[0]
        if not line.startswith(f'{name} version 6.1.1'):
            raise ValueError(f'negative control requires system {name} 6.1.1')
        versions.append(line)
    log.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='unverified-oracle-') as temporary:
        selected = Path(temporary)
        for name in ('ffmpeg', 'ffprobe'):
            (selected / name).symlink_to(directory / name)
        env = dict(os.environ, PATH=str(selected) + os.pathsep + os.environ.get('PATH', ''))
        for name in ('COREAV_FFMPEG', 'COREAV_FFPROBE', 'COREAV_V2_FFMPEG', 'COREAV_V2_FFPROBE'):
            env.pop(name, None)
        with log.open('wb') as output:
            result = subprocess.run(command, env=env, stdout=output, stderr=subprocess.STDOUT,
                                    check=False)
    diagnostic = log.read_text(errors='replace')
    if result.returncode == 0 or 'stage=oracle_qualification' not in diagnostic:
        raise ValueError('unverified suite did not fail at oracle qualification')
    for line in versions:
        print(line)
    print(f'Negative suite exit {result.returncode}; oracle-qualification failures confirmed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--system-dir', type=Path, default=Path('/usr/bin'))
    parser.add_argument('--log', type=Path, required=True)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ['--'] else args.command
    if not command:
        parser.error('a suite command is required')
    try:
        check(command, args.system_dir.resolve(), args.log)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f'oracle-negative: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
