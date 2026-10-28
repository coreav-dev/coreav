#!/usr/bin/env python3
"""Keeps docs/unsafe-sites.md current. Fails, naming the file, when:

- a Rust file of the workspace, its packages or its fuzz projects uses
  `unsafe` (a block, function, impl, trait or extern block, an `unsafe(...)`
  attribute such as `#[unsafe(no_mangle)]`, the attributes that need it
  (`no_mangle`, `export_name`, `link_section`), or an `allow`, `expect` or
  `warn` of `unsafe_code`) and the table has no row naming it;
- a crate's `Cargo.toml` does not take the workspace lints and sets
  `unsafe_code` to anything but `forbid`, or sets no lints at all, without
  a row naming that `Cargo.toml`; or the workspace's own level is not
  `forbid`;
- a crate marked `[package.metadata.coreav] test-only = true` is depended
  on outside another crate's `[dev-dependencies]`, which would link its
  `unsafe` into a library, a binary or a wheel. A rename, through
  `package = "..."` or through `[workspace.dependencies]`, is resolved
  first. Like everything else here, this reads the manifests under
  `SCOPES`; a crate added outside them is not seen;
- a row names a file that no longer exists.

    unsafe_sites_check.py [--root DIR]

Rows are table lines of docs/unsafe-sites.md whose first cell is a file
path in backticks. Spikes are outside the list: their `unsafe`
never reaches production code. Exit status 0 when the table is complete, 1
otherwise. Python 3.11 standard library only.
"""
import argparse
import pathlib
import re
import sys
import tomllib

SCOPES = ('crates', 'tools', 'packages', 'bench')
USE = re.compile(
    r'\bunsafe\s*(\{|fn\b|impl\b|trait\b|extern\b|\()'
    r'|#!?\[\s*(no_mangle|export_name|link_section)\b'
    r'|\b(allow|expect|warn)\s*\([^)]*\bunsafe_code\b')


def skipped(root, path):
    """Spikes, Node packages, and build directories: a `target` directory
    next to a `Cargo.toml`."""
    rel = path.relative_to(root)
    if 'spikes' in rel.parts or 'node_modules' in rel.parts:
        return True
    for i, part in enumerate(rel.parts[:-1]):
        if part == 'target' and (root.joinpath(*rel.parts[:i]) / 'Cargo.toml').exists():
            return True
    return False


def code(text):
    """Rust source without comments and string literals, so that the word
    in a comment or a string is not a use. Raw strings, nested block
    comments and escapes are handled; char literals are kept (they cannot
    hold the patterns)."""
    out, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith('//', i):
            j = text.find('\n', i)
            i = n if j < 0 else j
        elif text.startswith('/*', i):
            depth, i = 1, i + 2
            while i < n and depth:
                if text.startswith('/*', i):
                    depth, i = depth + 1, i + 2
                elif text.startswith('*/', i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
        elif c == 'r' and re.match(r'r(#*)"', text[i:]) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == '_')):
            hashes = re.match(r'r(#*)"', text[i:]).group(1)
            end = text.find('"' + hashes, i + 2 + len(hashes))
            i = n if end < 0 else end + 1 + len(hashes)
            out.append('""')
        elif c == '"':
            i += 1
            while i < n and text[i] != '"':
                i += 2 if text[i] == '\\' else 1
            i += 1
            out.append('""')
        else:
            out.append(c)
            i += 1
    return ''.join(out)


def test_only(data):
    """Whether a manifest marks its crate test-only: its code may be
    compiled into test binaries and nothing else."""
    return data.get('package', {}).get('metadata', {}).get('coreav', {}).get('test-only') is True


def shipped_dependencies(data, workspace):
    """Every crate a manifest depends on outside `[dev-dependencies]`, by
    package name: the table key, or the `package` it renames, or the name
    the root `[workspace.dependencies]` entry of that key renames. Covers
    `[dependencies]`, `[build-dependencies]` and their per-target forms."""
    names = set()
    tables = [data.get('dependencies', {}), data.get('build-dependencies', {})]
    for target in data.get('target', {}).values():
        tables.append(target.get('dependencies', {}))
        tables.append(target.get('build-dependencies', {}))
    for table in tables:
        for key, spec in table.items():
            if not isinstance(spec, dict):
                names.add(key)
            elif spec.get('workspace') is True:
                inherited = workspace.get(key, {})
                names.add(inherited.get('package', key) if isinstance(inherited, dict) else key)
            else:
                names.add(spec.get('package', key))
    return names


def loosened(path):
    """Whether a crate's manifest leaves the workspace `forbid`: no
    `[lints] workspace = true`, and `unsafe_code` other than `forbid` or
    missing."""
    data = tomllib.loads(path.read_text())
    if 'package' not in data:
        return False
    lints = data.get('lints', {})
    if lints.get('workspace') is True:
        return False
    level = lints.get('rust', {}).get('unsafe_code')
    if isinstance(level, dict):
        level = level.get('level')
    return level != 'forbid'


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--root', default=pathlib.Path(__file__).resolve().parent.parent, type=pathlib.Path)
    root = p.parse_args().root.resolve()
    table = (root / 'docs' / 'unsafe-sites.md').read_text()
    listed = set(re.findall(r'^\|\s*`([^`]+)`', table, re.M))
    found, problems = set(), []
    workspace = tomllib.loads((root / 'Cargo.toml').read_text())
    level = workspace.get('workspace', {}).get('lints', {}).get('rust', {}).get('unsafe_code')
    if level != 'forbid':
        problems.append(f'unsafe-sites: the workspace sets unsafe_code to {level!r}, not "forbid"')
    manifests = []
    for scope in SCOPES:
        base = root / scope
        if not base.exists():
            continue
        for path in base.rglob('*.rs'):
            if not skipped(root, path) and USE.search(code(path.read_text(errors='replace'))):
                found.add(str(path.relative_to(root)))
        for path in base.rglob('Cargo.toml'):
            if skipped(root, path):
                continue
            if loosened(path):
                found.add(str(path.relative_to(root)))
            manifests.append((path, tomllib.loads(path.read_text())))
    confined = {
        data['package']['name'] for _, data in manifests
        if 'package' in data and test_only(data)
    }
    inherited = workspace.get('workspace', {}).get('dependencies', {})
    for path, data in manifests:
        for name in sorted(confined & shipped_dependencies(data, inherited)):
            where = path.relative_to(root)
            problems.append(
                f'unsafe-sites: {where} depends on the test-only crate {name} outside '
                '[dev-dependencies], which would ship its unsafe')
    for f in sorted(found - listed):
        problems.append(f'unsafe-sites: {f} uses or allows unsafe but has no row in docs/unsafe-sites.md')
    for f in sorted(f for f in listed if not (root / f).exists()):
        problems.append(f'unsafe-sites: docs/unsafe-sites.md lists {f}, which does not exist')
    for line in problems:
        print(line)
    print(f'unsafe-sites: {len(found)} file(s) with unsafe, {len(listed)} row(s)')
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
