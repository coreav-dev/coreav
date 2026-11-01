#!/usr/bin/env python3
"""Checks that each distribution archive ships the notices
(docs/release/notices.md):

    notices_check.py ARCHIVE...

ARCHIVE is an npm tarball (.tgz), a release archive (.tar.gz or .zip) or a
Python wheel (.whl). Each must hold LICENSE-MIT, LICENSE-APACHE and NOTICE
in one directory, each the same bytes as the repository's. Exit status 0
when every archive does, 1 otherwise or when no archive is named, naming
each archive and what it lacks. Python standard library only.
"""
import pathlib
import posixpath
import sys
import tarfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
FILES = ('LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE')


def members(archive):
    """The archive's regular files named as one of FILES: path -> bytes."""
    if archive.suffix in ('.zip', '.whl'):
        with zipfile.ZipFile(archive) as z:
            return {i.filename: z.read(i) for i in z.infolist()
                    if not i.is_dir() and posixpath.basename(i.filename) in FILES}
    with tarfile.open(archive, 'r:gz') as t:
        return {m.name: t.extractfile(m).read() for m in t.getmembers()
                if m.isfile() and posixpath.basename(m.name) in FILES}


def problems(archive):
    found = members(archive)
    expected = {name: (ROOT / name).read_bytes() for name in FILES}
    directories = sorted({posixpath.dirname(path) for path in found})

    def shipped(directory, name):
        return found.get(posixpath.join(directory, name)) == expected[name]

    if any(all(shipped(d, name) for name in FILES) for d in directories):
        return []
    lacking = [name for name in FILES if not any(shipped(d, name) for d in directories)]
    what = f'missing or changed: {", ".join(lacking)}' if lacking else 'not in one directory'
    return [f'{archive.name}: does not ship {", ".join(FILES)} as committed ({what}); '
            'copy them into the package (docs/release/notices.md)']


def main():
    archives = sys.argv[1:]
    found = [problem for path in archives for problem in problems(pathlib.Path(path))]
    for problem in found:
        print(f'notices-check: {problem}')
    print(f'notices-check: {len(archives)} archives, {len(found)} problems')
    return 1 if found or not archives else 0


if __name__ == '__main__':
    sys.exit(main())
