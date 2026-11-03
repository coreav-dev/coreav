"""Synthetic build records and mocked tools only; no compiler or media execution."""

import copy
import io
import os
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import coreav_build as build
import evidence
import registration
from test_tools import h, registered


PINS = build.pin(evidence.read(registration.ROOT, build.PIN, 32768))
CANDIDATE = "a" * 40
TREE = "b" * 40


def sources():
    return {build.PIN: evidence.read(registration.ROOT, build.PIN, 32768),
            build.MANIFEST: b'[package]\nname = "coreav-backend-apple"\nversion = "0.0.1"\n'
            b'[profile.dist]\ninherits = "release"\nlto = "fat"\ncodegen-units = 1\n',
            build.LOCK: b'[[package]]\nname = "coreav-backend-apple"\nversion = "0.0.1"\n',
            "Cargo.toml": b"[workspace]\n", "rust-toolchain.toml": b'[toolchain]\nchannel = "stable"\n',
            "crates/coreav-backend-apple/examples/videotoolbox.rs": b"fn main() {}\n"}


def record(host="macos"):
    target = build.TARGETS[host]
    config = {"command": build.command(PINS["RUST_DIST_TOOLCHAIN"], target), "profile": dict(build.PROFILE),
              "target": target, "host": {"system": "Darwin" if host == "macos" else "Linux",
                                          "machine": "arm64" if host == "macos" else "x86_64", "release": "test-1"},
              "toolchain": {"rustc": "rustc " + PINS["RUST_DIST_TOOLCHAIN"] + "\nrelease: " +
                            PINS["RUST_DIST_TOOLCHAIN"] + "\ncommit-hash: " + PINS["RUST_DIST_COMMIT"] +
                            "\nhost: " + target + "\n", "cargo": "cargo " + PINS["RUST_DIST_TOOLCHAIN"] + " (test)\n"},
              "environment": {"PATH": h("path"), "HOME": h("home"), "CARGO_HOME": h("cargo-home"),
                              "RUSTUP_HOME": h("rustup-home"), "RUSTUP_AUTO_INSTALL": "0", "LC_ALL": "C"},
              "inputs": {name: evidence.sha(sources()[name]) for name in sorted(build.BASE_INPUTS)},
              "packages": [{"name": "coreav-backend-apple", "version": "0.0.1", "source": None,
                            "manifest": build.MANIFEST, "checksum": None, "files_sha256": None}],
              "metadata_sha256": h("metadata")}
    return {"schema": "coreav.videotoolbox-coreav-build/1", "source": {"commit": CANDIDATE, "tree": TREE},
            "configuration": config, "configuration_sha256": evidence.sha(build.canonical(config)),
            "binary": {"sha256": h("binary-" + host), "bytes": 10},
            "build_output": {"stdout_sha256": h("stdout"), "stderr_sha256": h("stderr")}}


class BuildRecords(unittest.TestCase):
    def test_closed_record_binds_source_profile_command_target_and_toolchain(self):
        build.validate(record(), CANDIDATE, "macos", PINS)
        mutations = [lambda r: r["source"].update(commit="c" * 40),
                     lambda r: r["source"].update(tree="not-a-tree"),
                     lambda r: r.update(pass_build=True),
                     lambda r: r["configuration"]["command"].append("--release"),
                     lambda r: r["configuration"]["profile"].update(lto="thin"),
                     lambda r: r["configuration"]["profile"].update(**{"codegen-units": True}),
                     lambda r: r["configuration"].update(target=build.TARGETS["linux"]),
                     lambda r: r["configuration"]["host"].update(machine="x86_64"),
                     lambda r: r["configuration"]["toolchain"].update(rustc="rustc stable"),
                     lambda r: r["configuration"]["toolchain"].update(cargo="cargo 0.0.0 (other)"),
                     lambda r: r["configuration"]["toolchain"].update(rustc=r["configuration"]["toolchain"]["rustc"].replace(
                         PINS["RUST_DIST_COMMIT"], "0" * 40)),
                     lambda r: r["configuration"]["inputs"].pop(build.LOCK),
                     lambda r: r["configuration"]["packages"].append(r["configuration"]["packages"][0]),
                     lambda r: r["binary"].update(bytes=0)]
        for mutate in mutations:
            value = record()
            mutate(value)
            value["configuration_sha256"] = evidence.sha(build.canonical(value["configuration"]))
            with self.subTest(mutate=mutate), self.assertRaises(evidence.Invalid):
                build.validate(value, CANDIDATE, "macos", PINS)
        value = record()
        value["configuration"]["metadata_sha256"] = h("other")
        with self.assertRaisesRegex(evidence.Invalid, "coreav_build_configuration"):
            build.validate(value, CANDIDATE, "macos", PINS)

    def check_registration(self, mutate=None, drift=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, data in sources().items():
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_bytes(data)
            reg = registered()
            records = {host: record(host) for host in ("macos", "linux")}
            reg["executables"]["coreav"]["sha256"] = records["macos"]["binary"]["sha256"]
            reg["executables"]["coreav"]["configuration_sha256"] = records["macos"]["configuration_sha256"]
            reg["metric_tools"]["coreav_decoder"] = records["linux"]["binary"]["sha256"]
            if mutate:
                mutate(reg, records)
            for host, value in records.items():
                value["configuration_sha256"] = evidence.sha(build.canonical(value["configuration"]))
                path = root / reg["coreav_builds"][host]["file"]
                evidence.write(path, value)
                reg["coreav_builds"][host]["sha256"] = evidence.file_hash(path)
            if drift:
                with (root / reg["coreav_builds"]["macos"]["file"]).open("ab") as output:
                    output.write(b"\n")

            def git(_root, *args):
                if args[0] == "rev-parse":
                    return (TREE + "\n").encode()
                self.assertEqual(args[0], "show")
                commit, name = args[1].split(":", 1)
                self.assertEqual(commit, CANDIDATE)
                return sources()[name]

            with mock.patch.object(registration, "git", side_effect=git), mock.patch.object(
                    registration, "committed") as committed:
                registration.coreav_build_records(reg, root)
                self.assertEqual(committed.call_count, 2)
                for host in ("macos", "linux"):
                    reference = reg["coreav_builds"][host]
                    committed.assert_any_call(root, reference["file"], reference["review_commit"])

    def test_registration_binds_both_reviewed_builds_and_actual_configurations(self):
        self.check_registration()
        mutations = [lambda reg, records: reg["executables"]["coreav"].update(sha256=h("other")),
                     lambda reg, records: reg["executables"]["coreav"].update(configuration_sha256=h("other")),
                     lambda reg, records: reg["metric_tools"].update(coreav_decoder=h("other")),
                     lambda reg, records: records["linux"]["source"].update(commit="c" * 40),
                     lambda reg, records: records["macos"]["source"].update(tree="c" * 40),
                     lambda reg, records: records["macos"]["configuration"]["inputs"].update(
                         **{build.LOCK: h("other")}),
                     lambda reg, records: records["linux"]["configuration"]["packages"][0].update(version="0.0.2"),
                     lambda reg, records: records.update(linux=copy.deepcopy(records["macos"]))]
        for mutate in mutations:
            with self.subTest(mutate=mutate), self.assertRaises(evidence.Invalid):
                self.check_registration(mutate)
        with self.assertRaisesRegex(evidence.Invalid, "coreav_build_changed"):
            self.check_registration(drift=True)

    def test_registration_refuses_missing_or_unreviewed_record_references(self):
        for host in ("macos", "linux"):
            reg = registered()
            reg["coreav_builds"][host].pop("review_commit")
            with self.assertRaises(evidence.Invalid):
                registration.validate(reg)
            reg = registered()
            reg["coreav_builds"].pop(host)
            with self.assertRaises(evidence.Invalid):
                registration.validate(reg)
        reg = registered()
        with mock.patch.object(registration, "committed", side_effect=evidence.Invalid("uncommitted_registration")):
            with self.assertRaisesRegex(evidence.Invalid, "uncommitted_registration"):
                registration.coreav_build_records(reg, registration.ROOT)

    def test_registry_sources_are_checked_against_locked_checksums_and_every_file(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "lib.rs").write_bytes(b"synthetic")
            value = {"files": {"lib.rs": evidence.sha(b"synthetic")}, "package": h("locked-crate")}
            evidence.write(root / ".cargo-checksum.json", value)
            expected = build.registry_files(root, h("locked-crate"))
            self.assertEqual(expected, evidence.sha(build.canonical(value["files"])))
            with self.assertRaises(evidence.Invalid):
                build.registry_files(root, h("different-lock"))
            (root / "extra.rs").write_bytes(b"unlisted")
            with self.assertRaises(evidence.Invalid):
                build.registry_files(root, h("locked-crate"))
            (root / "extra.rs").unlink()
            (root / "lib.rs").write_bytes(b"changed")
            with self.assertRaises(evidence.Invalid):
                build.registry_files(root, h("locked-crate"))


class Capture(unittest.TestCase):
    def capture(self, failure=None):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root, cache, output = base / "checkout", base / "cargo", base / "output"
            root.mkdir()
            cache.mkdir()
            (root / build.PIN).parent.mkdir(parents=True)
            (root / build.PIN).write_bytes(sources()[build.PIN])
            commands = []
            statuses = 0

            def git(_root, *args):
                nonlocal statuses
                if args[0] == "status":
                    statuses += 1
                    return b"dirty" if failure == "source-drift" and statuses > 1 else b""
                return ((TREE if args[1].endswith("^{tree}") else CANDIDATE) + "\n").encode()

            def run(args, work, timeout, stdout, stderr, env, cwd):
                commands.append(args)
                self.assertNotIn("RUSTFLAGS", env)
                self.assertNotIn("RUSTC_WRAPPER", env)
                self.assertNotIn("CARGO_BUILD_RUSTFLAGS", env)
                data = b""
                if args[:3] == ["rustup", "toolchain", "list"]:
                    data = (PINS["RUST_DIST_TOOLCHAIN"] + "-aarch64-apple-darwin\n").encode()
                elif args[0] == "rustc":
                    data = record()["configuration"]["toolchain"]["rustc"].encode()
                elif args[0] == "cargo" and args[2] == "-vV":
                    data = record()["configuration"]["toolchain"]["cargo"].encode()
                elif args[0] == "uname":
                    data = {"-s": b"Darwin\n", "-m": b"arm64\n", "-r": b"test-1\n"}[args[1]]
                elif args[0] == "git":
                    archive = Path(next(arg.removeprefix("--output=") for arg in args if arg.startswith("--output=")))
                    with tarfile.open(archive, "w") as contents:
                        for name, raw in sources().items():
                            entry = tarfile.TarInfo(name)
                            entry.size = len(raw)
                            contents.addfile(entry, io.BytesIO(raw))
                elif args[2] == "metadata":
                    data = build.canonical({"packages": [{"name": "coreav-backend-apple", "version": "0.0.1",
                                                          "source": None, "manifest_path": str(cwd / build.MANIFEST)}],
                                            "resolve": {"nodes": []}})
                elif args[2] == "build":
                    if failure == "command":
                        raise evidence.Invalid("child_failure")
                    target = Path(args[args.index("--target-dir") + 1])
                    binary = target / build.TARGETS["macos"] / "dist/examples/videotoolbox"
                    binary.parent.mkdir(parents=True)
                    binary.write_bytes(b"synthetic-binary")
                    if failure == "snapshot-drift":
                        (cwd / "crates/coreav-backend-apple/examples/videotoolbox.rs").write_bytes(b"changed")
                else:
                    self.fail(str(args))
                (work / stdout).write_bytes(data)
                (work / stderr).write_bytes(b"")

            with mock.patch.object(build, "git", side_effect=git), mock.patch.object(build, "run", side_effect=run), \
                    mock.patch.dict(os.environ, {"RUSTFLAGS": "unregistered", "RUSTC_WRAPPER": "unregistered",
                                                 "CARGO_BUILD_RUSTFLAGS": "unregistered"}):
                if failure:
                    with self.assertRaises(evidence.Invalid):
                        build.capture(root, cache, output)
                    self.assertFalse(output.exists())
                else:
                    value = build.capture(root, cache, output)
                    self.assertEqual(value, evidence.load(output / "build.json"))
                    self.assertEqual(value["binary"]["sha256"], evidence.file_hash(output / "videotoolbox"))
                    self.assertEqual(value["source"]["commit"], CANDIDATE)
                    self.assertEqual(value["configuration"]["command"],
                                     build.command(PINS["RUST_DIST_TOOLCHAIN"], build.TARGETS["macos"]))
                    self.assertEqual(sum(args[0] == "cargo" and args[2] == "build" for args in commands), 1)
                    self.assertTrue(all("--offline" in args and "--frozen" in args and "--locked" in args
                                        for args in commands if args[0] == "cargo" and args[2] in ("metadata", "build")))

    def test_capture_observes_fixed_command_tools_and_binary_without_inherited_overrides(self):
        self.capture()

    def test_failed_build_or_changed_sources_cannot_publish_a_record(self):
        for failure in ("command", "source-drift", "snapshot-drift"):
            with self.subTest(failure=failure):
                self.capture(failure)

    def test_dirty_sources_and_user_configuration_stop_before_cargo(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "root"
            cache = Path(directory) / "cargo"
            root.mkdir()
            cache.mkdir()
            (root / build.PIN).parent.mkdir(parents=True)
            (root / build.PIN).write_bytes(sources()[build.PIN])
            for dirty in (True, False):
                if not dirty:
                    (cache / "config.toml").write_text('[build]\nrustflags = ["--cfg", "changed"]\n')
                with mock.patch.object(build, "git", side_effect=lambda _root, *args:
                                       b"dirty" if dirty and args[0] == "status" else
                                       b"" if args[0] == "status" else (CANDIDATE + "\n").encode()), \
                        mock.patch.object(build, "run") as run:
                    with self.assertRaises(evidence.Invalid):
                        build.capture(root, cache, Path(directory) / "output")
                    run.assert_not_called()

    def test_missing_prepared_toolchain_and_wrong_host_stop_before_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "root"
            cache = Path(directory) / "cargo"
            root.mkdir()
            cache.mkdir()
            (root / build.PIN).parent.mkdir(parents=True)
            (root / build.PIN).write_bytes(sources()[build.PIN])
            for wrong_host in (False, True):
                calls = []

                def run(args, work, timeout, stdout, stderr, env, cwd):
                    calls.append(args)
                    if args[0] == "rustup":
                        data = (PINS["RUST_DIST_TOOLCHAIN"] + "-test\n").encode() if wrong_host else b"stable-test\n"
                    elif args[0] == "rustc":
                        data = b"host: x86_64-pc-windows-msvc\n"
                    else:
                        data = b"cargo unsupported\n"
                    (work / stdout).write_bytes(data)
                    (work / stderr).write_bytes(b"")

                with mock.patch.object(build, "git", side_effect=lambda _root, *args:
                                       b"" if args[0] == "status" else (CANDIDATE + "\n").encode()), \
                        mock.patch.object(build, "run", side_effect=run):
                    with self.assertRaises(evidence.Invalid):
                        build.capture(root, cache, Path(directory) / "output")
                    self.assertFalse(any("build" in args for args in calls))
                    self.assertFalse((Path(directory) / "output").exists())


if __name__ == "__main__":
    unittest.main()
