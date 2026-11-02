"""Synthetic command and container-field contracts; never generate media here."""

import importlib.util
import json
import pathlib
import struct
import sys
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location("thumbnail_fixtures", ROOT / "scripts/wasm-thumbnail-fixtures.py")
FIXTURES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FIXTURES)


class FixtureContracts(unittest.TestCase):
    def test_roster_and_commands_are_complete_without_running_a_binary(self):
        rows = list(FIXTURES.fixture_rows())
        self.assertEqual(len(rows), 52)
        self.assertEqual(len({row["id"] for row in rows}), 52)
        plan = list(FIXTURES.commands(pathlib.Path("/oracle"), pathlib.Path("/x264"), pathlib.Path("/outside")))
        encodes = [row["argv"] for row in plan if row["kind"] == "encode"]
        self.assertEqual(len(encodes), 14)
        for argv in encodes:
            for key, value in [("--frames", "50"), ("--fps", "25"), ("--keyint", "25"), ("--min-keyint", "25"), ("--scenecut", "0"), ("--threads", "1"), ("--bframes", "0")]:
                self.assertEqual(argv[argv.index(key) + 1], value)
        self.assertEqual(sum("--no-cabac" in argv for argv in encodes), 7)
        self.assertEqual(sum("--crop-rect" in argv for argv in encodes), 2)
        self.assertEqual(sum(row["kind"] == "inspect-sps" for row in plan), 14)

    def test_mp4_patch_changes_both_geometry_owners_not_payload_or_configuration(self):
        def box(kind, data):
            return struct.pack(">I4s", len(data) + 8, kind) + data
        entry = box(b"avc1", b"\x00" * 24 + struct.pack(">HH", 3840, 2160) + b"CONFIG")
        stsd = box(b"stsd", b"\x00" * 4 + struct.pack(">I", 1) + entry)
        tkhd = box(b"tkhd", b"\x00" * 8 + struct.pack(">II", 3840 << 16, 2160 << 16))
        original = box(b"moov", box(b"trak", tkhd + box(b"mdia", box(b"minf", box(b"stbl", stsd))))) + box(b"mdat", b"UNCHANGED")
        patched = FIXTURES.patch_dimensions(original, "mp4")
        self.assertEqual(len(original), len(patched))
        self.assertIn(struct.pack(">HH", 640, 360) + b"CONFIG", patched)
        self.assertIn(struct.pack(">II", 640 << 16, 360 << 16), patched)
        self.assertTrue(patched.endswith(box(b"mdat", b"UNCHANGED")))
        with self.assertRaises(ValueError):
            FIXTURES.patch_dimensions(box(b"mdat", b"UNCHANGED"), "mp4")

    def test_matroska_patch_preserves_sizes_and_cluster_bytes(self):
        def element(kind, data):
            return kind.to_bytes((kind.bit_length() + 7) // 8, "big") + bytes([0x80 | len(data)]) + data
        dimensions = element(0xB0, (3840).to_bytes(2, "big")) + element(0xBA, (2160).to_bytes(2, "big"))
        cluster = element(0x1F43B675, b"UNCHANGED")
        original = element(0x18538067, element(0x1654AE6B, element(0xAE, element(0xE0, dimensions))) + cluster)
        patched = FIXTURES.patch_dimensions(original, "matroska")
        self.assertEqual(len(patched), len(original))
        self.assertIn(element(0xB0, (640).to_bytes(2, "big")), patched)
        self.assertIn(element(0xBA, (360).to_bytes(2, "big")), patched)
        self.assertTrue(patched.endswith(cluster))
        with self.assertRaises(ValueError):
            FIXTURES.patch_dimensions(original[:-1], "matroska")

    def test_generation_is_linux_only_and_never_targets_the_checkout(self):
        with tempfile.TemporaryDirectory() as directory:
            target = pathlib.Path(directory) / "not-created"
            with mock.patch.object(FIXTURES.sys, "platform", "darwin"):
                with self.assertRaisesRegex(ValueError, "Linux-only"):
                    FIXTURES.generate(pathlib.Path("/oracle"), pathlib.Path("/x264"), target)
            self.assertFalse(target.exists())
        with mock.patch.object(FIXTURES.sys, "platform", "linux"):
            with self.assertRaisesRegex(ValueError, "outside"):
                FIXTURES.generate(pathlib.Path("/oracle"), pathlib.Path("/x264"), ROOT / "generated")

    def test_every_subprocess_has_a_finite_deadline_and_private_output(self):
        response = mock.Mock(returncode=0, stdout=b"result", stderr=b"private diagnostic")
        with mock.patch.object(FIXTURES.subprocess, "run", return_value=response) as run:
            records = []
            self.assertEqual(FIXTURES.execute(["/binary"], records), b"result")
            self.assertEqual(run.call_args.kwargs["timeout"], 300)
            self.assertEqual(records[0]["stderr"], "private diagnostic")

    def test_hostile_and_valid_boundary_handoff_is_complete_and_byte_bound(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            payload = root / "marker.bin"
            payload.write_bytes(b"synthetic marker, never decoded")
            required = FIXTURES.registry()["hostile"] + FIXTURES.registry()["boundaries"]
            rows = [{"id": name, "path": payload.name, "sha256": FIXTURES.digest(payload.read_bytes()), "options": {"at": "0"}, "expectation": "success" if name in FIXTURES.registry()["boundaries"] else "decode"} for name in required]
            path = root / "hostile.json"
            path.write_text(json.dumps(rows))
            self.assertEqual(FIXTURES.hostile_rows(path), rows)
            for bad in [rows[:-1], rows + rows[:1], [rows[0]] * len(rows), [{**row, "id": "unknown"} for row in rows]]:
                path.write_text(json.dumps(bad))
                with self.assertRaisesRegex(ValueError, "roster"):
                    FIXTURES.hostile_rows(path)
            path.write_text(json.dumps(rows))
            payload.write_bytes(b"changed marker")
            with self.assertRaisesRegex(ValueError, "bytes changed"):
                FIXTURES.hostile_rows(path)
            with mock.patch.object(FIXTURES.sys, "platform", "linux"), mock.patch.object(FIXTURES.subprocess, "run") as run:
                with self.assertRaisesRegex(ValueError, "bytes changed"):
                    FIXTURES.generate(root, root / "x264", root / "output", path)
                run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
