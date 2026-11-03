#!/usr/bin/env python3
"""Capture a bounded native smoke; verify transported H.264 on Linux."""

import argparse
from fractions import Fraction
from pathlib import Path
import platform
import re
import sys
import tempfile

from evidence import Invalid, bind, file_hash, integer, keys, load, read
from evidence import require, sha, token, verify, write
from process import run

ROOT = Path(__file__).resolve().parents[3]
LEVELS = (37, 91, 145, 199)
LIMIT = 4 * 1024**2
FIELDS = ("schema status requested_mode unavailable_reason width height fps_num fps_den "
          "bitrate submitted encoder_id hardware frames elementary_file source_file")
HOSTED_REFUSAL_SCHEMA = "coreav.videotoolbox-hosted-refusal/1"
HOSTED_REFUSAL_CAPTURE_SCHEMA = "coreav.videotoolbox-hosted-refusal-capture/1"
HOSTED_REFUSAL_VERIFICATION_SCHEMA = "coreav.videotoolbox-hosted-refusal-verification/1"
HOSTED_REFUSAL_OUTCOME = "refused_hardware_status_unobservable"
NATIVE_FAILURES = {
    "invalid VideoToolbox input or settings", "invalid VideoToolbox session state",
    "VideoToolbox resource limit exceeded", "cannot reserve VideoToolbox output storage",
    "VideoToolbox encoding cancelled", "VideoToolbox deadline expired",
    "invalid VideoToolbox output or accounting", "cannot establish VideoToolbox hardware status",
    "pull VideoToolbox output before submitting more frames",
    "the requested VideoToolbox encoder is unavailable", "VideoToolbox encoding failed",
    "malformed H.264 output metadata", "unsupported H.264 output metadata",
    "H.264 output format changed", "unknown H.264 parameter-set reference",
    "H.264 slices do not form one ordered picture", "H.264 sync declaration disagrees with IDR headers",
    "H.264 output metadata exceeds its limit", "cannot reserve H.264 output storage",
    "VideoToolbox encoder-list match has no hardware flag",
    "hardware status is now observable; remove the hosted software exemption",
}


def source():
    return b"".join(bytes([level]) * 4096 + bytes([128]) * 2048 for level in LEVELS)


def manifest(value, mode, hosted_software_exemption=False):
    if hosted_software_exemption:
        require(mode == "software" and type(value) is dict, "hosted_software_exemption_scope")
        require(not (value.get("schema") == "coreav.videotoolbox-smoke/1" and value.get("status") == "encoded"),
                "remove_hosted_software_exemption_flag_is_now_observable")
        keys(value, "schema outcome requested_mode condition submitted software_encode_exercised")
        require(value["schema"] == HOSTED_REFUSAL_SCHEMA and value["outcome"] == HOSTED_REFUSAL_OUTCOME and
                value["requested_mode"] == "software" and
                value["condition"] == "unique_session_encoder_id_match_without_hardware_flag" and
                type(value["submitted"]) is int and value["submitted"] == 0 and
                value["software_encode_exercised"] is False, "hosted_refusal_condition")
        return ["manifest.json"]
    keys(value, FIELDS)
    require(value["schema"] == "coreav.videotoolbox-smoke/1", "schema")
    require(mode in ("hardware", "software", "any") and
            value["requested_mode"] == mode, "mode")
    for field, expected in (("width", 64), ("height", 64), ("fps_num", 30),
                            ("fps_den", 1), ("bitrate", 2000000)):
        require(type(value[field]) is int and value[field] == expected, "settings")
    require(value["hardware"] in ("hardware", "software", "unknown"), "hardware")
    encoder = value["encoder_id"]
    require(encoder is None or (type(encoder) is str and 0 < len(encoder) <= 512
                               and all(32 <= ord(c) < 127 for c in encoder)), "encoder_id")
    if value["status"] == "unavailable":
        require(value["unavailable_reason"] in ("no_encoder", "temporarily_unavailable"),
                "availability")
        require(type(value["submitted"]) is int and value["submitted"] == 0 and
                value["frames"] == [] and value["elementary_file"] is None and
                value["source_file"] is None, "availability")
        return ["manifest.json"]
    require(value["status"] == "encoded" and value["unavailable_reason"] is None,
            "status")
    require(encoder is not None and (mode == "any" or value["hardware"] == mode),
            "hardware")
    require(type(value["submitted"]) is int and value["submitted"] == 4, "frame_count")
    require(value["elementary_file"] == "video.h264" and
            value["source_file"] == "source.yuv", "path")
    require(type(value["frames"]) is list and len(value["frames"]) == 4, "frame_count")
    for index, frame in enumerate(value["frames"]):
        keys(frame, "id pts_value pts_scale duration_value duration_scale sync")
        require(type(frame["id"]) is int and frame["id"] == index, "frame_identity")
        for name in ("pts_value", "pts_scale", "duration_value", "duration_scale"):
            integer(frame[name], 0 if name == "pts_value" else 1, 2**63 - 1)
        require(Fraction(frame["pts_value"], frame["pts_scale"]) == Fraction(index, 30)
                and Fraction(frame["duration_value"], frame["duration_scale"]) ==
                Fraction(1, 30), "clock")
        require(type(frame["sync"]) is bool and (index != 0 or frame["sync"]), "sync")
    return ["manifest.json", "video.h264", "source.yuv"]


def artifacts(directory, mode, captured, hosted_software_exemption=False):
    value = load(Path(directory) / "manifest.json", 32768)
    names = manifest(value, mode, hosted_software_exemption)
    require({p.name for p in Path(directory).iterdir()} ==
            set(names) | ({"capture.json"} if captured else set()), "file_coverage")
    for name in names:
        data = read(directory, name, LIMIT)
        require(data, "empty_file")
    if not hosted_software_exemption and value["status"] == "encoded":
        require(sha(read(directory, "source.yuv", 24576)) == sha(source()), "source_pattern")
    return value, names


def capture(exporter, output, mode, runner_image, hosted_software_exemption=False):
    require(sys.platform == "darwin", "capture_platform")
    require(not hosted_software_exemption or mode == "software", "hosted_software_exemption_scope")
    token(runner_image)
    exporter = Path(exporter).resolve(strict=True)
    identity = file_hash(exporter)
    output = Path(output)
    output.mkdir(parents=False, exist_ok=False)
    with tempfile.TemporaryDirectory() as temporary:
        try:
            command = [str(exporter), "smoke", "--mode", mode, "--output", str(output.resolve())]
            if hosted_software_exemption:
                command += ["--hosted-software-exemption", "true"]
            run(command, temporary, 120)
        except Invalid:
            try:
                detail = read(temporary, "stderr", 32768).decode("utf-8")
            except (Invalid, OSError, UnicodeError):
                detail = ""
            for line in detail.splitlines():
                if line.removeprefix("videotoolbox: ") in NATIVE_FAILURES or re.fullmatch(
                        r"videotoolbox: VideoToolbox (?:[A-Za-z]{1,32} failed with status -?[0-9]{1,10}|did not retain [A-Za-z0-9]{1,64})", line):
                    print(f"videotoolbox-smoke {mode}: {line}", file=sys.stderr)
            raise
        value, names = artifacts(output, mode, False, hosted_software_exemption)
        run(["/usr/bin/sw_vers", "-buildVersion"], temporary, 10,
            stdout="os-build", stderr="os-error")
        os_build = read(temporary, "os-build", 128).decode().strip()
        token(os_build)
        require(identity == file_hash(exporter), "tool_changed")
        write(output / "capture.json", {
            "schema": HOSTED_REFUSAL_CAPTURE_SCHEMA if hosted_software_exemption else "coreav.videotoolbox-capture/1",
            "requested_mode": mode,
            "exporter_sha256": identity, "runner_image": runner_image,
            "os_build": os_build, "machine": platform.machine(),
            "files": bind(output, names, LIMIT),
        })
        return value if hosted_software_exemption else {"status": value["status"], "requested_mode": mode}


def pixels(data):
    require(len(data) == 24576, "decoded_frame_count")
    for index, level in enumerate(LEVELS):
        luma = data[index * 6144:index * 6144 + 4096]
        require(abs(sum(luma) / 4096 - level) <= 8 and
                all(abs(pixel - level) <= 8 for pixel in luma), "decoded_frame_identity")


def probe_sections(value):
    required = {"streams", "frames"}
    wrappers = {"programs", "stream_groups"}
    valid = (type(value) is dict and required <= set(value) <= required | wrappers and
             all(type(value[key]) is list and not value[key] for key in wrappers & set(value)))
    if type(value) is not dict:
        print("oracle probe shape: object=false", file=sys.stderr)
    elif not valid:
        known = required | wrappers
        print(f"oracle probe shape: fields={len(value)} streams={'streams' in value} frames={'frames' in value} "
              f"programs_present={'programs' in value} programs_empty={value.get('programs') == []} "
              f"stream_groups_present={'stream_groups' in value} stream_groups_empty={value.get('stream_groups') == []} "
              f"unknown_fields={len(set(value) - known)}", file=sys.stderr)
    require(valid, "probe_shape")


def probe(value, native):
    probe_sections(value)
    require(type(value["streams"]) is list and len(value["streams"]) == 1, "probe_streams")
    stream = value["streams"][0]
    keys(stream, "codec_name width height pix_fmt color_range color_space color_transfer color_primaries")
    require(stream == {"codec_name": "h264", "width": 64, "height": 64,
                       "pix_fmt": "yuv420p", "color_range": "tv", "color_space": "bt709",
                       "color_transfer": "bt709", "color_primaries": "bt709"}, "probe_settings")
    frames = value["frames"]
    require(type(frames) is list and len(frames) == 4, "decoded_frame_count")
    for index, frame in enumerate(frames):
        require(type(frame) is dict and set(frame) <=
                {"media_type", "key_frame", "width", "height", "pix_fmt", "side_data_list"},
                "probe_frame")
        require(frame.get("media_type") == "video" and frame.get("width") == 64 and
                frame.get("height") == 64 and frame.get("pix_fmt") == "yuv420p" and
                type(frame.get("key_frame")) is int and frame["key_frame"] in (0, 1) and
                bool(frame["key_frame"]) == native["frames"][index]["sync"], "probe_frame")


def verify_capture(directory, oracle, mode, coreav_decoder, install=False, hosted_software_exemption=False):
    require(sys.platform == "linux" and platform.machine() == "x86_64", "oracle_platform")
    require(not hosted_software_exemption or mode == "software", "hosted_software_exemption_scope")
    directory, oracle = Path(directory), Path(oracle).resolve()
    capture_record = load(directory / "capture.json", 32768)
    keys(capture_record, "schema requested_mode exporter_sha256 runner_image os_build machine files")
    expected_schema = HOSTED_REFUSAL_CAPTURE_SCHEMA if hosted_software_exemption else "coreav.videotoolbox-capture/1"
    require(capture_record["schema"] == expected_schema and
            capture_record["requested_mode"] == mode, "capture_schema")
    from evidence import digest
    digest(capture_record["exporter_sha256"])
    token(capture_record["runner_image"])
    token(capture_record["os_build"])
    require(capture_record["machine"] == "arm64", "capture_machine")
    value, names = artifacts(directory, mode, True, hosted_software_exemption)
    verify(directory, capture_record["files"], names, LIMIT)
    if hosted_software_exemption:
        return {"schema": HOSTED_REFUSAL_VERIFICATION_SCHEMA, "outcome": HOSTED_REFUSAL_OUTCOME,
                "requested_mode": "software", "software_encode_exercised": False,
                "capture_sha256": file_hash(directory / "capture.json"), "scope": "hosted-refusal-only"}
    if value["status"] == "unavailable":
        return {"schema": "coreav.videotoolbox-smoke-verification/1", "status": "unavailable",
                "requested_mode": mode, "reason": value["unavailable_reason"],
                "capture_sha256": file_hash(directory / "capture.json")}
    with tempfile.TemporaryDirectory() as temporary:
        temporary = Path(temporary)
        if install:
            run(["bash", str(ROOT / "scripts/install-oracle-ffmpeg.sh"), str(oracle)],
                temporary, 1800, stdout="install-out", stderr="install-error")
        run([sys.executable, "-I", str(ROOT / "scripts/oracle_pin.py"), "--verify-bin", str(oracle)],
            temporary, 120, stdout="verify-out", stderr="verify-error")
        hashes = {name: file_hash(oracle / name) for name in ("ffmpeg", "ffprobe")}
        coreav_decoder = Path(coreav_decoder).resolve(strict=True)
        decoder_hash = file_hash(coreav_decoder)
        video = read(directory, "video.h264", LIMIT)
        require(sha(video) == capture_record["files"]["video.h264"]["sha256"], "file_hash")
        (temporary / "video.h264").write_bytes(video)
        run([str(oracle / "ffmpeg"), "-nostdin", "-v", "error", "-xerror", "-err_detect", "explode",
             "-max_alloc", "67108864", "-max_pixels", "4096", "-threads", "1", "-c:v", "h264",
             "-f", "h264", "-i", str(temporary / "video.h264"),
             "-map", "0:v:0", "-an", "-frames:v", "5", "-fps_mode", "passthrough", "-pix_fmt", "yuv420p",
             "-f", "rawvideo", str(temporary / "decoded.yuv")], temporary, 60,
            stdout="decode-out", stderr="decode-error")
        run([str(oracle / "ffprobe"), "-v", "error", "-max_alloc", "67108864", "-max_pixels", "4096",
             "-threads", "1", "-f", "h264",
             "-select_streams", "v:0", "-show_entries",
             "stream=codec_name,width,height,pix_fmt,color_range,color_space,color_transfer,color_primaries:"
             "frame=media_type,key_frame,width,height,pix_fmt", "-of", "json",
             str(temporary / "video.h264")], temporary, 60, stdout="probe.json", stderr="probe-error")
        run([str(coreav_decoder), "decode", "--input", str(temporary / "video.h264"),
             "--output", str(temporary / "coreav.yuv"), "--report", str(temporary / "coreav.json")],
            temporary, 60, stdout="coreav-out", stderr="coreav-error")
        decoded = load(temporary / "coreav.json", 32768)
        keys(decoded, "schema width height frames warnings")
        require(decoded["schema"] == "coreav.videotoolbox-decode/1" and
                all(type(decoded[k]) is int and decoded[k] == value for k, value in
                    (("width", 64), ("height", 64), ("frames", 4), ("warnings", 0))), "coreav_decode")
        raw = read(temporary, "decoded.yuv", 24576)
        require(raw == read(temporary, "coreav.yuv", 24576), "decoder_disagreement")
        pixels(raw)
        require(not read(temporary, "decode-error", 32768) and
                not read(temporary, "probe-error", 32768) and
                not read(temporary, "coreav-error", 32768), "decoder_error")
        probe(load(temporary / "probe.json", LIMIT), value)
        require(hashes == {name: file_hash(oracle / name) for name in hashes}, "tool_changed")
        require(file_hash(coreav_decoder) == decoder_hash, "tool_changed")
        verify(directory, capture_record["files"], names, LIMIT)
        return {"schema": "coreav.videotoolbox-smoke-verification/1", "status": "verified",
                "requested_mode": mode, "hardware": value["hardware"],
                "capture_sha256": file_hash(directory / "capture.json"), "oracle_sha256": hashes,
                "coreav_decoder_sha256": decoder_hash,
                "scope": "elementary-synthetic-only"}


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
    check.add_argument("--coreav-decoder", required=True, type=Path)
    check.add_argument("--install-oracle", action="store_true")
    check.add_argument("--report", required=True, type=Path)
    for sub in (cap, check):
        sub.add_argument("--mode", required=True, choices=("hardware", "software", "any"))
        sub.add_argument("--hosted-software-exemption", action="store_true")
    args = parser.parse_args()
    try:
        if args.command == "capture":
            capture(args.exporter, args.output, args.mode, args.runner_image, args.hosted_software_exemption)
            return 0
        result = verify_capture(args.capture, args.oracle_bin, args.mode, args.coreav_decoder,
                                args.install_oracle, args.hosted_software_exemption)
        write(args.report, result)
        if args.hosted_software_exemption:
            return 0 if result["schema"] == HOSTED_REFUSAL_VERIFICATION_SCHEMA and result["outcome"] == HOSTED_REFUSAL_OUTCOME else 1
        return 0 if result["status"] == "verified" or args.mode == "software" else 1
    except (Invalid, OSError, UnicodeError, ValueError) as error:
        if args.command == "verify":
            write(args.report, {"schema": "coreav.videotoolbox-smoke-verification/1", "status": "failed",
                                "diagnostic": "verification_failed"})
        diagnostic = error.code if isinstance(error, Invalid) and re.fullmatch(r"[a-z_]{1,64}", error.code) else "capture_or_verification_error"
        print(f"videotoolbox-smoke {args.mode}: failed ({diagnostic})", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
