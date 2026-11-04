import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

import capture
from evidence import Invalid, sha


class Metadata(unittest.TestCase):
    def test_configuration_is_unique_and_shell_parsed(self):
        self.assertEqual(
            capture.configuration(b"configuration: --enable-one '--name=two words'\n"),
            ["--enable-one", "--name=two words"],
        )
        for value in (b"missing", b"configuration: one\nconfiguration: two\n"):
            with self.assertRaises(Invalid):
                capture.configuration(value)

    def test_command_failure_and_output_bounds_refuse(self):
        result = mock.Mock(stdout=b"x" * (1024**2 + 1), stderr=b"")
        with (
            mock.patch.object(capture.subprocess, "run", return_value=result),
            self.assertRaises(Invalid),
        ):
            capture.command(["metadata-only"])

    def test_observer_verifies_archive_and_binary_before_execution(self):
        binary = b"pinned-observer"
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w:gz") as archive:
            member = tarfile.TarInfo("vmtouch/1.3.1/bin/vmtouch")
            member.size = len(binary)
            archive.addfile(member, io.BytesIO(binary))
        raw = buffer.getvalue()
        token, bottle = mock.MagicMock(), mock.MagicMock()
        token.__enter__.return_value.read.return_value = b'{"token":"not-logged"}'
        bottle.__enter__.return_value.read.return_value = raw
        with (
            tempfile.TemporaryDirectory() as directory,
            mock.patch.object(capture, "ROOT", Path(directory)),
            mock.patch.object(capture, "BOTTLE_SHA", sha(raw)),
            mock.patch.object(capture, "BINARY_SHA", sha(binary)),
            mock.patch.object(
                capture.urllib.request, "urlopen", side_effect=[token, bottle]
            ),
            mock.patch.object(
                capture,
                "command",
                side_effect=[b"arm64\n", b"observer:\n /usr/lib/libSystem.B.dylib\n"],
            ),
        ):
            self.assertEqual(capture.observer_binary().read_bytes(), binary)

    def test_corrupt_archive_stops_before_publication_or_execution(self):
        token, bottle = mock.MagicMock(), mock.MagicMock()
        token.__enter__.return_value.read.return_value = b'{"token":"not-logged"}'
        bottle.__enter__.return_value.read.return_value = b"changed"
        with (
            mock.patch.object(
                capture.urllib.request, "urlopen", side_effect=[token, bottle]
            ),
            mock.patch.object(capture, "command") as command,
            self.assertRaises(Invalid),
        ):
            capture.observer_binary()
        command.assert_not_called()

    def test_failed_baseline_verification_cannot_publish(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (
                mock.patch.object(capture.sys, "platform", "darwin"),
                mock.patch.object(
                    capture.oracle_pin_macos, "verify_bin", return_value=["mismatch"]
                ),
                self.assertRaises(Invalid),
            ):
                capture.capture(root, root / "output")
            self.assertFalse((root / "output").exists())

    def test_collector_never_builds_or_encodes(self):
        source = Path(capture.__file__).read_text()
        for forbidden in ('-i"', "-c:v", "cargo", "producer.py", "timing.py", "rental"):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
