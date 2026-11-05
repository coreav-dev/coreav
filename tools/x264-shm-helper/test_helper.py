#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
import argparse
from decimal import Decimal
import errno
import fcntl
import os
from pathlib import Path
import random
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest


HELPER = None
X264 = None
SEALS = fcntl.F_SEAL_WRITE | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_SEAL


def arguments(csp="i420", depth=8):
    return [
        "--demuxer", "raw", "--input-csp", csp, "--output-csp", csp,
        "--input-depth", str(depth), "--output-depth", str(depth),
        "--input-res", "32x32", "--fps", "30000/1001", "--preset", "veryfast",
        "--crf", "23", "--threads", "1", "--min-keyint", "1",
        "--log-level", "error", "--no-progress", "--muxer", "raw", "--output", "-", "-",
    ]


def extended(args):
    return args + ["--coreav-shm", "--coreav-timebase", "1001/30000", "--coreav-vfr", "0"]


def timecode_arguments(path, program):
    args = arguments()
    index = args.index("--fps")
    args[index:index + 2] = ["--tcfile-in", str(path)]
    return args + [
        "--coreav-shm", "--coreav-timebase", "1/1000000000", "--coreav-vfr", "1",
        "--coreav-clock-program", str(program),
    ]


def fake_clock(directory, body):
    path = Path(directory) / "clock"
    path.write_text(f"#!{sys.executable}\n" + body)
    path.chmod(0o700)
    return path


def replace(args, name, value):
    result = list(args)
    result[result.index(name) + 1] = value
    return result


def pixels(csp="i420", depth=8, frame=0):
    chroma = {"i420": 256, "i422": 512, "i444": 1024}[csp]
    samples = [(i * 7 + frame * 11) % (1 << depth) for i in range(1024 + 2 * chroma)]
    return bytes(samples) if depth == 8 else struct.pack(f"<{len(samples)}H", *samples)


def snapshot(data, seals=SEALS):
    fd = os.memfd_create("coreav-helper-test", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
    try:
        os.write(fd, data)
        fcntl.fcntl(fd, fcntl.F_ADD_SEALS, seals)
        return fd
    except BaseException:
        os.close(fd)
        raise


def frame(fd, size, sequence=1, pts=0, path=None):
    path = path if path is not None else f"/proc/{os.getpid()}/fd/{fd}".encode()
    return b"\x01" + struct.pack("<QqQH", sequence, pts, size, len(path)) + path


def messages(data):
    if data[:8] != b"CAVOUT01":
        raise AssertionError("missing output magic")
    offset = 8
    acks = []
    encoded = bytearray()
    while offset < len(data):
        tag = data[offset]
        offset += 1
        if tag == 1:
            if offset + 8 > len(data):
                raise AssertionError("truncated acknowledgement")
            acks.append(struct.unpack_from("<Q", data, offset)[0])
            offset += 8
        elif tag == 2:
            if offset + 4 > len(data):
                raise AssertionError("truncated data length")
            length = struct.unpack_from("<I", data, offset)[0]
            offset += 4
            if not 1 <= length <= 1048576 or offset + length > len(data):
                raise AssertionError("invalid data length")
            encoded.extend(data[offset:offset + length])
            offset += length
        else:
            raise AssertionError("unknown output tag")
    return acks, bytes(encoded)


class HelperTests(unittest.TestCase):
    def invoke(self, wire, args=None, pass_fds=()):
        return subprocess.run(
            [HELPER, *(extended(arguments()) if args is None else args)],
            input=wire, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=15,
            pass_fds=pass_fds,
        )

    def rejected(self, wire, args=None, diagnostic=None):
        result = self.invoke(wire, args)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"coreav-x264-shm:", result.stderr)
        if diagnostic is not None:
            self.assertIn(diagnostic, result.stderr)
        if result.stdout:
            acks, _ = messages(result.stdout)
            self.assertEqual(acks, [])

    def test_empty_flush(self):
        result = self.invoke(b"CAVSHM01")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(messages(result.stdout), ([], b""))

    def test_all_formats_and_depths_flush_delayed_frames(self):
        for csp in ("i420", "i422", "i444"):
            for depth in (8, 10):
                with self.subTest(csp=csp, depth=depth):
                    fds = [snapshot(pixels(csp, depth, i)) for i in range(6)]
                    try:
                        wire = b"CAVSHM01" + b"".join(
                            frame(fd, len(pixels(csp, depth)), i + 1, i)
                            for i, fd in enumerate(fds)
                        )
                        result = self.invoke(wire, extended(arguments(csp, depth)))
                        self.assertEqual(result.returncode, 0, result.stderr)
                        acks, data = messages(result.stdout)
                        self.assertEqual(acks, list(range(1, 7)))
                        self.assertTrue(data.startswith(b"\0\0\0\x01"))
                        self.assertGreater(len(data), 100)
                    finally:
                        for fd in fds:
                            os.close(fd)

    def test_ack_releases_snapshot_before_eof(self):
        fd = snapshot(pixels())
        process = subprocess.Popen(
            [HELPER, *extended(arguments())], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        def read_exact(length):
            output = bytearray()
            while len(output) < length:
                ready, _, _ = select.select([process.stdout], [], [], 10)
                self.assertTrue(ready, "helper did not acknowledge before EOF")
                part = os.read(process.stdout.fileno(), length - len(output))
                self.assertTrue(part, "helper closed before acknowledgement")
                output.extend(part)
            return bytes(output)
        try:
            process.stdin.write(b"CAVSHM01" + frame(fd, len(pixels())))
            process.stdin.flush()
            self.assertEqual(read_exact(8), b"CAVOUT01")
            while True:
                tag = read_exact(1)
                if tag == b"\x01":
                    self.assertEqual(read_exact(8), struct.pack("<Q", 1))
                    break
                self.assertEqual(tag, b"\x02")
                length = struct.unpack("<I", read_exact(4))[0]
                self.assertTrue(1 <= length <= 1048576)
                read_exact(length)
            with self.assertRaises(OSError) as error:
                os.pwrite(fd, b"x", 0)
            self.assertEqual(error.exception.errno, errno.EPERM)
            os.close(fd)
            fd = None
            output, diagnostic = process.communicate(timeout=15)
            self.assertEqual(process.returncode, 0, diagnostic)
            acks, data = messages(b"CAVOUT01" + output)
            self.assertEqual(acks, [])
            self.assertGreater(len(data), 100)
        finally:
            if fd is not None:
                os.close(fd)
            if process.poll() is None:
                process.kill()
                process.communicate()

    def test_unsealed_or_incompletely_sealed_snapshots_refused(self):
        for seals in (0, *(SEALS & ~seal for seal in (
            fcntl.F_SEAL_WRITE, fcntl.F_SEAL_GROW, fcntl.F_SEAL_SHRINK, fcntl.F_SEAL_SEAL,
        ))):
            with self.subTest(seals=seals):
                fd = snapshot(pixels(), seals)
                try:
                    self.rejected(b"CAVSHM01" + frame(fd, len(pixels())), diagnostic=b"seals")
                finally:
                    os.close(fd)

    def test_backing_size_must_be_exact(self):
        for delta in (-1, 1):
            fd = snapshot(b"x" * (len(pixels()) + delta))
            try:
                self.rejected(b"CAVSHM01" + frame(fd, len(pixels())), diagnostic=b"exact size")
            finally:
                os.close(fd)

    def test_bad_wire_fields(self):
        fd = snapshot(pixels())
        try:
            for sequence, size in ((0, len(pixels())), (2, len(pixels())), (1, 1), (1, 2**64 - 1)):
                self.rejected(b"CAVSHM01" + frame(fd, size, sequence))
            for length in (0, 129, 65535):
                self.rejected(b"CAVSHM01\x01" + struct.pack("<QqQH", 1, 0, len(pixels()), length))
        finally:
            os.close(fd)

    def test_bad_paths(self):
        fd = snapshot(pixels())
        try:
            for path in (
                b"/tmp/frame", b"/proc/self/fd/1", b"/proc/0/fd/0", b"/proc/1/fd/-1",
                b"/proc/1/fd/1/trailing", b"/proc/1/fd/1\0", b"/proc/1/fd/1\xff",
                b"/proc/999999999999999999/fd/1", b"/proc/1/fd/999999999999999999",
            ):
                with self.subTest(path=path):
                    self.rejected(b"CAVSHM01" + frame(fd, len(pixels()), path=path))
        finally:
            os.close(fd)

    def test_regular_unsealed_file_refused(self):
        with tempfile.TemporaryFile() as file:
            file.write(pixels())
            file.flush()
            self.rejected(b"CAVSHM01" + frame(file.fileno(), len(pixels())), diagnostic=b"seals")

    def test_unopenable_descriptor(self):
        self.rejected(b"CAVSHM01" + frame(2147483647, len(pixels())), diagnostic=b"cannot open")

    def test_truncation_at_every_field(self):
        fd = snapshot(pixels())
        try:
            wire = b"CAVSHM01" + frame(fd, len(pixels()))
            for end in range(len(wire)):
                if end == 8:
                    continue
                with self.subTest(end=end):
                    self.rejected(wire[:end])
        finally:
            os.close(fd)

    def test_unknown_tags_and_magic(self):
        for wire in (b"CAVSHM00", b"CAVSHM01\0", b"CAVSHM01\xff"):
            self.rejected(wire)

    def test_duplicate_or_skipped_sequences(self):
        fd = snapshot(pixels())
        try:
            for seq, pts in ((1, 1), (3, 1)):
                result = self.invoke(b"CAVSHM01" + frame(fd, len(pixels())) + frame(fd, len(pixels()), seq, pts))
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(messages(result.stdout)[0], [1])
        finally:
            os.close(fd)

    def test_timecodes_require_an_absolute_clock_program_and_default_fps(self):
        args = timecode_arguments("/no-such-helper-timecodes", "/no-such-clock")
        for bad in (
            args[:-2], replace(args, "--coreav-clock-program", "x264"),
            args + ["--fps", "30000/1001"], replace(args, "--coreav-vfr", "0"),
            extended(arguments()) + ["--coreav-clock-program", "/no-such-clock"],
            args,
        ):
            with self.subTest(args=bad):
                self.rejected(b"CAVSHM01", bad)

    def test_clock_probe_rejects_failed_or_malformed_diagnostics_before_protocol(self):
        line = b"timecode [info]: automatic timebase generation 1/1000\n"
        cases = [
            (b"", 0), (b"unrelated\n", 0), (line * 2, 0), (line[:-1], 0),
            (line + b"truncated", 0), (line + b"\0\n", 0), (line, 1),
            (line + b"x" * 8192 + b"\n", 0),
        ]
        for value in ("0/1", "1/0", "1/2147483648", "2147483648/1", "-1/2", "1/2 junk", "1"):
            cases.append((f"timecode [info]: automatic timebase generation {value}\n".encode(), 0))
        with tempfile.TemporaryDirectory() as directory:
            for diagnostic, status in cases:
                with self.subTest(diagnostic=diagnostic[:100], status=status):
                    program = fake_clock(directory, f"import os\nos.write(2, {diagnostic!r})\nraise SystemExit({status})\n")
                    result = self.invoke(b"CAVSHM01", timecode_arguments("/unused-timecodes", program))
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(result.stdout, b"")
                    self.assertIn(b"clock probe", result.stderr)

    def test_clock_probe_inherits_group_and_limits_but_not_extra_descriptors(self):
        import resource
        fd = snapshot(pixels())
        try:
            with tempfile.TemporaryDirectory() as directory:
                expected = ["--demuxer", "raw", "--input-res", "16x16", "--tcfile-in", "/unused-timecodes",
                            "--log-level", "info", "--no-progress", "--muxer", "raw", "--output", "-", "-"]
                program = fake_clock(directory, (
                    "import os, resource, sys\n"
                    f"assert sys.argv[1:] == {expected!r}\n"
                    f"assert os.getpgrp() == {os.getpgrp()}\n"
                    f"assert resource.getrlimit(resource.RLIMIT_NOFILE) == {resource.getrlimit(resource.RLIMIT_NOFILE)!r}\n"
                    f"assert not os.path.exists('/proc/self/fd/{fd}')\n"
                    "assert os.read(0, 1) == b''\n"
                    "assert os.readlink('/proc/self/fd/0') == '/dev/null'\n"
                    "assert os.readlink('/proc/self/fd/1') == '/dev/null'\n"
                    "os.write(2, b'timecode [info]: automatic timebase generation 1/1000\\n')\n"
                ))
                result = self.invoke(b"CAVSHM01", timecode_arguments("/unused-timecodes", program), (fd,))
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(messages(result.stdout), ([], b""))
        finally:
            os.close(fd)

    def test_timestamp_rescale_refuses_negative_and_overflow(self):
        fd = snapshot(pixels())
        try:
            self.rejected(b"CAVSHM01" + frame(fd, len(pixels()), pts=-1), diagnostic=b"nonnegative")
            with tempfile.TemporaryDirectory() as directory:
                program = fake_clock(directory, "import os\nos.write(2, b'timecode [info]: automatic timebase generation 1/2147483647\\n')\n")
                args = replace(timecode_arguments("/unused-timecodes", program), "--coreav-timebase", "2147483647/1")
                self.rejected(b"CAVSHM01" + frame(fd, len(pixels()), pts=2**63 - 1), args, b"rescaled")
        finally:
            os.close(fd)

    def test_clock_probe_accepts_exact_diagnostic_limit(self):
        diagnostic = b"timecode [info]: automatic timebase generation 1/1000\n"
        diagnostic += b"x" * (8191 - len(diagnostic)) + b"\n"
        with tempfile.TemporaryDirectory() as directory:
            program = fake_clock(directory, f"import os\nos.write(2, {diagnostic!r})\n")
            result = self.invoke(b"CAVSHM01", timecode_arguments("/unused-timecodes", program))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(messages(result.stdout), ([], b""))

    def test_rational_timestamp_rescale_rounds_nearest_with_ties_up(self):
        times = [0, 1500000, 7499999, 10500000, 15000001, 19500000, 25499999, 28500000]
        expected = [0, 1, 2, 4, 5, 7, 8, 10]
        frames = [pixels(frame=i) for i in range(len(times))]
        fds = [snapshot(data) for data in frames]
        try:
            with tempfile.TemporaryDirectory() as directory:
                program = fake_clock(directory, "import os\nos.write(2, b'timecode [info]: automatic timebase generation 3/1000\\n')\n")
                args = timecode_arguments("/unused-timecodes", program)
                result = self.invoke(
                    b"CAVSHM01" + b"".join(frame(fd, len(frames[i]), i + 1, times[i]) for i, fd in enumerate(fds)), args,
                )
                reference_args = replace(extended(arguments()), "--fps", "25/1")
                reference_args = replace(replace(reference_args, "--coreav-timebase", "3/1000"), "--coreav-vfr", "1")
                reference = self.invoke(
                    b"CAVSHM01" + b"".join(frame(fd, len(frames[i]), i + 1, expected[i]) for i, fd in enumerate(fds)),
                    reference_args,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(reference.returncode, 0, reference.stderr)
                self.assertEqual(messages(result.stdout), messages(reference.stdout))
        finally:
            for fd in fds:
                os.close(fd)

    def test_bad_configuration(self):
        args = extended(arguments())
        for key, value in (
            ("--input-res", "16385x32"), ("--input-res", "31x32"),
            ("--input-res", "32x31"), ("--input-res", "32x32junk"),
            ("--input-res", "0x32"), ("--input-depth", "12"),
            ("--output-depth", "10"), ("--input-csp", "nv12"),
            ("--output-csp", "i444"), ("--coreav-timebase", "1/0"),
            ("--coreav-timebase", "1/2147483648"), ("--fps", "nan"),
            ("--coreav-vfr", "2"), ("--output", "/tmp/output"),
        ):
            with self.subTest(key=key, value=value):
                self.rejected(b"CAVSHM01", replace(args, key, value))
        for extra in (["--threads", "2"], ["--unknown"], ["--input-range", "pc", "--range", "tv"]):
            self.rejected(b"CAVSHM01", args + extra)

    def test_large_nal_is_split_into_bounded_messages(self):
        data = random.Random(17).randbytes(768 * 768 * 3)
        fd = snapshot(data)
        try:
            args = replace(arguments("i444"), "--input-res", "768x768")
            args = replace(replace(args, "--crf", "0"), "--preset", "ultrafast")
            result = self.invoke(b"CAVSHM01" + frame(fd, len(data)), extended(args))
            self.assertEqual(result.returncode, 0, result.stderr)
            acks, encoded = messages(result.stdout)
            self.assertEqual(acks, [1])
            self.assertGreater(len(encoded), 1048576)
            self.assertIn(b"\x02" + struct.pack("<I", 1048576), result.stdout)
        finally:
            os.close(fd)

    def test_vfr_annexb_matches_cli_with_timecode_clock(self):
        if not X264:
            self.skipTest("set X264 for the optional timecode byte comparison")
        patterns = {
            "integer_ms_jitter": ["0", "33", "67", "100", "150", "184", "217", "267"],
            "native_1_6": ["0", "166.666667", "333.333333", "500", "666.666667", "833.333333", "1000", "1166.666667"],
            "native_1001_30000": ["0", "33.366667", "66.733333", "100.1", "133.466667", "166.833333", "200.2", "233.566667"],
            "native_1_25": ["0", "40", "80", "120", "160", "200", "240", "280"],
        }
        for name, times in patterns.items():
            with self.subTest(pattern=name):
                frames = [pixels(frame=i) for i in range(len(times))]
                fds = [snapshot(data) for data in frames]
                pts = [int(Decimal(t) * 1000000) for t in times]
                try:
                    with tempfile.NamedTemporaryFile(mode="w") as timecodes:
                        timecodes.write("# timecode format v2\n" + "\n".join(times) + "\n")
                        timecodes.flush()
                        helper_args = timecode_arguments(timecodes.name, X264)
                        args = helper_args[:helper_args.index("--coreav-shm")]
                        result = self.invoke(
                            b"CAVSHM01" + b"".join(frame(fd, len(frames[i]), i + 1, pts[i]) for i, fd in enumerate(fds)),
                            helper_args,
                        )
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(messages(result.stdout)[0], list(range(1, len(times) + 1)))
                        reference = subprocess.run(
                            [X264, *args], input=b"".join(frames), stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=15,
                        )
                        self.assertEqual(reference.returncode, 0, reference.stderr)
                        self.assertEqual(messages(result.stdout)[1], reference.stdout)
                finally:
                    for fd in fds:
                        os.close(fd)

    def test_annexb_matches_cli(self):
        if not X264:
            self.skipTest("set X264 to an independently installed x264 CLI for byte comparison")
        for csp in ("i420", "i422", "i444"):
            for depth in (8, 10):
                with self.subTest(csp=csp, depth=depth):
                    args = arguments(csp, depth)
                    frames = [pixels(csp, depth, i) for i in range(8)]
                    fds = [snapshot(data) for data in frames]
                    try:
                        result = self.invoke(
                            b"CAVSHM01" + b"".join(frame(fd, len(frames[i]), i + 1, i) for i, fd in enumerate(fds)),
                            extended(args),
                        )
                        self.assertEqual(result.returncode, 0, result.stderr)
                        reference = subprocess.run(
                            [X264, *args], input=b"".join(frames), stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=15,
                        )
                        self.assertEqual(reference.returncode, 0, reference.stderr)
                        self.assertEqual(messages(result.stdout)[1], reference.stdout)
                    finally:
                        for fd in fds:
                            os.close(fd)

    def test_metadata_bitrate_and_thread_options_match_cli(self):
        if not X264:
            self.skipTest("set X264 for the optional encoder-option byte comparison")
        args = arguments("i422", 10)
        index = args.index("--crf")
        args[index:index + 2] = ["--bitrate", "300"]
        args = replace(replace(args, "--threads", "2"), "--preset", "slow")
        args += ["--colorprim", "bt2020", "--transfer", "smpte2084",
                 "--colormatrix", "bt2020nc", "--sar", "16:15",
                 "--input-range", "pc", "--range", "pc"]
        frames = [pixels("i422", 10, i) for i in range(8)]
        fds = [snapshot(data) for data in frames]
        try:
            result = self.invoke(
                b"CAVSHM01" + b"".join(frame(fd, len(frames[i]), i + 1, i) for i, fd in enumerate(fds)),
                extended(args),
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            reference = subprocess.run(
                [X264, *args], input=b"".join(frames), stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, timeout=15,
            )
            self.assertEqual(reference.returncode, 0, reference.stderr)
            self.assertEqual(messages(result.stdout)[1], reference.stdout)
        finally:
            for fd in fds:
                os.close(fd)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--helper", required=True)
    parser.add_argument("--x264", default=os.environ.get("X264") or shutil.which("x264"))
    options, remaining = parser.parse_known_args()
    HELPER = str(Path(options.helper).resolve(strict=True))
    X264 = str(Path(options.x264).resolve(strict=True)) if options.x264 else None
    unittest.main(argv=[sys.argv[0], *remaining])
