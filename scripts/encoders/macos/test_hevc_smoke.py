"""Synthetic regressions for the hosted HEVC reference smoke; no media tools."""

import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import hevc_smoke as smoke
from evidence import Invalid, bind, write


def native_report():
    hvcc = bytearray(23)
    hvcc[0], hvcc[1], hvcc[21], hvcc[22] = 1, 1, 3, 3
    for kind in (32, 33, 34):
        hvcc.extend(bytes([128 | kind, 0, 1, 0, 3, kind << 1, 1, 128]))
    return {
        "schema": "coreav.videotoolbox-encode/1", "status": "encoded", "codec": "hevc",
        "requested_mode": "hardware", "hardware": "hardware", "encoder_id": "synthetic encoder",
        "width": 64, "height": 64, "fps_num": 30, "fps_den": 1, "bitrate": 2000000,
        "submitted": 4, "access_unit_bytes": 128, "drain_complete": True,
        "elementary_file": "video.hevc", "source_file": "source.yuv", "hvcc": list(hvcc),
        "frames": [{"id": index, "pts_value": index, "pts_scale": 30,
                    "duration_value": 1, "duration_scale": 30, "sync": index == 0} for index in range(4)],
        "settings": {"profile": "main", "reordering": False, "realtime": False,
                     "expected_frame_rate_hint": 30, "keyframe_seconds": 2, "keyframe_frames": 60,
                     "pixel_format": "i420", "native_pixel_format": "420v", "color_range": "limited",
                     "primaries": "bt709", "transfer": "bt709", "matrix": "bt709"},
    }


def oracle_probe(container=False):
    stream = {"codec_name": "hevc", "codec_tag_string": "hvc1" if container else "[0][0][0][0]",
              "width": 64, "height": 64, "pix_fmt": "yuv420p", "color_range": "tv",
              "color_space": "bt709", "color_transfer": "bt709", "color_primaries": "bt709"}
    return {"streams": [stream], "frames": [
        {"media_type": "video", "key_frame": int(index == 0), "width": 64,
         "height": 64, "pix_fmt": "yuv420p"} for index in range(4)]}


def populate(directory):
    value = native_report()
    write(directory / "manifest.json", value)
    (directory / "source.yuv").write_bytes(smoke.source())
    (directory / "video.hevc").write_bytes(smoke.configuration(value["hvcc"]) + b"\0\0\0\1\x26\x01\x80")


class HevcSmoke(unittest.TestCase):
    def test_manifest_requires_encoded_hardware_and_validated_configuration(self):
        value = native_report()
        self.assertTrue(smoke.manifest(value).startswith(b"\0\0\0\1\x40\x01"))
        for field, wrong in (("status", "unavailable"), ("hardware", "unknown"),
                             ("requested_mode", "software"), ("drain_complete", False),
                             ("codec", "h264"), ("submitted", 3), ("width", 32),
                             ("encoder_id", None), ("hvcc", [])):
            with self.subTest(field=field):
                changed = copy.deepcopy(value)
                changed[field] = wrong
                with self.assertRaises(Invalid):
                    smoke.manifest(changed)
        for mutation in (lambda v: v.pop("hvcc"), lambda v: v.update(avcc=[1, 2, 3]),
                         lambda v: v["settings"].update(profile="high"),
                         lambda v: v["frames"][1].update(pts_value=2),
                         lambda v: v["frames"][0].update(sync=False)):
            changed = copy.deepcopy(value)
            mutation(changed)
            with self.assertRaises(Invalid):
                smoke.manifest(changed)

    def test_configuration_refuses_incomplete_arrays_lengths_and_non_bytes(self):
        original = native_report()["hvcc"]
        for value in (original[:-1], original + [0], [True] + original[1:],
                      original[:21] + [0] + original[22:],
                      original[:22] + [2] + original[23:],
                      original[:23] + [0] + original[24:],
                      original[:26] + [255, 255] + original[28:]):
            with self.subTest(value=value):
                with self.assertRaises(Invalid):
                    smoke.configuration(value)

    def test_artifacts_bind_configuration_to_elementary_bytes_and_source(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            populate(directory)
            self.assertEqual(smoke.artifacts(directory, False)["submitted"], 4)
            video = (directory / "video.hevc").read_bytes()
            (directory / "video.hevc").write_bytes(video.replace(b"\x40\x01\x80", b"\x40\x01\x81"))
            with self.assertRaisesRegex(Invalid, "parameter_sets"):
                smoke.artifacts(directory, False)
            (directory / "video.hevc").write_bytes(video)
            (directory / "source.yuv").write_bytes(b"\0" * 24576)
            with self.assertRaisesRegex(Invalid, "source_pattern"):
                smoke.artifacts(directory, False)

    def test_probe_refuses_short_extra_wrong_geometry_codec_and_container_tag(self):
        native = native_report()
        for container in (False, True):
            smoke.probe(oracle_probe(container), native, container)
            for mutation in (lambda v: v["frames"].pop(),
                             lambda v: v["frames"].append(v["frames"][0]),
                             lambda v: v["streams"][0].update(width=32),
                             lambda v: v["streams"][0].update(codec_name="h264"),
                             lambda v: v["frames"][1].update(height=32),
                             lambda v: v["frames"][0].update(key_frame=0)):
                changed = oracle_probe(container)
                mutation(changed)
                with self.assertRaises(Invalid):
                    smoke.probe(changed, native, container)
        wrong = oracle_probe(True)
        wrong["streams"][0]["codec_tag_string"] = "hev1"
        with self.assertRaises(Invalid):
            smoke.probe(wrong, native, True)

    def test_capture_invokes_hardware_hevc_and_records_complete_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary).resolve()
            exporter = root / "exporter"
            exporter.write_bytes(b"synthetic exporter")
            output = root / "capture"

            def run(command, directory, timeout):
                self.assertEqual(command, [str(exporter), "smoke", "--codec", "hevc", "--mode",
                                           "hardware", "--output", str(output)])
                self.assertEqual(timeout, 120)
                populate(output)

            with patch.object(smoke.sys, "platform", "darwin"), patch.object(smoke.platform, "machine", return_value="arm64"), \
                    patch.object(smoke, "run", side_effect=run):
                smoke.capture(exporter, output, "macos-15-synthetic")
            record = json.loads((output / "capture.json").read_text())
            self.assertEqual(set(record["files"]), set(smoke.FILES))
            self.assertEqual(smoke.artifacts(output, True)["codec"], "hevc")

    def verify_synthetic(self, mutation=None):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            capture, oracle = root / "capture", root / "oracle"
            capture.mkdir()
            oracle.mkdir()
            populate(capture)
            write(capture / "capture.json", {
                "schema": "coreav.videotoolbox-hevc-capture/1", "exporter_sha256": "1" * 64,
                "runner_image": "synthetic", "machine": "arm64", "files": bind(capture, smoke.FILES, smoke.LIMIT)})
            for name in ("ffmpeg", "ffprobe"):
                (oracle / name).write_bytes(b"synthetic oracle")
            commands = []

            def run(command, directory, timeout, stdout="stdout", stderr="stderr"):
                commands.append(command)
                directory = Path(directory)
                (directory / stdout).write_bytes(b"")
                (directory / stderr).write_bytes(b"")
                if "-tag:v" in command:
                    self.assertEqual(command[command.index("-c:v") + 1], "copy")
                    self.assertEqual(command[command.index("-tag:v") + 1], "hvc1")
                    Path(command[-1]).write_bytes(b"synthetic mp4")
                elif "rawvideo" in command:
                    self.assertEqual(command[command.index("-frames:v") + 1], "5")
                    self.assertIn("-xerror", command)
                    Path(command[-1]).write_bytes(smoke.source())
                elif Path(command[0]).name == "ffprobe":
                    write(directory / stdout, oracle_probe(command[-1].endswith(".mp4")))
                if mutation:
                    mutation(command, directory, stdout, stderr)

            with patch.object(smoke.sys, "platform", "linux"), patch.object(smoke.platform, "machine", return_value="x86_64"), \
                    patch.object(smoke, "run", side_effect=run):
                result = smoke.verify_capture(capture, oracle)
            self.assertEqual(result["status"], "verified")
            self.assertEqual(result["scope"], "synthetic-elementary-and-hvc1-mp4")
            self.assertEqual(sum("rawvideo" in c for c in commands), 2)
            self.assertEqual(sum(Path(c[0]).name == "ffprobe" for c in commands), 2)
            self.assertTrue(any("oracle_pin.py" in arg for c in commands for arg in c))

    def test_verification_decodes_both_elementary_and_copied_hvc1_container(self):
        self.verify_synthetic()

    def test_container_decode_is_not_skipped_or_allowed_to_change_pixels(self):
        def mutate(command, directory, stdout, stderr):
            if "rawvideo" in command and command[-1].endswith("mp4.yuv"):
                path = Path(command[-1])
                raw = bytearray(path.read_bytes())
                raw[0] += 1
                path.write_bytes(raw)

        with self.assertRaisesRegex(Invalid, "container_pixels"):
            self.verify_synthetic(mutate)

    def test_verification_rejects_missing_frames_bad_luma_chroma_and_diagnostics(self):
        def mutate(command, directory, stdout, stderr, fault):
            if "rawvideo" not in command:
                return
            path = Path(command[-1])
            raw = bytearray(path.read_bytes())
            if fault == "short":
                raw = raw[:-1]
            elif fault == "extra":
                raw += b"\0"
            elif fault == "luma":
                raw[0] = 255
            elif fault == "chroma":
                raw[4096] = 255
            elif fault == "diagnostic":
                (directory / stderr).write_bytes(b"decode error on exit zero")
            path.write_bytes(raw)

        for fault in ("short", "extra", "luma", "chroma", "diagnostic"):
            with self.subTest(fault=fault), self.assertRaises(Invalid):
                self.verify_synthetic(lambda *args: mutate(*args, fault))


if __name__ == "__main__":
    unittest.main()
