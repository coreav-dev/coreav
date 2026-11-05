"""Run the explicitly oracle-free selection on an unregistered platform."""
import json
from pathlib import Path
import platform
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent


def command(arguments):
    if arguments[:2] != ['cargo', 'test']:
        raise ValueError('oracle-free selection requires cargo test')
    selected = json.loads((ROOT / 'test-support/oracle-dependent.json').read_text())
    filters = sorted({name for names in selected.values() for name in names})
    if not filters:
        raise ValueError('oracle-dependent test selection is empty')
    result = list(arguments)
    if '--' not in result:
        result.append('--')
    result.append('--exact')
    for name in filters:
        result.extend(('--skip', name))
    return result


if __name__ == '__main__':
    arguments = sys.argv[1:]
    if sys.platform != 'darwin' or platform.machine() != 'arm64':
        arguments = command(arguments)
    sys.exit(subprocess.run(arguments, check=False).returncode)
