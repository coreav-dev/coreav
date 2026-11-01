#!/usr/bin/env python3
"""Check distribution archives for the committed root notices.

With --third-party, also require target-specific Rust notices beside them.
Archives may be npm tarballs, release tarballs or ZIPs, or Python wheels.
"""
import argparse
import pathlib
import posixpath
import re
import sys
import tarfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[3]
FILES = ('LICENSE-MIT', 'LICENSE-APACHE', 'NOTICE')
THIRD_PARTY = 'THIRD-PARTY-NOTICES.txt'
TARGETS = {
    'linux-x64': 'x86_64-unknown-linux-musl',
    'linux-arm64': 'aarch64-unknown-linux-musl',
    'darwin-arm64': 'aarch64-apple-darwin',
    'darwin-x64': 'x86_64-apple-darwin',
    'win32-x64': 'x86_64-pc-windows-msvc',
}


def members(archive, third_party=False):
    """The archive's regular notice files: path -> bytes."""
    names = FILES + ((THIRD_PARTY,) if third_party else ())
    if archive.suffix in ('.zip', '.whl'):
        with zipfile.ZipFile(archive) as z:
            return {i.filename: z.read(i) for i in z.infolist()
                    if not i.is_dir() and posixpath.basename(i.filename) in names}
    with tarfile.open(archive, 'r:gz') as t:
        return {m.name: t.extractfile(m).read() for m in t.getmembers()
                if m.isfile() and posixpath.basename(m.name) in names}


def problems(archive, third_party=False):
    found = members(archive, third_party)
    expected = {name: (ROOT / name).read_bytes() for name in FILES}
    directories = sorted({posixpath.dirname(path) for path in found})

    def shipped(directory, name):
        return found.get(posixpath.join(directory, name)) == expected[name]

    complete = [directory for directory in directories
                if all(shipped(directory, name) for name in FILES)]
    if complete and not third_party:
        return []
    if complete:
        target = None
        for platform, triple in TARGETS.items():
            if archive.name.startswith(f'coreav-cli-{platform}-') and archive.name.endswith('.tgz'):
                target = triple
            if archive.name.endswith(f'-{triple}.tar.gz') or archive.name.endswith(f'-{triple}.zip'):
                target = triple
        if target is None and re.fullmatch(r'coreav-[0-9][A-Za-z0-9.+-]*\.tgz', archive.name):
            target = 'wasm32-wasip1'
        if target is None:
            return [f'{archive.name}: unrecognized Rust binary archive target']
        pin = (ROOT / 'scripts' / 'rust-dist.env').read_text()
        version = re.search(r'^RUST_DIST_TOOLCHAIN=([0-9.]+)$', pin, re.M)
        commit = re.search(r'^RUST_DIST_COMMIT=([a-f0-9]{40})$', pin, re.M)
        if not version or not commit:
            return [f'{archive.name}: distribution Rust pin is missing']
        standard = f'=== Rust standard library {version[1]} ({commit[1]}) (Rust toolchain) ==='
        for directory in complete:
            data = found.get(posixpath.join(directory, THIRD_PARTY))
            if data is None:
                continue
            try:
                text = data.decode('utf-8')
            except UnicodeDecodeError:
                continue
            if (text.startswith(f'Rust dependency notices for coreav on {target}.\n')
                    and standard in text.splitlines()):
                return []
        return [f'{archive.name}: missing or stale {THIRD_PARTY} for {target} beside the root licence files']
    lacking = [name for name in FILES if not any(shipped(d, name) for d in directories)]
    what = f'missing or changed: {", ".join(lacking)}' if lacking else 'not in one directory'
    return [f'{archive.name}: does not ship {", ".join(FILES)} as committed ({what}); '
            'copy them into the package (docs/release/notices.md)']


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--third-party', action='store_true', help='require target-specific Rust notices')
    parser.add_argument('archives', nargs='*')
    args = parser.parse_args()
    found = [problem for path in args.archives
             for problem in problems(pathlib.Path(path), args.third_party)]
    for problem in found:
        print(f'notices-check: {problem}')
    print(f'notices-check: {len(args.archives)} archives, {len(found)} problems')
    return 1 if found or not args.archives else 0


if __name__ == '__main__':
    sys.exit(main())
