#!/usr/bin/env python3
"""Hosted synthetic HEVC hardware capture and independent elementary/MP4 decode."""

import argparse
from fractions import Fraction
from pathlib import Path
import platform
import sys
import tempfile

from evidence import Invalid, bind, digest, file_hash, integer, keys, load, read, require, token, verify, write
from observations import encoder_identifier
from process import run
from smoke import LIMIT, ROOT, pixels, probe_sections, source

sys.path.insert(0, str(ROOT / "scripts"))
import test_oracle

FILES = ("manifest.json", "video.hevc", "source.yuv")


def configuration(value):
    require(type(value) is list and 23 <= len(value) <= 65536, "hvcc")
    require(all(type(byte) is int and 0 <= byte <= 255 for byte in value), "hvcc")
    data = bytes(value)
    require(data[0] == 1 and data[1] & 31 == 1 and data[21] & 3 == 3 and data[22] == 3, "hvcc")
    offset, parameter_sets = 23, {}
    for _ in range(3):
        require(offset + 3 <= len(data), "hvcc_extent")
        kind = data[offset] & 63
        require(kind in (32, 33, 34) and kind not in parameter_sets and
                int.from_bytes(data[offset + 1:offset + 3], "big") == 1, "hvcc_arrays")
        offset += 3
        require(offset + 2 <= len(data), "hvcc_extent")
        size = int.from_bytes(data[offset:offset + 2], "big")
        offset += 2
        require(size >= 2 and offset + size <= len(data), "hvcc_extent")
        nal = data[offset:offset + size]
        require(nal[0] >> 1 & 63 == kind, "hvcc_nal")
        parameter_sets[kind] = nal
        offset += size
    require(offset == len(data), "hvcc_extent")
    return b"".join(b"\0\0\0\1" + parameter_sets[kind] for kind in (32, 33, 34))


def manifest(value):
    keys(value, "schema status requested_mode width height fps_num fps_den bitrate submitted "
         "access_unit_bytes encoder_id hardware frames settings drain_complete codec hvcc elementary_file source_file")
    require(value["schema"] == "coreav.videotoolbox-encode/1" and value["status"] == "encoded" and
            value["codec"] == "hevc" and value["requested_mode"] == "hardware" and
            value["hardware"] == "hardware" and value["drain_complete"] is True, "encode_status")
    encoder_identifier(value["encoder_id"])
    for field, expected in (("width", 64), ("height", 64), ("fps_num", 30), ("fps_den", 1),
                            ("bitrate", 2000000), ("submitted", 4)):
        require(type(value[field]) is int and value[field] == expected, "encode_settings")
    integer(value["access_unit_bytes"], 1, LIMIT)
    require(value["elementary_file"] == "video.hevc" and value["source_file"] == "source.yuv", "paths")
    require(value["settings"] == {
        "profile": "main", "reordering": False, "realtime": False, "expected_frame_rate_hint": 30,
        "keyframe_seconds": 2, "keyframe_frames": 60, "pixel_format": "i420",
        "native_pixel_format": "420v", "color_range": "limited", "primaries": "bt709",
        "transfer": "bt709", "matrix": "bt709"}, "encode_settings")
    frames = value["frames"]
    require(type(frames) is list and len(frames) == 4, "frame_count")
    for index, frame in enumerate(frames):
        keys(frame, "id pts_value pts_scale duration_value duration_scale sync")
        require(type(frame["id"]) is int and frame["id"] == index, "frame_identity")
        for name in ("pts_value", "pts_scale", "duration_value", "duration_scale"):
            integer(frame[name], 0 if name == "pts_value" else 1, 2**63 - 1)
        require(Fraction(frame["pts_value"], frame["pts_scale"]) == Fraction(index, 30) and
                Fraction(frame["duration_value"], frame["duration_scale"]) == Fraction(1, 30), "clock")
        require(type(frame["sync"]) is bool and (index != 0 or frame["sync"]), "sync")
    return configuration(value["hvcc"])


def artifacts(directory, captured):
    require({path.name for path in directory.iterdir()} ==
            set(FILES) | ({"capture.json"} if captured else set()), "file_coverage")
    value = load(directory / "manifest.json", 131072)
    prefix = manifest(value)
    require(read(directory, "source.yuv", 24576) == source(), "source_pattern")
    video = read(directory, "video.hevc", LIMIT)
    require(video.startswith(prefix) and len(video) > len(prefix), "parameter_sets")
    return value


def capture(exporter, output, runner_image):
    require(sys.platform == "darwin" and platform.machine() == "arm64", "capture_platform")
    token(runner_image)
    exporter = Path(exporter).resolve(strict=True)
    identity = file_hash(exporter)
    output = Path(output).resolve()
    output.mkdir()
    with tempfile.TemporaryDirectory() as temporary:
        run([str(exporter), "smoke", "--codec", "hevc", "--mode", "hardware", "--output", str(output)],
            temporary, 120)
    artifacts(output, False)
    require(file_hash(exporter) == identity, "tool_changed")
    write(output / "capture.json", {
        "schema": "coreav.videotoolbox-hevc-capture/1", "exporter_sha256": identity,
        "runner_image": runner_image, "machine": platform.machine(), "files": bind(output, FILES, LIMIT)})


def probe(value, native, container):
    probe_sections(value)
    require(type(value["streams"]) is list and len(value["streams"]) == 1, "probe_streams")
    expected = {"codec_name": "hevc", "width": 64, "height": 64, "pix_fmt": "yuv420p",
                "color_range": "tv", "color_space": "bt709", "color_transfer": "bt709",
                "color_primaries": "bt709"}
    if container:
        expected["codec_tag_string"] = "hvc1"
    stream = dict(value["streams"][0])
    if not container:
        stream.pop("codec_tag_string", None)
    require(stream == expected, "probe_settings")
    frames = value["frames"]
    require(type(frames) is list and len(frames) == 4, "decoded_frame_count")
    for index, frame in enumerate(frames):
        require(type(frame) is dict and set(frame) <=
                {"media_type", "key_frame", "width", "height", "pix_fmt", "side_data_list"}, "probe_frame")
        require(frame.get("media_type") == "video" and frame.get("width") == 64 and
                frame.get("height") == 64 and frame.get("pix_fmt") == "yuv420p" and
                type(frame.get("key_frame")) is int and frame["key_frame"] in (0, 1) and
                bool(frame["key_frame"]) == native["frames"][index]["sync"], "probe_frame")


def verify_capture(directory, oracle):
    require(sys.platform == "linux" and platform.machine() == "x86_64", "oracle_platform")
    directory, oracle = Path(directory).resolve(), Path(oracle).resolve()
    record = load(directory / "capture.json", 32768)
    keys(record, "schema exporter_sha256 runner_image machine files")
    require(record["schema"] == "coreav.videotoolbox-hevc-capture/1" and record["machine"] == "arm64", "capture_schema")
    digest(record["exporter_sha256"])
    token(record["runner_image"])
    native = artifacts(directory, True)
    verify(directory, record["files"], FILES, LIMIT)
    with tempfile.TemporaryDirectory() as temporary:
        temporary = Path(temporary)
        try:
            oracle = test_oracle.require(oracle / "ffmpeg", oracle / "ffprobe")
        except test_oracle.QualificationError:
            raise Invalid("child_failure") from None
        hashes = {name: file_hash(getattr(oracle, name)) for name in ("ffmpeg", "ffprobe")}
        elementary = temporary / "video.hevc"
        elementary.write_bytes(read(directory, "video.hevc", LIMIT))
        mp4 = temporary / "video.mp4"
        run([str(oracle.ffmpeg), "-nostdin", "-v", "error", "-xerror", "-f", "hevc",
             "-r", "30", "-i", str(elementary), "-map", "0:v:0", "-an", "-c:v", "copy",
             "-tag:v", "hvc1", str(mp4)], temporary, 60, stdout="mux-out", stderr="mux-error")
        require(not read(temporary, "mux-error", 32768), "mux_error")
        decoded = []
        for name, path, format_args in (("elementary", elementary, ["-f", "hevc"]), ("mp4", mp4, [])):
            run([str(oracle.ffmpeg), "-nostdin", "-v", "error", "-xerror", "-err_detect", "explode",
                 "-max_alloc", "67108864", "-max_pixels", "4096", "-threads", "1", "-c:v", "hevc",
                 *format_args, "-i", str(path), "-map", "0:v:0", "-an", "-frames:v", "5",
                 "-fps_mode", "passthrough", "-pix_fmt", "yuv420p", "-f", "rawvideo",
                 str(temporary / f"{name}.yuv")], temporary, 60, stdout=f"{name}-out", stderr=f"{name}-error")
            run([str(oracle.ffprobe), "-v", "error", "-max_alloc", "67108864", "-max_pixels", "4096",
                 "-threads", "1", *format_args, "-select_streams", "v:0", "-show_entries",
                 "stream=codec_name,codec_tag_string,width,height,pix_fmt,color_range,color_space,color_transfer,color_primaries:"
                 "frame=media_type,key_frame,width,height,pix_fmt", "-of", "json", str(path)],
                temporary, 60, stdout=f"{name}.json", stderr=f"{name}-probe-error")
            raw = read(temporary, f"{name}.yuv", 24576)
            pixels(raw)
            for frame in range(4):
                require(all(abs(byte - 128) <= 8 for byte in raw[frame * 6144 + 4096:(frame + 1) * 6144]),
                        "decoded_chroma")
            require(not read(temporary, f"{name}-error", 32768) and
                    not read(temporary, f"{name}-probe-error", 32768), "decoder_error")
            probe(load(temporary / f"{name}.json", LIMIT), native, name == "mp4")
            decoded.append(raw)
        require(decoded[0] == decoded[1], "container_pixels")
        require(hashes == {name: file_hash(getattr(oracle, name)) for name in hashes}, "tool_changed")
    verify(directory, record["files"], FILES, LIMIT)
    return {"schema": "coreav.videotoolbox-hevc-verification/1", "status": "verified",
            "codec": "hevc", "hardware": "hardware", "frames": 4, "width": 64, "height": 64,
            "capture_sha256": file_hash(directory / "capture.json"), "oracle_sha256": hashes,
            "scope": "synthetic-elementary-and-hvc1-mp4"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    cap = commands.add_parser("capture")
    cap.add_argument("--exporter", required=True, type=Path)
    cap.add_argument("--output", required=True, type=Path)
    cap.add_argument("--runner-image", required=True)
    check = commands.add_parser("verify")
    check.add_argument("--capture", required=True, type=Path)
    check.add_argument("--oracle-bin", required=True, type=Path)
    check.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == "capture":
            capture(args.exporter, args.output, args.runner_image)
        else:
            write(args.report, verify_capture(args.capture, args.oracle_bin))
        return 0
    except (Invalid, OSError, UnicodeError, ValueError, TypeError) as error:
        if args.command == "verify":
            write(args.report, {"schema": "coreav.videotoolbox-hevc-verification/1", "status": "failed"})
        diagnostic = error.code if isinstance(error, Invalid) else "capture_or_verification_error"
        print(f"videotoolbox-hevc-smoke: failed ({diagnostic})", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
