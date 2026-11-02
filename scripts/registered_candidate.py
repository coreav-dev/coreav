"""Refuse manual measurements outside the candidate registered on main."""
import argparse
from pathlib import Path
import re
import sys

README = Path(__file__).resolve().parents[1] / 'docs/measurements/compat-0.1.0/README.md'


def registered(readme):
    lines = [line for line in readme.splitlines() if line.startswith('Release candidate:')]
    match = re.fullmatch(r'Release candidate: `([0-9a-f]{40})`\.', lines[0]) if len(lines) == 1 else None
    if match is None:
        raise ValueError('register exactly one full release candidate SHA in the round README')
    return match[1]


def allowed(readme, ref, commit):
    if ref != 'refs/heads/main':
        raise ValueError('dispatch measurements from main, where the candidate is registered')
    if not commit:
        raise ValueError('commit is required and must match the round README candidate')
    if re.fullmatch(r'[0-9a-f]{40}', commit) is None:
        raise ValueError('commit must be a full lowercase SHA, not a branch, tag or abbreviated SHA')
    if commit != registered(readme):
        raise ValueError('commit differs from the candidate registered in the round README')
    return commit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ref', required=True)
    parser.add_argument('--commit', required=True)
    args = parser.parse_args()
    try:
        commit = allowed(README.read_text(), args.ref, args.commit)
    except (OSError, ValueError) as error:
        print(f'::error::{error}', file=sys.stderr)
        return 1
    print(commit)
    return 0


if __name__ == '__main__':
    sys.exit(main())
