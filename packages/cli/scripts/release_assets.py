#!/usr/bin/env python3
"""Checks that a release's files are exactly its archives and SHA256SUMS
(docs/design/release.md, "The archives"):

    release_assets.py DIR

DIR must hold, and hold only, the ten archives of one version, the default
build (coreav-VERSION-TARGET) and the full build (coreav-full-VERSION-TARGET)
of each of the five targets, .zip for Windows and .tar.gz for the others,
and SHA256SUMS, which lists exactly those archives. Anything else in DIR
would be attached to the release beside them. Exit status 0 when DIR is
exact, 1 otherwise, naming each missing or unexpected file. Python standard
library only.
"""
import pathlib
import re
import sys

TARGETS = {
    'x86_64-unknown-linux-musl': '.tar.gz',
    'aarch64-unknown-linux-musl': '.tar.gz',
    'aarch64-apple-darwin': '.tar.gz',
    'x86_64-apple-darwin': '.tar.gz',
    'x86_64-pc-windows-msvc': '.zip',
}
BUILDS = ('coreav', 'coreav-full')


def expected(version):
    return {f'{build}-{version}-{target}{ext}' for build in BUILDS for target, ext in TARGETS.items()}


def problems(directory):
    names = {path.name for path in directory.iterdir()}
    versions = {m.group(1) for name in names
                if (m := re.fullmatch(r'coreav-(\d+\.\d+\.\d+[^/]*)-x86_64-unknown-linux-musl\.tar\.gz', name))}
    if len(versions) != 1:
        return [f'expected one coreav-VERSION-x86_64-unknown-linux-musl.tar.gz, found {sorted(versions) or "none"}']
    archives = expected(versions.pop())
    found = [f'missing: {name}' for name in sorted(archives - names)]
    found += [f'unexpected: {name}' for name in sorted(names - archives - {'SHA256SUMS'})]
    sums = directory / 'SHA256SUMS'
    if not sums.is_file():
        return found + ['missing: SHA256SUMS']
    listed = [line.split(maxsplit=1)[-1].lstrip('*') for line in sums.read_text().splitlines() if line.strip()]
    if sorted(listed) != sorted(archives):
        found.append(f'SHA256SUMS lists {sorted(listed)}, not the ten archives')
    return found


def main():
    directory = pathlib.Path(sys.argv[1])
    found = problems(directory)
    for problem in found:
        print(f'release-assets: {problem}')
    print(f'release-assets: {len(found)} problems in {directory}')
    return 1 if found else 0


if __name__ == '__main__':
    sys.exit(main())
