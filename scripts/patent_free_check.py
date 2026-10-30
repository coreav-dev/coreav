#!/usr/bin/env python3
"""Checks that default builds hold no implementation of a still-patented
codec: the
crates of PATENTED are reached only through an opt-in feature
(docs/design/ci-checks.md, "Default builds and patents").

    patent_free_check.py [--root DIR]

For every workspace member that is not itself in PATENTED, and for each
build of BUILDS that a package script makes with other features, runs
`cargo tree` over every target with the normal and build edges and fails,
naming the build and the crate, when a crate of PATENTED is in the graph.
Also fails when a crate of PATENTED is no workspace member, so that a
renamed crate cannot leave the list checking nothing. Exit status 0 when
every build is clean, 1 otherwise. Python 3.11 standard library only.
"""
import argparse
import json
import pathlib
import subprocess
import sys

# The crates that implement a codec whose patents are still in force where
# CoreAV is distributed, with the codec. A crate for another such codec
# (HEVC, for example) adds a line in the pull request that adds it; a line
# goes only after the project's patent review.
PATENTED = {
    'coreav-h264': 'H.264',
    'coreav-kernels-h264': 'H.264',
    'coreav-backend-h264': 'H.264',
    # The sources disagree on whether AAC-LC patents are still in force; the
    # decoder ships opt-in, as H.264 does (docs/design/audio-decode.md).
    'coreav-aac': 'AAC',
}

# Builds that a package script makes with other than the default features:
# (label, cargo arguments). Default builds of every member are checked
# without being listed.
BUILDS = [
    ('coreav-cli for npm (packages/cli/scripts/build.sh)',
     ['-p', 'coreav-cli', '--no-default-features', '--features', 'cli']),
]


def cargo(root, args):
    run = subprocess.run(['cargo', *args], cwd=root, capture_output=True, text=True, check=False)
    if run.returncode != 0:
        sys.stderr.write(run.stderr)
        sys.exit(f'cargo {" ".join(args)} failed (exit {run.returncode})')
    return run.stdout


def tree(root, args):
    out = cargo(root, ['tree', '--locked', '--target', 'all', '-e', 'normal,build',
                       '--prefix', 'none', '-f', '{p}', *args])
    return {line.split(' ', 1)[0] for line in out.splitlines() if line.strip()}


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--root', type=pathlib.Path, default=pathlib.Path(__file__).resolve().parents[1])
    args = p.parse_args()
    meta = json.loads(cargo(args.root, ['metadata', '--locked', '--no-deps', '--format-version', '1']))
    members = sorted(pkg['name'] for pkg in meta['packages'])
    problems = [f'{name} is in PATENTED but is no workspace member' for name in PATENTED if name not in members]
    builds = [(f'{name}, default features', ['-p', name]) for name in members if name not in PATENTED]
    builds += BUILDS
    for label, cargo_args in builds:
        found = sorted(tree(args.root, cargo_args) & set(PATENTED))
        for name in found:
            problems.append(f'{label}: links {name} ({PATENTED[name]})')
    for problem in problems:
        print(problem)
    print(f'{len(builds)} builds checked, {len(problems)} problems')
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
