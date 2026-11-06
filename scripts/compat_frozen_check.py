"""Check registered frozen source identities without running a measurement."""
import argparse
from pathlib import Path
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import compat_sources


def check(root):
    tooling = compat_sources.git(root, 'rev-parse', 'HEAD')
    readme = compat_sources.git(root, 'show', f'{tooling}:{compat_sources.README}')
    candidate = compat_sources.registered_candidate.registered(readme)
    return compat_sources.plan(root, 'pull_request', '', candidate, tooling)


def main():
    argparse.ArgumentParser(description=__doc__).parse_args()
    try:
        sources = check(Path.cwd())
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f'::error::compatibility-frozen-inputs: invalid-source-identity: {error}', file=sys.stderr)
        return 1
    count = sum(source['owner'] == 'tooling' for source in sources['frozen'].values())
    print(f'compatibility frozen sources: {count} tooling inputs verified')
    return 0


if __name__ == '__main__':
    sys.exit(main())
