"""Capture reviewed, offline dist-exporter build evidence; never acquire tools or inputs."""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import tarfile
import tempfile
import tomllib

from evidence import Invalid, digest, file_hash, integer, keys, loads, read, relative, require, sha, write
from process import run

MANIFEST = "crates/coreav-backend-apple/Cargo.toml"
LOCK = "crates/coreav-backend-apple/Cargo.lock"
PIN = "scripts/rust-dist.env"
PROFILE = {"inherits": "release", "lto": "fat", "codegen-units": 1}
TARGETS = {"macos": "aarch64-apple-darwin", "linux": "x86_64-unknown-linux-gnu"}
LIMIT = 4 * 1024**2
BASE_INPUTS = {MANIFEST, LOCK, PIN, "Cargo.toml", "rust-toolchain.toml"}


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def pin(data):
    values = dict(line.split("=", 1) for line in data.decode().splitlines() if line)
    keys(values, "RUST_DIST_TOOLCHAIN RUST_DIST_COMMIT")
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", values["RUST_DIST_TOOLCHAIN"]) and
            re.fullmatch(r"[0-9a-f]{40}", values["RUST_DIST_COMMIT"]), "build_toolchain_pin")
    return values


def command(version, target):
    return ["cargo", "+" + version, "build", "--locked", "--offline", "--frozen",
            "--manifest-path", MANIFEST, "--profile", "dist", "--example", "videotoolbox",
            "--target", target, "--target-dir", "{target}"]


def validate(value, candidate, host_class, pins):
    keys(value, "schema source configuration configuration_sha256 binary build_output")
    require(value["schema"] == "coreav.videotoolbox-coreav-build/1", "coreav_build_schema")
    keys(value["source"], "commit tree")
    require(value["source"]["commit"] == candidate and type(value["source"]["tree"]) is str and
            re.fullmatch(r"[0-9a-f]{40}", value["source"]["tree"]), "coreav_build_source")
    config = value["configuration"]
    keys(config, "command profile target host toolchain environment inputs packages metadata_sha256")
    require(config["target"] == TARGETS[host_class] and
            config["command"] == command(pins["RUST_DIST_TOOLCHAIN"], TARGETS[host_class]),
            "coreav_build_command")
    require(canonical(config["profile"]) == canonical(PROFILE), "coreav_build_profile")
    keys(config["host"], "system machine release")
    require((config["host"]["system"], config["host"]["machine"]) ==
            (("Darwin", "arm64") if host_class == "macos" else ("Linux", "x86_64")),
            "coreav_build_host")
    require(type(config["host"]["release"]) is str and
            re.fullmatch(r"[A-Za-z0-9_.+-]{1,128}", config["host"]["release"]), "coreav_build_host")
    keys(config["toolchain"], "rustc cargo")
    for text in config["toolchain"].values():
        require(type(text) is str and 0 < len(text) <= 16384, "coreav_build_toolchain")
    rustc = config["toolchain"]["rustc"]
    fields = dict(line.split(": ", 1) for line in rustc.splitlines() if ": " in line)
    require(fields.get("release") == pins["RUST_DIST_TOOLCHAIN"] and
            fields.get("commit-hash") == pins["RUST_DIST_COMMIT"] and
            fields.get("host") == config["target"], "coreav_build_toolchain")
    require(config["toolchain"]["cargo"].startswith("cargo " + pins["RUST_DIST_TOOLCHAIN"] + " "),
            "coreav_build_toolchain")
    keys(config["environment"], "PATH HOME CARGO_HOME RUSTUP_HOME RUSTUP_AUTO_INSTALL LC_ALL")
    require(config["environment"]["RUSTUP_AUTO_INSTALL"] == "0" and config["environment"]["LC_ALL"] == "C",
            "coreav_build_environment")
    for name in ("PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME"):
        digest(config["environment"][name])
    require(type(config["inputs"]) is dict and BASE_INPUTS <= set(config["inputs"]) and
            len(config["inputs"]) <= 256, "coreav_build_inputs")
    for name, expected in config["inputs"].items():
        relative(name)
        require(name in BASE_INPUTS or name.endswith("/Cargo.toml"), "coreav_build_inputs")
        digest(expected)
    require(type(config["packages"]) is list and 0 < len(config["packages"]) <= 256,
            "coreav_build_packages")
    identities = []
    for package in config["packages"]:
        keys(package, "name version source manifest checksum files_sha256")
        for key in ("name", "version"):
            require(type(package[key]) is str and re.fullmatch(r"[A-Za-z0-9_.+-]{1,128}", package[key]),
                    "coreav_build_package")
        if package["source"] is None:
            relative(package["manifest"])
            require(package["manifest"] in config["inputs"] and package["checksum"] is None and
                    package["files_sha256"] is None, "coreav_build_package")
        else:
            require(package["source"] == "registry+https://github.com/rust-lang/crates.io-index" and
                    package["manifest"] is None, "coreav_build_dependency")
            digest(package["checksum"])
            digest(package["files_sha256"])
        identities.append((package["name"], package["version"]))
    require(identities == sorted(set(identities)), "coreav_build_packages")
    digest(config["metadata_sha256"])
    require(value["configuration_sha256"] == sha(canonical(config)), "coreav_build_configuration")
    keys(value["binary"], "sha256 bytes")
    digest(value["binary"]["sha256"])
    integer(value["binary"]["bytes"], 1, 512 * 1024**2)
    keys(value["build_output"], "stdout_sha256 stderr_sha256")
    for expected in value["build_output"].values():
        digest(expected)
    require(len(canonical(value)) <= LIMIT, "coreav_build_bounds")
    return value


def git(root, *arguments):
    from registration import git as registered_git
    return registered_git(root, *arguments)


def clean(root, candidate):
    require(not git(root, "status", "--porcelain", "--untracked-files=all") and
            git(root, "rev-parse", "HEAD").decode().strip() == candidate, "coreav_build_dirty")


def source_identity(root):
    return {path.relative_to(root).as_posix(): sha(b"symlink\0" + os.readlink(path).encode())
            if path.is_symlink() else file_hash(path, root=root)
            for path in root.rglob("*") if path.is_symlink() or path.is_file()}


def registry_files(directory, checksum):
    value = loads(read(directory, ".cargo-checksum.json", LIMIT))
    keys(value, "files package")
    require(value["package"] == checksum and type(value["files"]) is dict and
            0 < len(value["files"]) <= 10000, "coreav_build_dependency")
    paths = list(directory.rglob("*"))
    require(all(not path.is_symlink() and (path.is_dir() or path.is_file()) for path in paths),
            "coreav_build_dependency_files")
    actual = {path.relative_to(directory).as_posix() for path in paths if path.is_file()}
    require(actual == set(value["files"]) | {".cargo-checksum.json", ".cargo-ok"} or
            actual == set(value["files"]) | {".cargo-checksum.json"}, "coreav_build_dependency_files")
    for name, expected in value["files"].items():
        digest(expected)
        require(file_hash(directory / relative(name), root=directory) == expected,
                "coreav_build_dependency_changed")
    return sha(canonical(value["files"]))


def packages(metadata, source, cargo_home):
    require(type(metadata) is dict and type(metadata.get("packages")) is list and
            0 < len(metadata["packages"]) <= 256 and metadata.get("resolve") is not None,
            "coreav_build_metadata")
    locked = tomllib.loads(read(source, LOCK, LIMIT).decode())["package"]
    results = []
    inputs = set(BASE_INPUTS)
    for package in metadata["packages"]:
        manifest = Path(package["manifest_path"])
        record = {"name": package["name"], "version": package["version"], "source": package["source"],
                  "manifest": None, "checksum": None, "files_sha256": None}
        matches = [p for p in locked if p["name"] == record["name"] and p["version"] == record["version"] and
                   p.get("source") == record["source"]]
        require(len(matches) == 1, "coreav_build_lock")
        if record["source"] is None:
            require(manifest.is_relative_to(source), "coreav_build_external_path")
            record["manifest"] = relative(manifest.relative_to(source).as_posix())
            inputs.add(record["manifest"])
        else:
            require(record["source"] == "registry+https://github.com/rust-lang/crates.io-index" and
                    manifest.is_relative_to(cargo_home / "registry" / "src"), "coreav_build_dependency")
            record["checksum"] = matches[0]["checksum"]
            record["files_sha256"] = registry_files(manifest.parent, record["checksum"])
        results.append(record)
    return sorted(results, key=lambda p: (p["name"], p["version"])), inputs


def capture(root, cargo_home, output):
    root, cargo_home = Path(root).resolve(strict=True), Path(cargo_home).resolve(strict=True)
    output = Path(output).absolute()
    require(not output.exists() and not output.parent.resolve().is_relative_to(root), "coreav_build_output")
    candidate = git(root, "rev-parse", "HEAD").decode().strip()
    require(re.fullmatch(r"[0-9a-f]{40}", candidate), "coreav_build_source")
    clean(root, candidate)
    pins = pin(read(root, PIN, 32768))
    require(all(os.environ.get(key) for key in ("PATH", "HOME")), "coreav_build_environment")
    env = {key: os.environ[key] for key in ("PATH", "HOME")}
    env.update(CARGO_HOME=str(cargo_home), RUSTUP_AUTO_INSTALL="0", LC_ALL="C",
               RUSTUP_HOME=os.environ.get("RUSTUP_HOME", str(Path(env["HOME"]) / ".rustup")))
    require(not any((cargo_home / name).exists() for name in ("config", "config.toml")),
            "coreav_build_cargo_config")
    with tempfile.TemporaryDirectory() as temporary:
        work = Path(temporary)
        count = 0

        def observe(arguments, cwd=root, timeout=30):
            nonlocal count
            count += 1
            run(arguments, work, timeout, stdout=f"{count}.out", stderr=f"{count}.err", env=env, cwd=cwd)
            return (read(work, f"{count}.out", 16 * LIMIT), read(work, f"{count}.err", 16 * LIMIT))

        installed = observe(["rustup", "toolchain", "list"])[0].decode().splitlines()
        require(any(line.split()[0].split("-")[0] == pins["RUST_DIST_TOOLCHAIN"] for line in installed if line),
                "coreav_build_toolchain_missing")
        rustc = observe(["rustc", "+" + pins["RUST_DIST_TOOLCHAIN"], "-vV"])[0].decode()
        cargo = observe(["cargo", "+" + pins["RUST_DIST_TOOLCHAIN"], "-vV"])[0].decode()
        target = next((line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: ")), None)
        require(target in TARGETS.values(), "coreav_build_host")
        host_class = next(name for name, value in TARGETS.items() if value == target)
        host = {name: observe(["uname", flag])[0].decode().strip()
                for name, flag in (("system", "-s"), ("machine", "-m"), ("release", "-r"))}
        archive = work / "source.tar"
        observe(["git", "-C", str(root), "archive", "--format=tar", "--output=" + str(archive), candidate])
        require(archive.stat().st_size <= 512 * 1024**2, "coreav_build_source_bounds")
        source = work / "source"
        source.mkdir()
        with tarfile.open(archive) as contents:
            for entry in contents.getmembers():
                require(entry.isfile() or entry.isdir() or entry.issym(), "coreav_build_source_type")
                relative(entry.name.rstrip("/"))
            contents.extractall(source, filter="data")
        require(not list(source.rglob(".cargo/config")) and not list(source.rglob(".cargo/config.toml")) and
                not any((parent / ".cargo" / name).exists() for parent in source.parents
                        for name in ("config", "config.toml")), "coreav_build_cargo_config")
        require(pin(read(source, PIN, 32768)) == pins, "coreav_build_pin")
        profile = tomllib.loads(read(source, MANIFEST, LIMIT).decode()).get("profile", {})
        require(canonical(profile) == canonical({"dist": PROFILE}), "coreav_build_profile")
        source_files = source_identity(source)
        metadata_bytes = observe(["cargo", "+" + pins["RUST_DIST_TOOLCHAIN"], "metadata", "--locked", "--offline",
                                  "--frozen", "--format-version", "1", "--manifest-path", MANIFEST,
                                  "--filter-platform", target], source)[0]
        metadata = loads(metadata_bytes)
        dependencies, names = packages(metadata, source, cargo_home)
        config = {"command": command(pins["RUST_DIST_TOOLCHAIN"], target), "profile": PROFILE,
                  "target": target, "host": host, "toolchain": {"rustc": rustc, "cargo": cargo},
                  "environment": {key: sha(value.encode()) if key in ("PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME")
                                  else value for key, value in env.items()},
                  "inputs": {name: file_hash(source / name, root=source) for name in sorted(names)},
                  "packages": dependencies,
                  "metadata_sha256": sha(metadata_bytes.replace(str(source).encode(), b"{source}").replace(
                      str(cargo_home).encode(), b"{cargo_home}"))}
        value = {"schema": "coreav.videotoolbox-coreav-build/1",
                 "source": {"commit": candidate, "tree": git(root, "rev-parse", candidate + "^{tree}").decode().strip()},
                 "configuration": config, "configuration_sha256": sha(canonical(config)),
                 "binary": {"sha256": "0" * 64, "bytes": 1},
                 "build_output": {"stdout_sha256": "0" * 64, "stderr_sha256": "0" * 64}}
        validate(value, candidate, host_class, pins)
        actual_command = [str(work / "target") if arg == "{target}" else arg for arg in config["command"]]
        stdout, stderr = observe(actual_command, source, 3600)
        binary = work / "target" / target / "dist" / "examples" / "videotoolbox"
        value["binary"] = {"sha256": file_hash(binary, limit=512 * 1024**2), "bytes": binary.stat().st_size}
        value["build_output"] = {"stdout_sha256": sha(stdout), "stderr_sha256": sha(stderr)}
        require(packages(metadata, source, cargo_home)[0] == dependencies and
                source_identity(source) == source_files,
                "coreav_build_inputs_changed")
        clean(root, candidate)
        validate(value, candidate, host_class, pins)
        with tempfile.TemporaryDirectory(dir=output.parent) as staging:
            staged = Path(staging) / "build"
            staged.mkdir()
            shutil.copyfile(binary, staged / "videotoolbox")
            (staged / "videotoolbox").chmod(0o755)
            require(file_hash(staged / "videotoolbox") == value["binary"]["sha256"], "coreav_build_binary")
            write(staged / "build.json", value)
            require(not output.exists(), "coreav_build_output")
            staged.rename(output)
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--cargo-home", required=True, type=Path, help="Existing prepared registry cache; no config overrides")
    parser.add_argument("--output", required=True, type=Path, help="New directory outside the checkout")
    args = parser.parse_args()
    try:
        capture(args.root, args.cargo_home, args.output)
    except (Invalid, OSError, ValueError, KeyError, UnicodeError, tarfile.TarError) as error:
        print("refused: " + (error.code if isinstance(error, Invalid) else "coreav_build_capture"))
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
