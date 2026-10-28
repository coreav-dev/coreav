#!/usr/bin/env python3
"""Choose the fuzz targets a CI run must exercise.

A push to main runs every target. A pull request runs every target of each
fuzz project that the change can affect: the project's own directory, or any
workspace crate the project depends on, directly or through other workspace
crates. A change to the shared build configuration (root manifests, lock
file, toolchain, lint configuration, the CI workflow or this script) affects
every project.

Usage:
  fuzz_plan.py --all
  fuzz_plan.py --changed FILE   (NUL-separated changed paths, from git diff -z)
  fuzz_plan.py --projects

Prints one "FUZZ_DIR TARGET" line per target to run; with --projects, one
FUZZ_DIR line per fuzz project, for the license check of each project's own
dependency graph. Fuzz projects are found as crates/*/fuzz; their targets
are the projects' [[bin]] targets. Uses `cargo metadata` and the Python
standard library only.
"""

import argparse
import json
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

# Paths whose change affects every fuzz project.
GLOBAL = (
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    "clippy.toml",
    ".github/workflows/ci.yml",
    "scripts/fuzz_plan.py",
)


def is_global(path):
    # Cargo reads .cargo/config.toml in every parent directory of a build.
    return path in GLOBAL or path.startswith(".cargo/") or "/.cargo/" in path


def fuzz_projects():
    return sorted(p.parent for p in ROOT.glob("crates/*/fuzz/Cargo.toml"))


def metadata(project):
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1",
         "--manifest-path", str(project / "Cargo.toml")],
        cwd=project, check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out)


def targets(project, meta):
    """The fuzz project's own [[bin]] targets."""
    manifest = (project / "Cargo.toml").resolve()
    names = []
    for package in meta["packages"]:
        if pathlib.Path(package["manifest_path"]).resolve() == manifest:
            names = [t["name"] for t in package["targets"] if "bin" in t["kind"]]
    return sorted(names)


def local_dirs(meta):
    """Directories of the workspace crates the fuzz project builds, itself included."""
    return [
        pathlib.Path(os.path.abspath(pathlib.Path(p["manifest_path"]).parent))
        for p in meta["packages"]
        if p.get("source") is None  # a path dependency, not a registry crate
    ]


def affected(changed, meta):
    if any(is_global(c) for c in changed):
        return True
    dirs = local_dirs(meta)
    for c in changed:
        # Normalised without following symlinks: a path counts where it is.
        path = pathlib.Path(os.path.abspath(ROOT / c))
        if any(path == d or d in path.parents for d in dirs):
            return True
    return False


def main():
    parser = argparse.ArgumentParser()
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--all", action="store_true")
    group.add_argument("--changed", type=pathlib.Path)
    group.add_argument("--projects", action="store_true")
    args = parser.parse_args()

    if args.projects:
        projects = fuzz_projects()
        for project in projects:
            print(project.relative_to(ROOT))
        if not projects:
            sys.stderr.write("no fuzz projects found\n")
            sys.exit(2)
        return

    changed = []
    if args.changed:
        # NUL-separated, from git diff -z: no quoting of unusual paths.
        changed = [c for c in args.changed.read_bytes().decode().split("\0") if c.strip()]

    planned = 0
    for project in fuzz_projects():
        meta = metadata(project)
        if args.all or affected(changed, meta):
            rel = project.relative_to(ROOT)
            for target in targets(project, meta):
                print(f"{rel} {target}")
                planned += 1
    if args.all and planned == 0:
        sys.stderr.write("no fuzz targets found\n")
        sys.exit(2)


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as e:
        sys.stderr.write(e.stderr or str(e))
        sys.exit(2)
