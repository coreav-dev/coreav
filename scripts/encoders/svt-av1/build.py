#!/usr/bin/env python3
"""Build and verify a pinned, native static SVT-AV1 installation.

The destination must not be created or modified concurrently by another writer.
"""

import argparse
import contextlib
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import urllib.request


SVT_VERSION = "4.2.0"
ARCHIVE_URL = (
    "https://gitlab.com/AOMediaCodec/SVT-AV1/-/archive/"
    f"v{SVT_VERSION}/SVT-AV1-v{SVT_VERSION}.tar.gz"
)
ARCHIVE_SHA256 = "c7b13c4a84bd3751aa35fcc72be13e6875467e7c2216879251a486e5b1e4e740"
SOURCE_ROOT = f"SVT-AV1-v{SVT_VERSION}"
C_FLAGS = "-O3 -DNDEBUG -ffunction-sections -fdata-sections"
NOTICES = {
    "LICENSE.md": "LICENSE.md",
    "LICENSE-BSD2.md": "LICENSE-BSD2.md",
    "PATENTS.md": "PATENTS.md",
    "third_party/fastfeat/LICENSE": "fastfeat-LICENSE",
    "third_party/safestringlib/LICENSE": "safestringlib-LICENSE",
}


@dataclass(frozen=True)
class Target:
    system: str
    machine: str
    cc: str
    cxx: str
    deployment: str = ""
    elf_machine: str = ""


TARGETS = {
    "x86_64-unknown-linux-musl": Target("Linux", "x86_64", "musl-gcc", "c++",
                                          elf_machine="Advanced Micro Devices X86-64"),
    "aarch64-unknown-linux-musl": Target("Linux", "aarch64", "musl-gcc", "c++",
                                           elf_machine="AArch64"),
    "aarch64-apple-darwin": Target("Darwin", "arm64", "clang", "clang++", "11.0"),
    "x86_64-apple-darwin": Target("Darwin", "x86_64", "clang", "clang++", "10.12"),
}


class BuildError(RuntimeError):
    pass


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--prefix", type=Path, required=True,
                        help="absolute installation directory, which must not exist")
    parser.add_argument("--archive", type=Path, help="local pinned source archive")
    parser.add_argument("--jobs", type=int, choices=range(1, 17), default=2,
                        metavar="1..16")
    parser.add_argument("--cmake", default="cmake", help="CMake executable or path")
    return parser.parse_args(argv)


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def extract(archive, destination):
    if sha256(archive) != ARCHIVE_SHA256:
        raise BuildError("source archive SHA-256 does not match the pin")
    with tarfile.open(archive, "r:gz") as source:
        members = source.getmembers()
        seen = set()
        for member in members:
            path = PurePosixPath(member.name)
            if (not path.parts or path.parts[0] != SOURCE_ROOT
                    or path.is_absolute() or ".." in path.parts
                    or member.name.rstrip("/") != path.as_posix()
                    or not (member.type == tarfile.REGTYPE or member.isdir())
                    or path in seen):
                raise BuildError("source archive contains an unsafe or unexpected entry")
            seen.add(path)
        for member in members:
            output = destination.joinpath(*PurePosixPath(member.name).parts)
            if member.isdir():
                output.mkdir(parents=True, exist_ok=True)
            else:
                output.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as content, output.open("xb") as target:
                    shutil.copyfileobj(content, target)
                output.chmod(member.mode & 0o777)
    root = destination / SOURCE_ROOT
    if not root.is_dir():
        raise BuildError("source archive has no expected source directory")
    return root


def run(command, env):
    process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, start_new_session=True)
    try:
        stdout, stderr = process.communicate()
    except BaseException:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate()
        raise
    if process.returncode:
        raise BuildError(f"command failed ({process.returncode}): {command[0]}\n{stderr}")
    return stdout


@contextlib.contextmanager
def cancellation():
    def cancel(signum, frame):
        raise BuildError(f"build cancelled by signal {signum}")

    previous = {}
    try:
        for signum in (signal.SIGHUP, signal.SIGINT, signal.SIGQUIT, signal.SIGTERM):
            previous[signum] = signal.signal(signum, cancel)
        yield
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


def publish(stage, prefix):
    if os.path.lexists(prefix):
        raise BuildError("--prefix was created during the build; refusing publication")
    stage.rename(prefix)


def build(options, runner=run):
    target = TARGETS[options.target]
    if (platform.system(), platform.machine()) != (target.system, target.machine):
        raise BuildError("target requires its exact native host OS and architecture")
    if not options.prefix.is_absolute():
        raise BuildError("--prefix must be absolute")
    if os.path.lexists(options.prefix):
        raise BuildError("--prefix must not exist")
    prefix = options.prefix.resolve()
    if os.path.lexists(prefix):
        raise BuildError("--prefix must not exist")
    if not prefix.parent.is_dir():
        raise BuildError("--prefix parent directory must exist")
    if not 1 <= options.jobs <= 16:
        raise BuildError("--jobs must be between 1 and 16")
    tools = [options.cmake, target.cc, target.cxx, "make"]
    tools += ["readelf"] if target.system == "Linux" else ["lipo", "otool"]
    if target.machine == "x86_64":
        tools.append("nasm")
    resolved = {}
    for tool in tools:
        found = shutil.which(tool)
        if found is None:
            raise BuildError(f"required tool not found: {tool}")
        resolved[tool] = str(Path(found).resolve())
    cmake, cc, cxx = (resolved[name] for name in (options.cmake, target.cc, target.cxx))
    with cancellation(), tempfile.TemporaryDirectory(
            prefix=".svt-av1-", dir=prefix.parent) as temporary:
        work = Path(temporary)
        scratch = work / "tmp"
        scratch.mkdir()
        env = {"PATH": os.environ.get("PATH", os.defpath), "LC_ALL": "C", "LANG": "C",
               "TMPDIR": str(scratch), "ZERO_AR_DATE": "1", "SOURCE_DATE_EPOCH": "0"}
        archive = work / "source.tar.gz"
        if options.archive is not None:
            with options.archive.open("rb") as source, archive.open("xb") as output:
                shutil.copyfileobj(source, output)
        else:
            with urllib.request.urlopen(ARCHIVE_URL, timeout=60) as source:
                with archive.open("xb") as output:
                    shutil.copyfileobj(source, output)
        source = extract(archive, work / "source")
        build_dir = work / "build"
        destdir = work / "stage"
        stage = destdir / prefix.relative_to(prefix.anchor)
        compiler_version = runner([cc, "--version"], env).strip()
        flags = [
            "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_SHARED_LIBS=OFF", "-DBUILD_APPS=OFF",
            "-DBUILD_TESTING=OFF", "-DNATIVE=OFF", "-DSVT_AV1_LTO=ON",
            "-DCMAKE_INTERPROCEDURAL_OPTIMIZATION=ON", "-DCMAKE_POSITION_INDEPENDENT_CODE=ON",
            f"-DCMAKE_C_COMPILER={cc}", f"-DCMAKE_CXX_COMPILER={cxx}",
            f"-DCMAKE_C_FLAGS={C_FLAGS}", f"-DCMAKE_CXX_FLAGS={C_FLAGS}",
            "-DCMAKE_C_FLAGS_RELEASE=", "-DCMAKE_CXX_FLAGS_RELEASE=",
            "-DCMAKE_EXE_LINKER_FLAGS=", "-DCMAKE_SHARED_LINKER_FLAGS=",
            "-DCMAKE_MODULE_LINKER_FLAGS=", "-DCMAKE_INSTALL_LIBDIR=lib",
            "-DCMAKE_INSTALL_INCLUDEDIR=include", "-DCMAKE_INSTALL_BINDIR=bin",
            f"-DCMAKE_INSTALL_PREFIX={prefix}",
            "-DCMAKE_FIND_USE_PACKAGE_REGISTRY=OFF",
            "-DCMAKE_FIND_USE_SYSTEM_PACKAGE_REGISTRY=OFF",
        ]
        platform_flags = []
        if target.system == "Darwin":
            flags += [f"-DCMAKE_OSX_ARCHITECTURES={target.machine}",
                      f"-DCMAKE_OSX_DEPLOYMENT_TARGET={target.deployment}"]
            platform_flags = ["-arch", target.machine,
                              f"-mmacosx-version-min={target.deployment}"]
        runner([cmake, "-S", str(source), "-B", str(build_dir),
                "-G", "Unix Makefiles", *flags], env)
        runner([cmake, "--build", str(build_dir), "--config", "Release",
                "--parallel", str(options.jobs)], env)
        runner([cmake, "--install", str(build_dir), "--config", "Release"],
               dict(env, DESTDIR=str(destdir)))
        header = stage / "include/svt-av1/EbSvtAv1Enc.h"
        library = stage / "lib/libSvtAv1Enc.a"
        for required in (header, library):
            if not required.is_file() or required.is_symlink():
                raise BuildError(f"installation is missing a regular file: {required.name}")
        notices = stage / "share/licenses/svt-av1"
        notices.mkdir(parents=True, exist_ok=True)
        for original, name in NOTICES.items():
            shutil.copyfile(source / original, notices / name)
        probe_source = work / "version.c"
        probe_source.write_text(
            "#include <EbSvtAv1Enc.h>\n#include <stdio.h>\n"
            "static void (*volatile roots[])(void) = {\n"
            + "".join("    (void (*)(void))" + name + ",\n" for name in (
                "svt_av1_enc_init_handle", "svt_av1_enc_set_parameter", "svt_av1_enc_init",
                "svt_av1_enc_send_picture", "svt_av1_enc_get_packet",
                "svt_av1_enc_release_out_buffer", "svt_av1_enc_deinit", "svt_av1_enc_deinit_handle"))
            + "};\nint main(void){\n"
            "    for (size_t i=0; i<sizeof(roots)/sizeof(roots[0]); ++i)\n"
            "        if (!roots[i]) return 1;\n"
            "    const char *v=svt_av1_get_version(); return v?puts(v)<0:1;\n}\n",
            encoding="utf-8")
        probe = stage / "bin/svt-av1-version"
        probe.parent.mkdir(parents=True, exist_ok=True)
        runner([cc, *C_FLAGS.split(), *platform_flags,
                *(["-static"] if target.system == "Linux" else []),
                str(probe_source), "-I" + str(header.parent), str(library),
                "-pthread", "-lm", "-o", str(probe)], env)
        version = runner([str(probe)], env).removesuffix("\n")
        if version not in (SVT_VERSION, "v" + SVT_VERSION):
            raise BuildError("linked probe did not report the exact pinned version")
        if target.system == "Linux":
            elf_header = runner([resolved["readelf"], "-h", str(probe)], env)
            if not re.search(r"^\s*Machine:\s+" + re.escape(target.elf_machine) + r"\s*$",
                             elf_header, re.MULTILINE):
                raise BuildError("linked probe has the wrong ELF machine")
            program = runner([resolved["readelf"], "-l", str(probe)], env)
            dynamic = runner([resolved["readelf"], "-d", str(probe)], env)
            if not re.search(r"\bLOAD\b", program) or re.search(r"\bINTERP\b", program) \
                    or re.search(r"\bNEEDED\b", dynamic):
                raise BuildError("linked probe is not a static Linux executable")
        else:
            arches = runner([resolved["lipo"], "-archs", str(probe)], env).split()
            if arches != [target.machine]:
                raise BuildError("linked probe has the wrong or multiple architectures")
            listing = runner([resolved["otool"], "-L", str(probe)], env).splitlines()
            if not listing or listing[0] != str(probe) + ":" or len(listing) < 2:
                raise BuildError("linked probe has no valid dylib dependency listing")
            for line in listing[1:]:
                dependency = line.strip().split(" (", 1)[0]
                if (not dependency.startswith(("/usr/lib/", "/System/Library/"))
                        or ".." in PurePosixPath(dependency).parts):
                    raise BuildError("linked probe depends on a non-system dylib")
        retained = [library, probe, *sorted(header.parent.rglob("*.h")),
                    *sorted(notices.iterdir())]
        manifest_flags = [flag.replace(str(prefix), "<prefix>")
                          .replace(cc, target.cc).replace(cxx, target.cxx) for flag in flags]
        manifest = {
            "target": options.target, "version": SVT_VERSION, "archive_sha256": ARCHIVE_SHA256,
            "configure_args": ["-G", "Unix Makefiles", *manifest_flags],
            "compiler_version": compiler_version,
            "sha256": {str(path.relative_to(stage)): sha256(path) for path in retained},
        }
        (stage / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True)
                                             + "\n", encoding="utf-8")
        publish(stage, prefix)
    return prefix


def main(argv=None):
    try:
        prefix = build(parse_args(argv))
    except (BuildError, OSError, tarfile.TarError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(prefix)
    return 0


if __name__ == "__main__":
    sys.exit(main())
