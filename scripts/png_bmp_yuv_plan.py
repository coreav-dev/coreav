#!/usr/bin/env python3
"""Validate the separately registered, main-only image acquisition dispatch."""
import argparse
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
REQUIRED = ('scripts/png_bmp_yuv_plan.py', 'docs/design/png-bmp-yuv-actions.md',
            'tools/compat-corpus/png_bmp_yuv.py', 'tools/compat-corpus/v2-fixtures.json')


def candidate(event, ref, selection, registration, commit, root=ROOT):
    if (event, ref, selection, registration) != ('workflow_dispatch', 'refs/heads/main', 'v2', 'png-bmp-yuv-v1'):
        raise ValueError('dispatch_selection')
    if re.fullmatch(r'[0-9a-f]{40}', commit) is None:
        raise ValueError('candidate_sha')
    def git(*args):
        return subprocess.run(['git', '-C', str(root), *args], capture_output=True, text=True, timeout=30)
    status = git('status', '--porcelain', '--untracked-files=normal')
    if status.returncode or status.stdout:
        raise ValueError('source_not_clean')
    resolved = git('rev-parse', '--verify', commit + '^{commit}')
    if resolved.returncode or resolved.stdout.strip() != commit:
        raise ValueError('candidate_missing')
    if git('merge-base', '--is-ancestor', commit, 'origin/main').returncode:
        raise ValueError('candidate_not_on_main')
    if any(git('cat-file', '-e', commit + ':' + path).returncode for path in REQUIRED):
        raise ValueError('candidate_without_integration')
    return commit


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('event', 'ref', 'set', 'registration', 'commit'):
        parser.add_argument('--' + name, required=True)
    args = parser.parse_args()
    try:
        print(candidate(args.event, args.ref, args.set, args.registration, args.commit))
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        reason = str(error) if isinstance(error, ValueError) else 'source_query'
        print('image-dispatch: ' + reason, file=sys.stderr)
        return 2
    return 0


if __name__ == '__main__':
    sys.exit(main())
