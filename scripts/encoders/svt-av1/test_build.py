import contextlib
import hashlib
import io
import json
import os
from pathlib import Path
import signal
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest import mock

import build


def archive_bytes(entries=None):
    output = io.BytesIO()
    if entries is None:
        entries = [(build.SOURCE_ROOT + "/" + name, tarfile.REGTYPE,
                    ("notice " + name + "\n").encode()) for name in build.NOTICES]
    with tarfile.open(fileobj=output, mode="w:gz") as archive:
        for name, kind, content in entries:
            member = tarfile.TarInfo(name)
            member.type = kind
            member.size = len(content) if kind == tarfile.REGTYPE else 0
            member.linkname = "outside"
            archive.addfile(member, io.BytesIO(content))
    return output.getvalue()


class FakeRunner:
    def __init__(self, options, fail=None, version=None, signal_number=None):
        self.options = options
        self.target = build.TARGETS[options.target]
        self.fail = fail
        self.version = version if version is not None else "v" + build.SVT_VERSION + "\n"
        self.signal_number = signal_number
        self.calls = []
        self.stage = None
        self.on_verified = None
        self.program_headers = "Program Headers:\n LOAD\n"
        self.elf_header = "Machine: " + ("Advanced Micro Devices X86-64"
                                         if self.target.machine == "x86_64" else "AArch64") + "\n"
        self.dynamic = "There is no dynamic section in this file.\n"
        self.arches = self.target.machine + "\n"
        self.dependency = "/usr/lib/libSystem.B.dylib"
        self.omit = None

    def __call__(self, command, env):
        self.calls.append((command, dict(env)))
        tool = Path(command[0]).name
        phase = ""
        output = ""
        if command[1:] == ["--version"]:
            phase = "compiler"
            output = "fake native compiler 1\n"
        elif "-S" in command:
            phase = "configure"
            self.configure = command
            self.source = Path(command[command.index("-S") + 1])
        elif "--build" in command:
            phase = "build"
        elif "--install" in command:
            phase = "install"
            self.stage = Path(env["DESTDIR"]) / self.options.prefix.relative_to("/")
            for name in ("include/svt-av1/EbSvtAv1Enc.h", "include/svt-av1/extra.h",
                         "lib/libSvtAv1Enc.a"):
                if name != self.omit:
                    path = self.stage / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(("installed " + name).encode())
        elif "-o" in command:
            phase = "link"
            self.link = command
            self.probe_source = Path(next(arg for arg in command if arg.endswith(".c"))).read_text()
            self.probe = Path(command[command.index("-o") + 1])
            self.probe.write_bytes(b"fake linked executable")
            self.probe.chmod(0o755)
        elif tool == "svt-av1-version":
            phase = "version"
            output = self.version
        elif tool == "readelf":
            phase = "readelf"
            output = {"-h": self.elf_header, "-l": self.program_headers,
                      "-d": self.dynamic}[command[1]]
            if "-d" in command and self.on_verified:
                self.on_verified()
        elif tool == "lipo":
            phase = "lipo"
            output = self.arches
        elif tool == "otool":
            phase = "otool"
            output = str(self.probe) + ":\n\t" + self.dependency + " (compatibility version 1.0.0, current version 1.0.0)\n"
            if self.on_verified:
                self.on_verified()
        else:
            raise AssertionError(command)
        if phase == self.fail:
            if self.signal_number is not None:
                signal.raise_signal(self.signal_number)
            raise build.BuildError("injected " + phase + " failure")
        return output


class BuildTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.archive = self.root / "source.tar.gz"
        self.archive.write_bytes(archive_bytes())
        self.pin = mock.patch.object(build, "ARCHIVE_SHA256",
                                     hashlib.sha256(self.archive.read_bytes()).hexdigest())
        self.pin.start()
        self.addCleanup(self.pin.stop)

    def options(self, target="aarch64-apple-darwin", extra=()):
        return build.parse_args(["--target", target, "--prefix", str(self.root / "installed"),
                                 "--archive", str(self.archive), *extra])

    @contextlib.contextmanager
    def native(self, target):
        with mock.patch.object(build.platform, "system", return_value=target.system), \
                mock.patch.object(build.platform, "machine", return_value=target.machine), \
                mock.patch.object(build.shutil, "which",
                                  side_effect=lambda name: "/fake/bin/" + Path(name).name):
            yield

    def invoke(self, options, runner):
        with self.native(build.TARGETS[options.target]):
            return build.build(options, runner)

    def assert_clean(self, published=False):
        self.assertEqual(sorted(path.name for path in self.root.iterdir()),
                         ["installed", "source.tar.gz"] if published else ["source.tar.gz"])

    def test_all_native_target_builds_publish_verified_artifacts(self):
        expected = {
            "x86_64-unknown-linux-musl": ("Linux", "x86_64", "musl-gcc", ""),
            "aarch64-unknown-linux-musl": ("Linux", "aarch64", "musl-gcc", ""),
            "aarch64-apple-darwin": ("Darwin", "arm64", "clang", "11.0"),
            "x86_64-apple-darwin": ("Darwin", "x86_64", "clang", "10.12"),
        }
        self.assertEqual(set(build.TARGETS), set(expected))
        for triple, (system, machine, compiler, deployment) in expected.items():
            with self.subTest(target=triple):
                options = self.options(triple, ("--cmake", "custom-cmake", "--jobs", "3"))
                runner = FakeRunner(options)
                target = build.TARGETS[triple]
                self.assertEqual((target.system, target.machine, target.cc, target.deployment),
                                 (system, machine, compiler, deployment))
                with mock.patch.dict(os.environ, {"PATH": "/native/tools", "CC": "bad-cc",
                        "CFLAGS": "-march=native", "CXXFLAGS": "bad", "LDFLAGS": "bad",
                        "CMAKE_TOOLCHAIN_FILE": "bad", "SDKROOT": "bad", "CPATH": "bad",
                        "DYLD_INSERT_LIBRARIES": "bad", "SECRET": "not-for-children"}):
                    self.assertEqual(self.invoke(options, runner), options.prefix)
                self.assert_clean(published=True)
                self.assertEqual(runner.configure[0], "/fake/bin/custom-cmake")
                required_flags = {
                    "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=OFF", "-DBUILD_APPS=OFF",
                    "-DBUILD_TESTING=OFF", "-DNATIVE=OFF", "-DSVT_AV1_LTO=ON",
                    "-DCMAKE_INTERPROCEDURAL_OPTIMIZATION=ON",
                    "-DCMAKE_POSITION_INDEPENDENT_CODE=ON", "-DCMAKE_INSTALL_LIBDIR=lib",
                    "-DCMAKE_C_FLAGS=-O3 -DNDEBUG -ffunction-sections -fdata-sections",
                    "-DCMAKE_C_COMPILER=/fake/bin/" + compiler,
                    "-DCMAKE_INSTALL_PREFIX=" + str(options.prefix),
                }
                self.assertTrue(required_flags.issubset(runner.configure))
                build_command = next(cmd for cmd, env in runner.calls if "--build" in cmd)
                self.assertEqual(build_command[-2:], ["--parallel", "3"])
                self.assertIn("-pthread", runner.link)
                self.assertIn("-lm", runner.link)
                self.assertIn(str(runner.stage / "lib/libSvtAv1Enc.a"), runner.link)
                self.assertIn("-I" + str(runner.stage / "include/svt-av1"), runner.link)
                self.assertIn("#include <EbSvtAv1Enc.h>", runner.probe_source)
                self.assertIn("svt_av1_get_version()", runner.probe_source)
                self.assertIn("(*volatile roots[])(void)", runner.probe_source)
                for name in ("svt_av1_enc_init_handle", "svt_av1_enc_set_parameter",
                             "svt_av1_enc_init", "svt_av1_enc_send_picture",
                             "svt_av1_enc_get_packet", "svt_av1_enc_release_out_buffer",
                             "svt_av1_enc_deinit", "svt_av1_enc_deinit_handle"):
                    self.assertIn("(void (*)(void))" + name + ",", runner.probe_source)
                    self.assertNotIn(name + "(", runner.probe_source)
                self.assertIn("if (!roots[i]) return 1", runner.probe_source)
                if system == "Linux":
                    self.assertIn("-static", runner.link)
                    self.assertEqual([cmd[1] for cmd, env in runner.calls
                                      if Path(cmd[0]).name == "readelf"], ["-h", "-l", "-d"])
                    self.assertIn("-DCMAKE_CXX_COMPILER=/fake/bin/c++", runner.configure)
                    self.assertIn("-DCMAKE_C_COMPILER_AR=/fake/bin/gcc-ar", runner.configure)
                    self.assertIn("-DCMAKE_C_COMPILER_RANLIB=/fake/bin/gcc-ranlib", runner.configure)
                else:
                    self.assertNotIn("-static", runner.link)
                    self.assertIn("-DCMAKE_OSX_ARCHITECTURES=" + machine, runner.configure)
                    self.assertIn("-DCMAKE_OSX_DEPLOYMENT_TARGET=" + deployment, runner.configure)
                    self.assertEqual(runner.link[runner.link.index("-arch") + 1], machine)
                    self.assertIn("-mmacosx-version-min=" + deployment, runner.link)
                for cmd, env in runner.calls:
                    self.assertEqual(env["PATH"], "/native/tools")
                    self.assertLessEqual(set(env), {"PATH", "LC_ALL", "LANG", "TMPDIR",
                                                   "ZERO_AR_DATE", "SOURCE_DATE_EPOCH", "DESTDIR"})
                    self.assertEqual("DESTDIR" in env, "--install" in cmd)
                manifest = json.loads((options.prefix / "manifest.json").read_text())
                self.assertEqual(manifest["target"], triple)
                self.assertEqual(manifest["version"], build.SVT_VERSION)
                self.assertEqual(manifest["archive_sha256"], build.ARCHIVE_SHA256)
                self.assertEqual(manifest["compiler_version"], "fake native compiler 1")
                self.assertNotIn(".svt-av1-", json.dumps(manifest))
                if system == "Linux":
                    self.assertIn("-DCMAKE_C_COMPILER_AR=gcc-ar", manifest["configure_args"])
                    self.assertIn("-DCMAKE_C_COMPILER_RANLIB=gcc-ranlib", manifest["configure_args"])
                self.assertEqual(len(manifest["sha256"]), 9)
                for name, digest in manifest["sha256"].items():
                    self.assertEqual(hashlib.sha256((options.prefix / name).read_bytes()).hexdigest(), digest)
                for original, name in build.NOTICES.items():
                    self.assertEqual((options.prefix / "share/licenses/svt-av1" / name).read_bytes(),
                                     ("notice " + original + "\n").encode())
                self.assertTrue(os.access(options.prefix / "bin/svt-av1-version", os.X_OK))
                shutil.rmtree(options.prefix)

    def test_host_refusal_precedes_tools_network_and_output(self):
        for triple in build.TARGETS:
            options = self.options(triple)
            options.prefix = self.root / "absent-parent" / "installed"
            with self.subTest(target=triple), \
                    mock.patch.object(build.platform, "system", return_value="Windows"), \
                    mock.patch.object(build.platform, "machine", return_value="other"), \
                    mock.patch.object(build.shutil, "which") as which, \
                    mock.patch.object(build.urllib.request, "urlopen") as download:
                with self.assertRaises(build.BuildError):
                    build.build(options, mock.Mock(side_effect=AssertionError))
                which.assert_not_called()
                download.assert_not_called()
                self.assert_clean()

    def test_architecture_mismatch_is_refused(self):
        for triple, target in build.TARGETS.items():
            with self.subTest(target=triple), \
                    mock.patch.object(build.platform, "system", return_value=target.system), \
                    mock.patch.object(build.platform, "machine", return_value="wrong"), \
                    mock.patch.object(build.shutil, "which") as which:
                with self.assertRaises(build.BuildError):
                    build.build(self.options(triple), mock.Mock(side_effect=AssertionError))
                which.assert_not_called()
                self.assert_clean()

    def test_existing_prefix_and_relative_prefix_are_refused_before_acquisition(self):
        options = self.options()
        for kind in ("directory", "file", "symlink", "relative"):
            with self.subTest(kind=kind), self.native(build.TARGETS[options.target]), \
                    mock.patch.object(build.urllib.request, "urlopen") as download:
                options.prefix = self.root / "installed"
                if kind == "directory":
                    options.prefix.mkdir()
                elif kind == "file":
                    options.prefix.write_bytes(b"preserve")
                elif kind == "symlink":
                    options.prefix.symlink_to(self.root / "absent")
                else:
                    options.prefix = Path("relative")
                with self.assertRaises(build.BuildError):
                    build.build(options, mock.Mock(side_effect=AssertionError))
                download.assert_not_called()
                if kind == "directory":
                    options.prefix.rmdir()
                elif kind != "relative":
                    if kind == "file":
                        self.assertEqual(options.prefix.read_bytes(), b"preserve")
                    options.prefix.unlink()
                self.assert_clean()

    def test_cli_bounds_and_defaults(self):
        options = self.options()
        self.assertEqual(options.jobs, 2)
        self.assertEqual(options.cmake, "cmake")
        for jobs in ("0", "17", "-1", "1.5"):
            with self.subTest(jobs=jobs), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit):
                    self.options(extra=("--jobs", jobs))
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            self.options("x86_64-unknown-linux-gnu")

    def test_archive_hash_is_verified_before_tar_parsing(self):
        self.archive.write_bytes(b"not the pinned archive")
        with mock.patch.object(build.tarfile, "open", side_effect=AssertionError), \
                self.assertRaises(build.BuildError):
            self.invoke(self.options(), mock.Mock(side_effect=AssertionError))
        self.assert_clean()

    def test_unsafe_archive_entries_never_reach_a_tool(self):
        invalid = []
        invalid.extend([[(name, tarfile.REGTYPE, b"x")] for name in (
            "elsewhere/file", build.SOURCE_ROOT + "/../escape",
            "/" + build.SOURCE_ROOT + "/file", build.SOURCE_ROOT + "//file")])
        invalid.extend([[(build.SOURCE_ROOT + "/special", kind, b"")]
                        for kind in (tarfile.SYMTYPE, tarfile.LNKTYPE, tarfile.CHRTYPE,
                                     tarfile.BLKTYPE, tarfile.FIFOTYPE, tarfile.GNUTYPE_SPARSE)])
        invalid.append([(build.SOURCE_ROOT + "/duplicate", tarfile.REGTYPE, b"x")] * 2)
        for entries in invalid:
            with self.subTest(entries=entries):
                self.archive.write_bytes(archive_bytes(entries))
                with mock.patch.object(build, "ARCHIVE_SHA256", build.sha256(self.archive)):
                    with self.assertRaises((build.BuildError, tarfile.TarError)):
                        self.invoke(self.options(), mock.Mock(side_effect=AssertionError))
                self.assert_clean()

    def test_download_uses_only_fixed_url_and_verifies_it(self):
        options = self.options()
        options.archive = None
        with mock.patch.object(build.urllib.request, "urlopen",
                               return_value=io.BytesIO(self.archive.read_bytes())) as download:
            self.invoke(options, FakeRunner(options))
        download.assert_called_once_with(build.ARCHIVE_URL, timeout=60)
        self.assert_clean(published=True)

    def test_each_tool_failure_cleans_all_transients_without_publishing(self):
        for triple in ("aarch64-apple-darwin", "x86_64-unknown-linux-musl"):
            phases = ("compiler", "configure", "build", "install", "link", "version")
            phases += ("readelf",) if "linux" in triple else ("lipo", "otool")
            for phase in phases:
                with self.subTest(target=triple, phase=phase):
                    options = self.options(triple)
                    with self.assertRaises(build.BuildError):
                        self.invoke(options, FakeRunner(options, fail=phase))
                    self.assert_clean()

    def test_missing_install_outputs_are_not_published(self):
        for omitted in ("include/svt-av1/EbSvtAv1Enc.h", "lib/libSvtAv1Enc.a"):
            options = self.options()
            runner = FakeRunner(options)
            runner.omit = omitted
            with self.subTest(omitted=omitted), self.assertRaises(build.BuildError):
                self.invoke(options, runner)
            self.assert_clean()

    def test_wrong_version_and_dynamic_or_universal_probes_are_not_published(self):
        cases = [("version", value) for value in ("", "4.2.1\n", " " + build.SVT_VERSION,
                                                   build.SVT_VERSION + "\nextra", build.SVT_VERSION + "\n\n")]
        cases += [("elf_header", "Machine: AArch64\n"), ("elf_header", ""),
                  ("program_headers", "LOAD INTERP"), ("program_headers", ""),
                  ("dynamic", "(NEEDED) libc.so.6"), ("arches", "arm64 x86_64"),
                  ("arches", "x86_64"), ("dependency", "@rpath/libextra.dylib"),
                  ("dependency", "/usr/local/lib/libextra.dylib"),
                  ("dependency", "/usr/lib/../../tmp/libextra.dylib")]
        for field, value in cases:
            triple = "x86_64-unknown-linux-musl" if field in ("elf_header", "program_headers", "dynamic") \
                else "aarch64-apple-darwin"
            options = self.options(triple)
            runner = FakeRunner(options)
            setattr(runner, field, value)
            with self.subTest(field=field, value=value), self.assertRaises(build.BuildError):
                self.invoke(options, runner)
            self.assert_clean()

    def test_plain_version_is_accepted(self):
        options = self.options()
        self.invoke(options, FakeRunner(options, version=build.SVT_VERSION + "\n"))
        self.assert_clean(published=True)

    def test_signals_clean_transients_and_restore_handlers(self):
        for number in (signal.SIGHUP, signal.SIGINT, signal.SIGQUIT, signal.SIGTERM):
            previous = signal.getsignal(number)
            options = self.options()
            runner = FakeRunner(options, fail="install", signal_number=number)
            with self.subTest(signal=number), self.assertRaises(build.BuildError):
                self.invoke(options, runner)
            self.assertEqual(signal.getsignal(number), previous)
            self.assert_clean()

    def test_publish_refuses_even_an_empty_concurrently_created_prefix(self):
        options = self.options()
        runner = FakeRunner(options)
        runner.on_verified = options.prefix.mkdir
        with self.assertRaises(build.BuildError):
            self.invoke(options, runner)
        self.assertEqual(list(options.prefix.iterdir()), [])
        self.assert_clean(published=True)

    def test_missing_compiler_never_falls_back(self):
        options = self.options("x86_64-unknown-linux-musl")
        with self.native(build.TARGETS[options.target]), \
                mock.patch.object(build.shutil, "which", side_effect=lambda tool:
                                  None if tool == "musl-gcc" else "/fake/bin/" + tool) as which:
            with self.assertRaises(build.BuildError):
                build.build(options, mock.Mock(side_effect=AssertionError))
        self.assertEqual(which.call_args_list, [mock.call("cmake"), mock.call("musl-gcc")])
        self.assert_clean()

    def test_missing_lto_archive_tools_fail_before_acquisition(self):
        for triple in ("x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"):
            for missing in ("gcc-ar", "gcc-ranlib"):
                with self.subTest(target=triple, missing=missing), \
                        self.native(build.TARGETS[triple]), \
                        mock.patch.object(build.shutil, "which", side_effect=lambda tool:
                                          None if tool == missing else "/fake/bin/" + tool), \
                        mock.patch.object(build.urllib.request, "urlopen") as download:
                    with self.assertRaisesRegex(build.BuildError, missing):
                        build.build(self.options(triple), mock.Mock(side_effect=AssertionError))
                    download.assert_not_called()
                    self.assert_clean()

    def test_subprocess_runner_kills_and_reaps_its_group_on_cancellation(self):
        process = mock.Mock(pid=1234)
        process.communicate.side_effect = [KeyboardInterrupt, subprocess.TimeoutExpired("fake", 5),
                                           ("", "")]
        with mock.patch.object(build.subprocess, "Popen", return_value=process), \
                mock.patch.object(build.os, "killpg") as kill:
            with self.assertRaises(KeyboardInterrupt):
                build.run(["fake"], {})
        self.assertEqual(kill.call_args_list, [mock.call(1234, signal.SIGTERM),
                                              mock.call(1234, signal.SIGKILL)])
        self.assertEqual(process.communicate.call_count, 3)
        self.assertEqual(process.communicate.call_args_list[0],
                         mock.call(timeout=build.RUN_TIMEOUT))

    def test_subprocess_timeout_kills_reaps_and_reports_a_build_error(self):
        process = mock.Mock(pid=1234)
        process.communicate.side_effect = [subprocess.TimeoutExpired("fake", 7), ("", "")]
        with mock.patch.object(build.subprocess, "Popen", return_value=process), \
                mock.patch.object(build.os, "killpg") as kill:
            with self.assertRaisesRegex(build.BuildError, "timed out after 7s"):
                build.run(["fake"], {}, timeout=7)
        kill.assert_called_once_with(1234, signal.SIGTERM)
        self.assertEqual(process.communicate.call_args_list,
                         [mock.call(timeout=7), mock.call(timeout=5)])

    def test_real_subprocess_timeout_leaves_no_child(self):
        with mock.patch.object(build.os, "killpg", wraps=os.killpg) as kill:
            with self.assertRaisesRegex(build.BuildError, "command timed out"):
                build.run([sys.executable, "-c", "import signal; signal.pause()"],
                          {}, timeout=0.1)
        pid = kill.call_args_list[0][0][0]
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)


if __name__ == "__main__":
    unittest.main()
