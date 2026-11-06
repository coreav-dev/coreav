#!/usr/bin/env python3
"""Command-output/decoded-byte evidence for prepared raw I420 encoder behavior.

Capture preflights and fixtures on the registered Mac. Produce the complete
analysis bundle on Linux with the existing verified oracle and the independent
CoreAV decoder. Neither path accepts caller-supplied acceptance assertions.
"""

import argparse
import fcntl
from fractions import Fraction
import json
import mmap
import os
from pathlib import Path
import re
import shutil
import stat
import sys
import tempfile
import time
from types import SimpleNamespace

from evidence import Invalid, digest, file_hash, integer, keys, load, read, require, sha, write, write_bytes
import judging
import metrics
from observations import BASELINE_FILES, GATE_FILES, capture_identity, encoder_identifier, identity as baseline_identity, trace, validate_requested
from process import run
from registration import CLASSES, COLOUR, ROOT, SCOPE, cadence_hint, clock, coverage, covered_class, git, host_observation, input_hashes, preflight, raw_size, roster_coverage, source_registration, validate
import smoke
import timing

sys.path.insert(0, str(ROOT / "scripts"))
import test_oracle

ENCODED = 1024**3
RAW = 32 * 1024**3
SMALL = 16 * 1024**2
ANALYSIS_FILES = {"ffmpeg.yuv", "coreav.yuv", "coreav.json", "probe.json", "vmaf.json", "psnr.log", "ssim.log", "compare.json", "model.json"}


def records(directory, names, limits):
    result = {}
    for name in names:
        path = Path(directory) / name
        result[name] = {"sha256": file_hash(path, limit=limits[name], root=Path(directory)),
                        "bytes": path.stat().st_size}
    return result


def check_records(directory, value, limits):
    require(type(value) is dict and set(value) == set(limits), "retained_coverage")
    for name, record in value.items():
        keys(record, "sha256 bytes")
        digest(record["sha256"])
        integer(record["bytes"], 0, limits[name])
        path = Path(directory) / name
        require(file_hash(path, limit=limits[name], root=Path(directory)) == record["sha256"] and
                path.stat().st_size == record["bytes"], "retained_hash")


def copy_immutable(source, destination, resume):
    source, destination = Path(source), Path(destination)
    data = read(source.parent, source.name, SMALL)
    if destination.exists() or destination.is_symlink():
        require(resume and not destination.is_symlink() and data == read(destination.parent, destination.name, SMALL),
                "resume_evidence_changed")
    else:
        if destination.with_name(destination.name + ".tmp").exists():
            require(resume, "resume_required")
            remove_partial_write(destination)
        write_bytes(destination, data)
    return str(destination)


def remove_partial_write(path):
    temporary = path.with_name(path.name + ".tmp")
    if temporary.exists() or temporary.is_symlink():
        read(temporary.parent, temporary.name, SMALL)
        temporary.unlink()


def packet_ledger(data, item, payload_bytes):
    covered_class(item)
    require(len(data) <= SMALL, "packet_clock_bounds")
    try:
        lines = [line for line in data.decode("ascii").splitlines() if line.strip()]
    except UnicodeError:
        raise Invalid("packet_clock") from None
    require(len(lines) == item["frames"], "packet_clock_count")
    result = []
    for line in lines:
        fields = line.split()
        require(len(fields) == 4 and re.fullmatch(r"-?[0-9]+", fields[0]) is not None and
                re.fullmatch(r"[0-9]+/[0-9]+", fields[1]) is not None and
                re.fullmatch(r"-?[0-9]+", fields[2]) is not None and
                re.fullmatch(r"[0-9]+", fields[3]) is not None, "packet_clock")
        try:
            tick = Fraction(fields[1])
        except (ValueError, ZeroDivisionError):
            raise Invalid("packet_clock") from None
        require(tick > 0, "packet_clock")
        result.append({"pts_value": int(fields[0]), "dts_value": int(fields[2]),
                       "time_base": {"num": tick.numerator, "den": tick.denominator}, "bytes": int(fields[3])})
    judging.packet_facts(result, item, payload_bytes)
    return result


def parameter_sets(data):
    found = {}

    def store(start, end):
        require(end > start.end(), "annexb")
        kind = data[start.end()] & 31
        require(data[start.end()] & 128 == 0 and kind in (1, 5, 6, 7, 8, 9, 12), "annexb")
        if kind not in (7, 8):
            return
        require(end - start.end() <= 65536, "parameter_bounds")
        nal = bytes(data[start.end():end]).rstrip(b"\x00")
        require(kind not in found or found[kind] == nal, "parameter_changed")
        found[kind] = nal
    previous = None
    count = 0
    for marker in re.finditer(b"\x00\x00(?:\x00)*\x01", data):
        count += 1
        require(count <= 1800 * 4096 + 16 and (previous is not None or marker.start() == 0), "annexb")
        if previous is not None:
            store(previous, marker.start())
        previous = marker
    require(previous is not None, "annexb")
    store(previous, len(data))
    require(set(found) == {7, 8}, "parameter_sets")
    return found


def avcc(data):
    require(type(data) is list and 7 <= len(data) <= 131072 and
            all(type(b) is int and 0 <= b <= 255 for b in data), "avcc")
    data = bytes(data)
    require(data[0] == 1 and data[4] & 3 == 3 and data[5] & 31 == 1, "avcc")
    size = int.from_bytes(data[6:8], "big")
    require(size > 0 and 8 + size + 3 <= len(data), "avcc")
    sps = data[8:8 + size]
    at = 8 + size
    require(data[at] == 1, "avcc")
    size = int.from_bytes(data[at + 1:at + 3], "big")
    require(size > 0 and at + 3 + size <= len(data), "avcc")
    return {7: sps, 8: data[at + 3:at + 3 + size]}


def probe_facts(value, item):
    covered_class(item)
    smoke.probe_sections(value)
    require(type(value["streams"]) is list and len(value["streams"]) == 1, "probe_streams")
    stream = value["streams"][0]
    expected = {"codec_name": "h264", "profile": "High", "has_b_frames": 0, "width": item["width"], "height": item["height"],
                "pix_fmt": "yuv420p", "color_range": "tv", "color_space": "bt709",
                "color_transfer": "bt709", "color_primaries": "bt709"}
    require(type(stream) is dict and set(stream) == set(expected) | {"level", "time_base", "r_frame_rate", "extradata", "extradata_size"},
            "probe_shape")
    require(all(type(stream[k]) is type(v) and stream[k] == v for k, v in expected.items()), "probe_settings")
    require(type(stream["time_base"]) is str and
            re.fullmatch(r"[0-9]+/[0-9]+", stream["time_base"]) is not None, "stream_clock")
    try:
        tick = Fraction(stream["time_base"])
    except (ValueError, ZeroDivisionError):
        raise Invalid("stream_clock") from None
    require(tick > 0 and type(stream["extradata"]) is str, "stream_clock")
    require(type(stream["r_frame_rate"]) is str and re.fullmatch(r"[0-9]+/[0-9]+", stream["r_frame_rate"]), "stream_clock")
    try:
        advertised = Fraction(stream["r_frame_rate"])
    except (ValueError, ZeroDivisionError):
        raise Invalid("stream_clock") from None
    require(advertised >= 0, "stream_clock")
    integer(stream["level"], 1, 255)
    encoded = bytearray()
    for line in stream["extradata"].splitlines():
        if not line:
            continue
        match = re.fullmatch(r"([0-9a-fA-F]{8}): (.*)", line)
        require(match is not None and int(match[1], 16) == len(encoded), "probe_extradata")
        hexadecimal = match[2].split("  ", 1)[0].replace(" ", "")
        require(re.fullmatch(r"(?:[0-9a-fA-F]{2})+", hexadecimal) is not None, "probe_extradata")
        encoded.extend(bytes.fromhex(hexadecimal))
    require(type(stream["extradata_size"]) is int and stream["extradata_size"] == len(encoded) and
            len(encoded) <= 131072, "probe_extradata")
    parameters = parameter_sets(bytes(encoded)) if bytes(encoded).startswith(b"\0\0") else avcc(list(encoded))
    require(len(parameters[7]) >= 4 and parameters[7][1] == 100 and
            parameters[7][3] == stream["level"], "probe_settings")
    frames = value["frames"]
    require(type(frames) is list and len(frames) == item["frames"], "decoded_frame_count")
    sync = []
    for index, frame in enumerate(frames):
        require(type(frame) is dict and set(frame) <= {"media_type", "key_frame", "width", "height",
                "pix_fmt", "pict_type", "duration", "pkt_duration", "side_data_list"}, "probe_frame")
        require(frame.get("media_type") == "video" and frame.get("width") == item["width"] and
                frame.get("height") == item["height"] and frame.get("pix_fmt") == "yuv420p" and
                frame.get("pict_type") in ("I", "P") and type(frame.get("key_frame")) is int and
                frame["key_frame"] in (0, 1), "probe_frame")
        durations = [frame[key] for key in ("duration", "pkt_duration") if key in frame]
        require(durations and all(type(n) is int and n > 0 and n == durations[0]
                for n in durations), "stream_clock")
        sync.append(bool(frame["key_frame"]))
    require(sync[0], "keyframe")
    return sync, parameters, {"demux_time_base_diagnostic": {"num": tick.numerator, "den": tick.denominator},
                              "advertised_rate_diagnostic": {"num": advertised.numerator, "den": advertised.denominator},
                              "level": stream["level"]}


def native_facts(value, item, cell):
    covered_class(item)
    keys(value, "schema status requested_mode width height fps_num fps_den bitrate submitted access_unit_bytes "
         "encoder_id hardware frames avcc settings drain_complete")
    require(value["schema"] == "coreav.videotoolbox-encode/1" and value["status"] == "encoded" and
            value["requested_mode"] == cell and value["hardware"] == cell and value["drain_complete"] is True,
            "native_completion")
    for key, expected in (("width", item["width"]), ("height", item["height"]),
                          ("fps_num", item["rate"]["num"]), ("fps_den", item["rate"]["den"]),
                          ("bitrate", dict(CLASSES)[item["class"]]), ("submitted", item["frames"])):
        require(type(value[key]) is int and value[key] == expected, "native_settings")
    encoder_identifier(value["encoder_id"])
    expected = {"profile": "high", "reordering": False, "realtime": False, "keyframe_seconds": 2,
                "expected_frame_rate_hint": cadence_hint(item),
                "keyframe_frames": 2 * item["rate"]["num"] // item["rate"]["den"], "pixel_format": "i420",
                "native_pixel_format": "420v", "color_range": "limited", "primaries": "bt709",
                "transfer": "bt709", "matrix": "bt709"}
    require(value["settings"] == expected and all(type(value["settings"][k]) is type(v)
            for k, v in expected.items()), "native_settings")
    require(type(value["frames"]) is list and len(value["frames"]) == item["frames"], "native_frames")
    integer(value["access_unit_bytes"], 1, ENCODED)
    pts, sync = [], []
    for index, frame in enumerate(value["frames"]):
        keys(frame, "id pts_value pts_scale duration_value duration_scale sync")
        require(type(frame["id"]) is int and frame["id"] == index and type(frame["sync"]) is bool,
                "frame_identity")
        for key in ("pts_value", "pts_scale", "duration_value", "duration_scale"):
            integer(frame[key], 0 if key == "pts_value" else 1, 2**63 - 1)
        position = Fraction(frame["pts_value"], frame["pts_scale"])
        require(abs(position - Fraction(index, 1) / clock(item["rate"])) <= clock(item["source_tick"]) / 2 and
                Fraction(frame["duration_value"], frame["duration_scale"]) == 1 / clock(item["rate"]), "clock")
        pts.append(position)
        sync.append(frame["sync"])
    return pts, sync, avcc(value["avcc"])


def decoder_report(value, item):
    keys(value, "schema width height frames warnings")
    require(value["schema"] == "coreav.videotoolbox-decode/1" and
            all(type(value[key]) is int and value[key] == expected for key, expected in
                (("width", item["width"]), ("height", item["height"]),
                 ("frames", item["frames"]), ("warnings", 0))), "coreav_decode")


def decode(video, item, directory, ffmpeg, ffprobe, decoder):
    covered_class(item)
    area = ((item["width"] + 15) // 16 * 16) * ((item["height"] + 15) // 16 * 16)
    run([str(ffmpeg), "-nostdin", "-v", "error", "-xerror", "-err_detect", "explode",
         "-max_alloc", "67108864", "-max_pixels", str(area), "-threads", "1", "-c:v", "h264",
         "-f", "h264", "-i", str(video), "-map", "0:v:0", "-an", "-frames:v", str(item["frames"] + 1),
         "-fps_mode", "passthrough", "-pix_fmt", "yuv420p", "-f", "rawvideo", str(directory / "ffmpeg.yuv")],
        directory, 1800, stdout="ffmpeg-out", stderr="ffmpeg-error")
    run([str(decoder), "decode", "--input", str(video), "--output", str(directory / "coreav.yuv"),
         "--report", str(directory / "coreav.json")], directory, 1800,
        stdout="coreav-out", stderr="coreav-error")
    run([str(ffprobe), "-v", "error", "-max_alloc", "67108864", "-max_pixels", str(area), "-threads", "1",
         "-f", "h264", "-select_streams", "v:0", "-show_data", "-show_entries",
         "stream=codec_name,profile,level,has_b_frames,width,height,pix_fmt,color_range,color_space,color_transfer,color_primaries,"
         "time_base,r_frame_rate,extradata,extradata_size:frame=media_type,key_frame,width,height,pix_fmt,pict_type,duration,pkt_duration",
         "-of", "json", str(video)], directory, 1800, stdout="probe.json", stderr="probe-error")
    require(not read(directory, "ffmpeg-error", SMALL) and not read(directory, "coreav-error", SMALL) and
            not read(directory, "probe-error", SMALL), "decoder_error")
    require((directory / "coreav.json").is_file() and (directory / "probe.json").is_file(), "decoder_missing_output")
    decoder_report(load(directory / "coreav.json"), item)
    extent = raw_size(item)
    a, b = directory / "ffmpeg.yuv", directory / "coreav.yuv"
    require(a.is_file() and b.is_file(), "decoder_missing_output")
    require(a.stat().st_size == b.stat().st_size == extent and
            file_hash(a, extent) == file_hash(b, extent), "decoder_disagreement")


def capture_metrics(source, decoded, item, directory, ffmpeg, model, decoder):
    run([str(decoder), "compare", "--source", str(source), "--decoded", str(decoded), "--width", str(item["width"]),
         "--height", str(item["height"]), "--frames", str(item["frames"]), "--report", str(directory / "compare.json")],
        directory, 1800, stdout="compare-out", stderr="compare-error")
    require(not read(directory, "compare-error", SMALL), "compare_command")
    require((directory / "compare.json").is_file(), "metric_missing_output")
    shutil.copyfile(model, directory / "model.json")
    raw = ["-f", "rawvideo", "-pixel_format", "yuv420p", "-video_size", f'{item["width"]}x{item["height"]}',
           "-framerate", f'{item["rate"]["num"]}/{item["rate"]["den"]}']
    graph = ("[0:v]split=3[dv][dp][ds];[1:v]split=3[rv][rp][rs];"
             "[dv][rv]libvmaf=model=path=model.json:log_fmt=json:log_path=vmaf.json:n_threads=1[v];"
             "[dp][rp]psnr=stats_file=psnr.log:stats_version=2:output_max=1[p];"
             "[ds][rs]ssim=stats_file=ssim.log[s]")
    run([str(ffmpeg), "-nostdin", "-v", "error", "-xerror", "-filter_complex_threads", "1",
         *raw, "-i", str(decoded), *raw, "-i", str(source), "-filter_complex", graph,
         "-map", "[v]", "-map", "[p]", "-map", "[s]", "-an", "-f", "null", "-"],
        directory, 3600, stdout="metrics-out", stderr="metrics-error", cwd=directory)
    require(not read(directory, "metrics-error", SMALL), "metric_command")
    require(all((directory / name).is_file() for name in ("compare.json", "vmaf.json", "psnr.log", "ssim.log")),
            "metric_missing_output")


def output_limits(item, side, failed=False):
    covered_class(item)
    limits = {"video.h264": ENCODED, "analysis-owner.json": SMALL}
    if failed:
        limits["failure.json"] = SMALL
    limits.update({"native.json": SMALL} if side == "coreav" else
                  {"packets.stats": SMALL, **BASELINE_FILES})
    limits.update({"analysis/" + name: raw_size(item) if name.endswith(".yuv") else SMALL
                   for name in ANALYSIS_FILES})
    return limits


def output_facts(directory, source, item, cell, side, reg, observability):
    roster_coverage(reg)
    covered_class(item)
    directory = Path(directory)
    require(not directory.is_symlink() and {p.name for p in directory.iterdir()} ==
            (({"video.h264", "native.json", "analysis", "analysis-owner.json"} if side == "coreav" else
              ({"video.h264", "packets.stats", "analysis", "analysis-owner.json"} | set(BASELINE_FILES))) |
             ({"failure.json"} if (directory / "failure.json").exists() else set())), "retained_coverage")
    analysis = directory / "analysis"
    require(not analysis.is_symlink() and {p.name for p in analysis.iterdir()} == ANALYSIS_FILES, "retained_coverage")
    limits = output_limits(item, side, (directory / "failure.json").exists())
    retained = records(directory, limits, limits)
    require(retained["analysis/model.json"]["sha256"] == reg["metric_tools"]["model"], "metric_model")
    require(source.stat().st_size == raw_size(item) and
            file_hash(source, raw_size(item)) == item["raw_sha256"], "input_hash")
    decoder_report(load(analysis / "coreav.json"), item)
    sync, described_parameters, stream_clock = probe_facts(load(analysis / "probe.json", SMALL), item)
    with (directory / "video.h264").open("rb") as encoded:
        with mmap.mmap(encoded.fileno(), 0, access=mmap.ACCESS_READ) as data:
            parameters = parameter_sets(data)
    require(parameters == described_parameters, "parameter_sets")
    baseline, ledger, packet_text, requests = None, None, None, None
    if side == "coreav":
        native = load(directory / "native.json", SMALL)
        pts, native_sync, native_parameters = native_facts(native, item, cell)
        require(native_sync == sync and native_parameters == parameters, "native_configuration")
        require(native["access_unit_bytes"] + 8 + len(parameters[7]) + len(parameters[8]) ==
                retained["video.h264"]["bytes"], "native_payload_size")
        encoder_id = native["encoder_id"]
    else:
        observed = baseline_identity(directory, reg, cell, observability, retained["video.h264"]["sha256"])
        encoder_id = observed["encoder_id"]
        baseline = {"query": load(directory / "identity-query.json", SMALL), "identity": observed}
        requests = load(directory / "requested.json", SMALL)
        validate_requested(requests, item, reg, cell)
        packet_data = read(directory, "packets.stats", SMALL)
        ledger = packet_ledger(packet_data, item, retained["video.h264"]["bytes"])
        packet_text = packet_data.decode("ascii")
        pts = [Fraction(row["pts_value"]) * clock(row["time_base"]) for row in ledger]
    logs = metrics.metric_logs(read(analysis, "vmaf.json", SMALL), read(analysis, "psnr.log", SMALL),
                               read(analysis, "ssim.log", SMALL), item["frames"])
    decoded_hashes, metric_frames = metrics.frames(source, analysis / "ffmpeg.yuv", item, logs,
                                                   read(analysis, "compare.json", SMALL), analysis / "coreav.yuv")
    sync_positions = [index for index, value in enumerate(sync) if value]
    gaps = [b - a for a, b in zip(sync_positions, sync_positions[1:] + [item["frames"]])]
    maximum = max(gaps)
    duration = 1 / clock(item["rate"])
    maximum_seconds = max((pts[b] if b < item["frames"] else pts[-1] + duration) - pts[a]
                          for a, b in zip(sync_positions, sync_positions[1:] + [item["frames"]]))
    result = {"schema": "coreav.videotoolbox-output-evidence/2", "input_id": item["id"], "cell": cell,
              "scope": SCOPE, "class": item["class"],
              "side": side, "output_sha256": retained["video.h264"]["sha256"], "raw_sha256": item["raw_sha256"],
              "producer_sha256": reg["metric_tools"]["evidence_producer"], "width": item["width"],
              "height": item["height"], "pixel_format": "yuv420p", "colour": dict(COLOUR), "profile": "high",
              "level": stream_clock.pop("level"), "baseline": baseline, "requested_settings": requests,
              "packet_ledger": ledger, "packet_text": packet_text,
              "reordering": False, "keyframe_max_frames": maximum,
              "stream_clock": {**stream_clock, "source_rate": dict(item["rate"]),
                               "provenance": "native_report" if side == "coreav" else "packet_ledger",
                               "duration_provenance": "original_source_rational"},
              "keyframe_max_seconds": float(maximum_seconds), "encoder_id": encoder_id, "hardware": cell,
              "frames": [{"id": index, "source_sha256": item["source_frame_sha256"][index],
                          "decoded_sha256": decoded_hashes[index],
                          "pts": {"value": position.numerator, "scale": position.denominator},
                          "duration": {"value": duration.numerator, "scale": duration.denominator}, "sync": sync[index]}
                         for index, position in enumerate(pts)],
              "audio_offset_error": None, "audio_applicability": "not_applicable_raw_driver",
              "parameter_sets_agree": True, "sample_description_applicability": "not_applicable_elementary",
              "payload_bytes": retained["video.h264"]["bytes"], "container_bytes": None,
              "decoders": {name: {"sha256": reg["metric_tools"]["ffmpeg" if name == "ffmpeg" else "coreav_decoder"],
                                  "status": "complete", "frame_sha256": decoded_hashes, "bytes": raw_size(item)}
                           for name in ("ffmpeg", "coreav")},
              "metrics": {"tool_sha256": reg["metric_tools"]["metrics"], "model_sha256": reg["metric_tools"]["model"],
                          "model": "vmaf_v0.6.1", "resolution": "native", "alignment": "fixed_presentation",
                          "psnr_planes": "all", "ssim_component": "All", "frames": metric_frames},
              "retained": retained}
    judging.conformance(result, item, cell, side, result["output_sha256"], reg)
    check_records(directory, retained, limits)
    return result


def verify_output(proof, directory, source, item, cell, side, reg, observability, args):
    directory = Path(directory)
    check_records(directory, proof["retained"], output_limits(item, side, (directory / "failure.json").exists()))
    owner = load(directory / "analysis-owner.json", SMALL)
    keys(owner, "schema binding status history files")
    validate_history(owner["history"])
    require(owner["schema"] == "coreav.videotoolbox-analysis-unit/1" and owner["status"] == "complete" and
            owner["binding"] == unit_binding(directory, item, cell, side, reg, args), "analysis_incomplete")
    limits = output_limits(item, side, (directory / "failure.json").exists())
    limits.pop("analysis-owner.json")
    check_records(directory, owner["files"], limits)
    require(output_facts(directory, source, item, cell, side, reg, observability) == proof, "output_evidence_changed")


def unit_binding(directory, item, cell, side, reg, args):
    covered_class(item)
    inputs = {"video.h264": ENCODED, "native.json": SMALL} if side == "coreav" else {
        "video.h264": ENCODED, "packets.stats": SMALL, **BASELINE_FILES}
    return {"registration_sha256": file_hash(args.registration, SMALL), "capture_sha256": file_hash(args.capture, SMALL),
            "input_id": item["id"], "cell": cell, "side": side, "raw_sha256": item["raw_sha256"],
            "source_frames_sha256": sha(json.dumps(item["source_frame_sha256"]).encode()),
            "tools_sha256": sha(json.dumps({"sources": reg["tools"], "tools": reg["metric_tools"]}, sort_keys=True).encode()),
            "capture_files": records(directory, inputs, inputs)}


def validate_history(value):
    require(type(value) is list and len(value) <= 1000, "resume_history")
    for row in value:
        keys(row, "status diagnostic")
        require(row["status"] in ("failed", "incomplete") and row["diagnostic"] in {
            "resumed_incomplete_unit", "infrastructure_abort", "analysis_failed", "resumed_interrupted_campaign"},
            "resume_history")


def analyse(directory, source, item, cell, side, reg, args, observability, oracle):
    directory = Path(directory)
    analysis = directory / "analysis"
    binding = unit_binding(directory, item, cell, side, reg, args)
    marker = directory / "analysis-owner.json"
    history = []
    if marker.exists():
        require(args.resume and not marker.is_symlink(), "resume_required")
        owner = load(marker, SMALL)
        keys(owner, "schema binding status history files")
        require(owner["schema"] == "coreav.videotoolbox-analysis-unit/1" and owner["binding"] == binding,
                "resume_binding")
        history = owner["history"]
        validate_history(history)
        if owner["status"] == "complete":
            limits = output_limits(item, side, (directory / "failure.json").exists())
            limits.pop("analysis-owner.json")
            check_records(directory, owner["files"], limits)
            return output_facts(directory, source, item, cell, side, reg, observability)
        require(owner["status"] == "incomplete", "analysis_failed_final")
        history = history + [{"status": "incomplete", "diagnostic": "resumed_incomplete_unit"}]
        remove_partial_write(marker)
        remove_partial_write(directory / "failure.json")
        if analysis.exists():
            allowed = ANALYSIS_FILES | {"ffmpeg-out", "ffmpeg-error", "coreav-out", "coreav-error", "probe-error",
                                        "metrics-out", "metrics-error", "compare-out", "compare-error"}
            require(not analysis.is_symlink() and all(path.is_file() and not path.is_symlink() and path.name in allowed
                    for path in analysis.iterdir()), "resume_unowned_path")
            for path in analysis.iterdir():
                path.unlink()
            analysis.rmdir()
    else:
        require(not analysis.exists() and not (directory / "failure.json").exists(), "resume_unowned_path")
        if marker.with_name(marker.name + ".tmp").exists():
            require(args.resume and hasattr(args, "output") and directory.parent == args.artifacts,
                    "resume_unowned_path")
            campaign = load(args.output / "campaign.json", SMALL)
            keys(campaign, "schema binding")
            require(campaign["schema"] == "coreav.videotoolbox-campaign/1" and
                    campaign["binding"] == campaign_binding(args, reg), "resume_binding")
            remove_partial_write(marker)
    owner = {"schema": "coreav.videotoolbox-analysis-unit/1", "binding": binding,
             "status": "incomplete", "history": history, "files": {}}
    write(marker, owner)
    analysis.mkdir(exist_ok=False)
    try:
        decode(directory / "video.h264", item, analysis, oracle.ffmpeg,
               oracle.ffprobe, args.coreav_decoder)
        capture_metrics(source, analysis / "ffmpeg.yuv", item, analysis, oracle.ffmpeg,
                        args.model, args.coreav_decoder)
        for path in analysis.iterdir():
            if path.name not in ANALYSIS_FILES:
                path.unlink()
        result = output_facts(directory, source, item, cell, side, reg, observability)
        owner["status"] = "complete"
        limits = output_limits(item, side, (directory / "failure.json").exists())
        limits.pop("analysis-owner.json")
        owner["files"] = records(directory, limits, limits)
        for name in limits:
            with (directory / name).open("rb") as retained_file:
                os.fsync(retained_file.fileno())
        write(marker, owner)
        result["retained"]["analysis-owner.json"] = records(directory, ["analysis-owner.json"],
                                                              {"analysis-owner.json": SMALL})["analysis-owner.json"]
        return result
    except BaseException as error:
        retryable = isinstance(error, (OSError, KeyboardInterrupt)) or isinstance(error, Invalid) and error.code in {
            "interrupted", "process_infrastructure"}
        owner["status"] = "incomplete" if retryable else "failed"
        owner["history"].append({"status": "failed", "diagnostic": "infrastructure_abort" if retryable else "analysis_failed"})
        write(marker, owner)
        for path in analysis.iterdir():
            if path.is_file() and path.name.endswith(("-out", "-error")):
                path.unlink()
        write(directory / "failure.json", {"schema": "coreav.videotoolbox-analysis-failure/1",
                                           "status": "failed", "diagnostic": "analysis_failed"})
        raise


def unit_item():
    return {"id": "preflight", "class": 360, "width": 64, "height": 64, "frames": 4,
            "rate": {"num": 30, "den": 1}, "source_tick": {"num": 1, "den": 30},
            "colour": dict(COLOUR), "raw_sha256": sha(smoke.source()),
            "source_frame_sha256": [sha(smoke.source()[n * 6144:(n + 1) * 6144]) for n in range(4)]}


def preflight_limits(files, encoded, side):
    expected = {"command.json": SMALL, "host.json": SMALL}
    if side == "coreav":
        expected.update({"manifest.json": SMALL, "capture.json": SMALL})
    else:
        expected.update({"observations.txt": SMALL, "source.yuv": 24576})
    if encoded:
        expected.update({"source.yuv": 24576, "video.h264": 4 * 1024**2,
                         "analysis/ffmpeg.yuv": 24576, "analysis/coreav.yuv": 24576,
                         "analysis/coreav.json": SMALL, "analysis/probe.json": SMALL})
        if side == "ffmpeg":
            expected["packets.stats"] = SMALL
            expected.update(BASELINE_FILES)
    require(set(files) == set(expected), "preflight_coverage")
    return expected


def native_preflight_manifest(directory, cell):
    value = load(directory / "manifest.json")
    require(type(value) is dict, "native_preflight_manifest")
    require(value.get("schema") != smoke.HOSTED_REFUSAL_SCHEMA, "hosted_refusal_is_not_availability")
    return value, smoke.manifest(value, cell)


def preflight_facts(directory, cell, side, reg, observability):
    command = load(directory / "command.json")
    keys(command, "schema status ended_ns")
    require(command["schema"] == "coreav.videotoolbox-command/1" and
            command["status"] in ("success", "child_failure"), "preflight_command")
    integer(command["ended_ns"], 1, 2**63 - 1)
    host = load(directory / "host.json")
    require(host == reg["host"], "host_mismatch")
    unit = unit_item()
    identity, hardware, reason = None, "unknown", None
    if side == "coreav":
        value, names = native_preflight_manifest(directory, cell)
        native_capture = load(directory / "capture.json")
        keys(native_capture, "schema requested_mode exporter_sha256 runner_image os_build machine files")
        require(native_capture["schema"] == "coreav.videotoolbox-capture/1" and
                native_capture["requested_mode"] == cell and native_capture["machine"] == "arm64" and
                native_capture["runner_image"] == reg["host"]["profile"] and
                native_capture["os_build"] == reg["host"]["os_build"] and native_capture["exporter_sha256"] ==
                reg["executables"]["coreav"]["sha256"], "preflight_binding")
        from evidence import verify
        verify(directory, native_capture["files"], names, 4 * 1024**2)
        require(command["status"] == "success", "preflight_command")
        encoded = value["status"] == "encoded"
        identity, hardware, reason = value["encoder_id"], value["hardware"], value["unavailable_reason"]
    else:
        encoded = command["status"] == "success"
        if encoded:
            value = baseline_identity(directory, reg, cell, observability, file_hash(directory / "video.h264", ENCODED),
                                      command["ended_ns"])
            identity, hardware = value["encoder_id"], value["hardware"]
            validate_requested(load(directory / "requested.json"), unit, reg, cell)
            packet_ledger(read(directory, "packets.stats", SMALL), unit, (directory / "video.h264").stat().st_size)
        else:
            value, _ = trace(read(directory, "observations.txt", SMALL), observability, failed=True)
            require(command["status"] == "child_failure" and value["submitted"] == 0,
                    "preflight_command")
            reason = value["status"]
    require(encoded or cell == "software", "required_hardware_unavailable")
    paths = {p.relative_to(directory).as_posix() for p in directory.rglob("*") if not p.is_dir()}
    limits = preflight_limits(paths, encoded, side)
    require(not directory.is_symlink() and all(not p.is_symlink() for p in directory.rglob("*")) and
            {p.relative_to(directory).as_posix() for p in directory.rglob("*")} ==
            set(limits) | ({"analysis"} if encoded else set()), "preflight_coverage")
    check_records(directory, records(directory, paths, limits), limits)
    if encoded:
        require(read(directory, "source.yuv", 24576) == smoke.source(), "source_pattern")
        analysis = directory / "analysis"
        decoder_report(load(analysis / "coreav.json"), unit)
        sync, parameters, _ = probe_facts(load(analysis / "probe.json"), unit)
        video = read(directory, "video.h264", 4 * 1024**2)
        require(parameter_sets(video) == parameters, "parameter_sets")
        if side == "coreav":
            require(sync == [frame["sync"] for frame in value["frames"]], "sync")
        a, b = read(analysis, "ffmpeg.yuv", 24576), read(analysis, "coreav.yuv", 24576)
        require(a == b, "decoder_disagreement")
        smoke.pixels(a)
    else:
        require(not (directory / "video.h264").exists() and not (directory / "analysis").exists(), "availability")
    return {"status": "encoded" if encoded else "unavailable", "submitted": 4 if encoded else 0,
            "reason": reason, "binary_sha256": reg["executables"][side]["sha256"],
            "hardware": hardware, "encoder_id": identity, "decoded_frames": 4 if encoded else 0,
            "pattern_identity": encoded, "drain_complete": encoded}, limits


def verify_preflight(proof, directory, cell, side, reg, observability):
    keys(proof, "schema registration_sha256 producer_sha256 settings_sha256 result files covered_classes uncovered_classes")
    require(proof["schema"] == "coreav.videotoolbox-preflight/2" and coverage(proof) == coverage(reg), "class_coverage")
    encoded = proof["result"]["status"] == "encoded"
    limits = preflight_limits(proof["files"], encoded, side)
    check_records(directory, proof["files"], limits)
    facts, _ = preflight_facts(directory, cell, side, reg, observability)
    require(facts == proof["result"], "preflight_changed")


def capture_preflights(args):
    ledger = {"schema": "coreav.videotoolbox-producer-failure/1", "status": "failed",
              "diagnostic": "preflight_incomplete"}
    args.output.mkdir(exist_ok=False)
    write(args.output / "producer-status.json", ledger)
    try:
        reg, oracle = preflight(args.registration, args.root, args.ffprobe)
        require(file_hash(Path(__file__).resolve()) == reg["metric_tools"]["evidence_producer"], "producer_changed")
        reg_hash = file_hash(args.registration)
        spec = load(args.root / reg["baseline_build"]["file"])["observability"]
        ledger = {"schema": "coreav.videotoolbox-availability/2", **roster_coverage(reg), "session_id": reg["session_id"],
                  "registration_sha256": reg_hash, "cells": {"hardware": {}, "software": {}}}
        native = args.root / reg["executables"]["coreav"]["file"]
        baseline = oracle.ffmpeg
        environment = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LANG": "C", "LC_ALL": "C", **reg["environment"]}
        probe = oracle.ffprobe
        build = load(args.root / reg["baseline_build"]["file"])
        require(file_hash(probe) == build["ffprobe_sha256"], "baseline_probe_changed")
        for cell in ("hardware", "software"):
            for side in ("coreav", "ffmpeg"):
                timing.host_state(reg)
                directory = args.output / f"preflight.{cell}.{side}.artifacts"
                status = "success"
                if side == "coreav":
                    smoke.capture(native, directory, cell, "ENC-MAC-1")
                    encoder_ended_ns = time.time_ns()
                    value, _ = native_preflight_manifest(directory, cell)
                    encoded = value["status"] == "encoded"
                else:
                    directory.mkdir()
                    (directory / "source.yuv").write_bytes(smoke.source())
                    unit = unit_item()
                    unit["raw_file"] = (directory / "source.yuv").relative_to(args.root).as_posix() \
                        if directory.is_relative_to(args.root) else "unused"
                    command = timing.commands(reg, unit, cell, args.root, directory / "video.h264",
                                              directory / "packets.stats", oracle)["ffmpeg"]
                    command[command.index("-i") + 1] = str(directory / "source.yuv")
                    command[command.index("-b:v") + 1] = "2000000"
                    try:
                        run(command, directory, 120, env=environment)
                        encoder_ended_ns = time.time_ns()
                    except Invalid as error:
                        require(error.code == "child_failure", error.code)
                        status = "child_failure"
                        encoder_ended_ns = time.time_ns()
                    encoded = status == "success"
                    if encoded:
                        capture_identity(directory, reg, args.root, spec, encoder_ended_ns, command, unit, cell)
                    else:
                        _, kept = trace(read(directory, "stderr", SMALL), spec, failed=True)
                        (directory / "observations.txt").write_bytes(kept)
                    for name in ("stdout", "stderr"):
                        (directory / name).unlink(missing_ok=True)
                    if not encoded:
                        for name in ("video.h264", "packets.stats"):
                            path = directory / name
                            require(not path.exists() or path.stat().st_size == 0, "availability_after_input")
                            path.unlink(missing_ok=True)
                write(directory / "host.json", reg["host"])
                write(directory / "command.json", {"schema": "coreav.videotoolbox-command/1", "status": status,
                                                    "ended_ns": encoder_ended_ns})
                if encoded:
                    analysis = directory / "analysis"
                    analysis.mkdir()
                    decode(directory / "video.h264", unit_item(), analysis, baseline, probe, native)
                    for path in analysis.iterdir():
                        if path.name not in {"ffmpeg.yuv", "coreav.yuv", "coreav.json", "probe.json"}:
                            path.unlink()
                facts, limits = preflight_facts(directory, cell, side, reg, spec)
                proof = {"schema": "coreav.videotoolbox-preflight/2", **coverage(reg), "registration_sha256": reg_hash,
                         "producer_sha256": reg["metric_tools"]["evidence_producer"],
                         "settings_sha256": timing.settings_hash(reg), "result": facts,
                         "files": records(directory, limits, limits)}
                path = args.output / f"preflight.{cell}.{side}.json"
                write(path, proof)
                ledger["cells"][cell][side] = {**facts, "smoke_sha256": file_hash(path)}
                timing.host_state(reg)
        timing.availability(ledger, reg)
        require(file_hash(args.registration) == reg_hash, "registration_changed")
        preflight(args.registration, args.root, args.ffprobe)
        write(args.output / "availability.json", ledger)
        (args.output / "producer-status.json").unlink()
    except BaseException as error:
        for path in args.output.rglob("*"):
            if path.is_file() and path.name in {"stdout", "stderr", "ffmpeg-error", "coreav-error", "probe-error"}:
                path.unlink()
        diagnostic = error.code if isinstance(error, Invalid) and error.code in {
            "ffmpeg_observability_missing", "ffmpeg_observation_changed",
            "hosted_refusal_is_not_availability",
            "judged_event", "timeout", "child_crash", "child_failure", "decoder_disagreement", "host_fault"} else "preflight_failed"
        write(args.output / "failure.json", {"schema": "coreav.videotoolbox-producer-failure/1",
                                           "status": "failed", "diagnostic": diagnostic, "retained": ledger})
        raise


def capture_fixtures(args):
    report = {"schema": "coreav.videotoolbox-fixture-capture/2", "scope": SCOPE,
              "covered_classes": None, "uncovered_classes": None, "status": "failed", "records": []}
    write(args.report, report)
    try:
        reg, oracle = preflight(args.registration, args.root)
        report.update(roster_coverage(reg))
        reg_hash = file_hash(args.registration)
        require(file_hash(Path(__file__).resolve()) == reg["metric_tools"]["evidence_producer"], "producer_changed")
        available = timing.availability_file(args.availability, reg, args.root)
        require(available["registration_sha256"] == reg_hash, "availability_binding")
        captured = load(args.capture, SMALL)
        require(captured["schema"] == "coreav.videotoolbox-timing/2" and captured["scope"] == SCOPE and
                coverage(captured) == coverage(reg) and
                captured["status"] == "awaiting_evidence" and captured["registration_sha256"] == reg_hash,
                "capture_status")
        judging.timings(captured["records"], reg)
        input_hashes(args.root, reg, fixtures=True)
        spec = load(args.root / reg["baseline_build"]["file"])["observability"]
        environment = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LANG": "C", "LC_ALL": "C", **reg["environment"]}
        for cell in reg["cells"]:
            for fixture, item in reg["fixtures"].items():
                for side in ("coreav", "ffmpeg"):
                    timing.host_state(reg)
                    directory = args.artifacts / f"fixture.{cell}.{fixture}.{side}"
                    directory.mkdir(exist_ok=False)
                    record = {"fixture": fixture, "cell": cell, "side": side, "status": "failed"}
                    report["records"].append(record)
                    try:
                        command = timing.commands(reg, item, cell, args.root, directory / "video.h264",
                                                  directory / ("native.json" if side == "coreav" else "packets.stats"), oracle)[side]
                        run(command, directory, 300, env=environment)
                        encoder_ended_ns = time.time_ns()
                        record["encoder_ended_ns"] = encoder_ended_ns
                        require(0 < (directory / "video.h264").stat().st_size <= ENCODED, "output_bounds")
                        file_hash(directory / "video.h264", ENCODED)
                        if side == "ffmpeg":
                            capture_identity(directory, reg, args.root, spec, encoder_ended_ns, command, item, cell)
                            packet_ledger(read(directory, "packets.stats", SMALL), item,
                                          (directory / "video.h264").stat().st_size)
                        else:
                            native_facts(load(directory / "native.json", SMALL), item, cell)
                        timing.host_state(reg)
                        limits = {"video.h264": ENCODED, "native.json": SMALL} if side == "coreav" else {
                            "video.h264": ENCODED, "packets.stats": SMALL, **BASELINE_FILES}
                        record["files"] = records(directory, limits, limits)
                        record["status"] = "success"
                    finally:
                        for name in ("stdout", "stderr"):
                            (directory / name).unlink(missing_ok=True)
                        write(args.report, report)
        input_hashes(args.root, reg, fixtures=True)
        require(file_hash(args.registration) == reg_hash, "registration_changed")
        preflight(args.registration, args.root)
        report.update(status="complete", registration_sha256=reg_hash, capture_sha256=file_hash(args.capture))
    finally:
        write(args.report, report)


def verify_fixture_capture(path, reg, captured_hash, artifacts):
    report = load(path, SMALL)
    keys(report, "schema status records registration_sha256 capture_sha256 covered_classes uncovered_classes scope")
    require(coverage(report) == roster_coverage(reg), "class_coverage")
    require(report["schema"] == "coreav.videotoolbox-fixture-capture/2" and report["scope"] == SCOPE and
            report["status"] == "complete" and
            report["capture_sha256"] == captured_hash, "fixture_capture")
    expected = [(cell, fixture, side) for cell in reg["cells"] for fixture in reg["fixtures"]
                for side in ("coreav", "ffmpeg")]
    require(type(report["records"]) is list and len(report["records"]) == len(expected), "fixture_capture")
    for row, identity in zip(report["records"], expected):
        keys(row, "fixture cell side status files encoder_ended_ns")
        require((row["cell"], row["fixture"], row["side"]) == identity and row["status"] == "success", "fixture_capture")
        covered_class(reg["fixtures"][row["fixture"]])
        directory = artifacts / f"fixture.{row['cell']}.{row['fixture']}.{row['side']}"
        limits = {"video.h264": ENCODED, "native.json": SMALL} if row["side"] == "coreav" else {
            "video.h264": ENCODED, "packets.stats": SMALL, **BASELINE_FILES}
        check_records(directory, row["files"], limits)
        integer(row["encoder_ended_ns"], 1, 2**63 - 1)
        if row["side"] == "ffmpeg":
            query = load(directory / "identity-query.json", SMALL)
            require(query["encoder_ended_ns"] == row["encoder_ended_ns"], "identity_clock")
    return report


def trusted_analysis_event():
    event = os.environ.get("GITHUB_EVENT_NAME", "")
    if os.environ.get("GITHUB_ACTIONS"):
        require(event in ("workflow_dispatch", "push") and os.environ.get("GITHUB_REF") == "refs/heads/main" and
                os.environ.get("GITHUB_REPOSITORY") == "coreav-dev/coreav", "judged_event")
    else:
        require(event in ("", "workflow_dispatch", "push"), "judged_event")


def analysis_preflight(args):
    trusted_analysis_event()
    require(sys.platform == "linux", "analysis_platform")
    reg = source_registration(args.registration, args.root)
    if os.environ.get("GITHUB_ACTIONS"):
        require(os.environ.get("GITHUB_SHA") == git(args.root, "rev-parse", "HEAD").decode().strip(), "judged_event")
    require(file_hash(Path(__file__).resolve()) == reg["metric_tools"]["evidence_producer"], "producer_changed")
    require(reg["metric_tools"]["metrics"] == reg["metric_tools"]["ffmpeg"], "metric_tool")
    with tempfile.TemporaryDirectory() as temporary:
        if args.install_oracle:
            run(["bash", str(args.root / "scripts/install-oracle-ffmpeg.sh"), str(args.oracle_bin)], temporary, 1800)
        try:
            oracle = test_oracle.require(args.oracle_bin / "ffmpeg", args.oracle_bin / "ffprobe")
        except test_oracle.QualificationError:
            raise Invalid("child_failure") from None
    for name in ("ffmpeg", "ffprobe"):
        require(file_hash(getattr(oracle, name)) == reg["metric_tools"][name], "oracle_changed")
    require(file_hash(args.coreav_decoder) == reg["metric_tools"]["coreav_decoder"] and
            file_hash(args.model, SMALL) == reg["metric_tools"]["model"], "analysis_tool_changed")
    with tempfile.TemporaryDirectory() as temporary:
        run([str(oracle.ffmpeg), "-hide_banner", "-filters"], temporary, 30)
        available = {parts[1] for line in read(temporary, "stdout", SMALL).decode().splitlines()
                     if len(parts := line.split()) >= 2}
        require({"libvmaf", "psnr", "ssim"} <= available, "metric_tool_unavailable")
    return reg, oracle


def campaign_binding(args, reg):
    integer(args.storage_budget_bytes, 1, 2**63 - 1)
    return {**roster_coverage(reg), "scope": SCOPE,
            "registration_sha256": file_hash(args.registration, SMALL), "capture_sha256": file_hash(args.capture, SMALL),
            "fixture_capture_sha256": file_hash(args.fixture_capture, SMALL), "availability_sha256": file_hash(args.availability, SMALL),
            "before_gate_sha256": file_hash(args.before_gate, SMALL), "after_gate_sha256": file_hash(args.after_gate, SMALL),
            "tools_sha256": sha(json.dumps({"sources": reg["tools"], "tools": reg["metric_tools"]}, sort_keys=True).encode()),
            "inputs_sha256": sha(json.dumps({"inputs": reg["inputs"], "fixtures": reg["fixtures"]}, sort_keys=True).encode()),
            "storage_budget_bytes": args.storage_budget_bytes}


def verify_campaign(directory, registration_path, capture_path, reg):
    campaign = load(directory / "campaign.json", SMALL)
    keys(campaign, "schema binding")
    require(campaign["schema"] == "coreav.videotoolbox-campaign/1", "campaign_binding")
    args = SimpleNamespace(registration=registration_path, capture=capture_path,
                           fixture_capture=directory / "fixtures.capture.json", availability=directory / "availability.json",
                           before_gate=directory / "before.json", after_gate=directory / "after.json",
                           storage_budget_bytes=campaign["binding"]["storage_budget_bytes"])
    require(campaign["binding"] == campaign_binding(args, reg), "campaign_binding")
    status = load(directory / "producer-status.json", SMALL)
    keys(status, "schema status diagnostic completed binding history storage")
    validate_history(status["history"])
    expected = [f"call-{n:04d}" for n in range(len(reg["inputs"]) * len(reg["cells"]) * 12)]
    expected += [f"fixture.{cell}.{fixture}" for cell in reg["cells"] for fixture in reg["fixtures"]]
    require(status["schema"] == "coreav.videotoolbox-producer-status/1" and status["status"] == "complete" and
            status["binding"] == campaign["binding"] and status["completed"] == expected and
            status["storage"]["budget_bytes"] == args.storage_budget_bytes, "campaign_incomplete")


def capacity(args, reg):
    require(type(args.storage_budget_bytes) is int and args.storage_budget_bytes > 0, "storage_budget")
    roster_coverage(reg)
    storage = args.storage_root.resolve(strict=True)
    require(args.artifacts.resolve(strict=True).is_relative_to(storage) and
            args.output.resolve(strict=True).is_relative_to(storage) and
            not args.output.resolve().is_relative_to(args.artifacts.resolve()) and
            not args.artifacts.resolve().is_relative_to(args.output.resolve()), "storage_location")
    units = [(item, side) for _ in reg["cells"] for item in reg["inputs"] for _ in range(6)
             for side in ("coreav", "ffmpeg")]
    units += [(item, side) for _ in reg["cells"] for item in reg["fixtures"].values() for side in ("coreav", "ffmpeg")]
    metadata = 64 * SMALL + 2 * 24 * sum(GATE_FILES.values())
    retained = sum(sum(output_limits(item, side, True).values()) + SMALL for item, side in units) + metadata
    scratch = RAW
    required = retained + scratch
    used = 0
    for root in (args.artifacts, args.output):
        for path in root.rglob("*"):
            require(not path.is_symlink(), "symlink")
            if path.is_file():
                used += path.stat().st_size
    require(required <= args.storage_budget_bytes and used <= args.storage_budget_bytes, "storage_budget")
    require(shutil.disk_usage(storage).free >= max(0, required - used), "storage_capacity")
    return {"budget_bytes": args.storage_budget_bytes, "retained_upper_bound_bytes": retained,
            "scratch_reserve_bytes": scratch, "campaign_metadata_reserve_bytes": metadata, "raw_file_limit_bytes": RAW,
            "reference_bytes": sum(raw_size(item) for item in reg["inputs"]) +
                               sum(raw_size(item) for item in reg["fixtures"].values())}


def build_bundle(args):
    trusted_analysis_event()
    prepared = validate(load(args.registration, SMALL))
    binding = campaign_binding(args, prepared)
    expected_files = {"campaign.json", "producer-status.json", "evidence.json", "before.json", "after.json",
                      "availability.json", "fixtures.capture.json"}
    expected_files |= {f"call-{n:04d}.json" for n in range(len(prepared["inputs"]) * len(prepared["cells"]) * 12)}
    expected_files |= {f"fixture.{cell}.{fixture}.json" for cell in prepared["cells"] for fixture in prepared["fixtures"]}
    expected_files |= {f"preflight.{cell}.{side}.json" for cell in ("hardware", "software") for side in ("coreav", "ffmpeg")}
    report = {"schema": "coreav.videotoolbox-producer-status/1", "status": "failed",
              "diagnostic": "analysis_incomplete", "completed": [], "binding": binding, "history": []}
    campaign = args.output / "campaign.json"
    if args.output.exists():
        require(args.resume and not args.output.is_symlink(), "resume_required")
        saved = load(campaign, SMALL)
        keys(saved, "schema binding")
        require(saved["schema"] == "coreav.videotoolbox-campaign/1" and saved["binding"] == binding, "resume_binding")
    else:
        args.output.mkdir(exist_ok=False)
        write(campaign, {"schema": "coreav.videotoolbox-campaign/1", "binding": binding})
    lock = os.open(args.output / ".campaign.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    try:
        info = os.fstat(lock)
        require(stat.S_ISREG(info.st_mode) and info.st_size == 0 and info.st_nlink == 1, "campaign_lock")
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        os.close(lock)
        raise Invalid("analysis_already_running") from None
    except BaseException:
        os.close(lock)
        raise
    if (args.output / "evidence.json").exists() or (args.output / "evidence.json").is_symlink():
        try:
            require(args.resume, "resume_required")
            reg, oracle = analysis_preflight(args)
            require(reg == prepared, "resume_binding")
            with tempfile.TemporaryDirectory() as temporary:
                timing.replay(SimpleNamespace(root=args.root, registration=args.registration,
                                              capture=args.capture, artifacts=args.artifacts,
                                              evidence=args.output, report=Path(temporary) / "report.json"))
        finally:
            os.close(lock)
        return
    try:
        if args.resume:
            previous_path = args.output / "producer-status.json"
            if previous_path.exists():
                previous = load(previous_path, SMALL)
                require(previous["binding"] == binding and type(previous["history"]) is list, "resume_binding")
                validate_history(previous["history"])
                report["history"] = previous["history"]
            if not previous_path.exists() or previous["status"] != "complete":
                report["history"] = report["history"] + [{"status": "failed", "diagnostic": "resumed_interrupted_campaign"}]
            for name in expected_files - {"campaign.json"}:
                remove_partial_write(args.output / name)
        write(args.output / "producer-status.json", report)
        reg, oracle = analysis_preflight(args)
        require(reg == prepared, "resume_binding")
        reg_hash = file_hash(args.registration)
        captured = load(args.capture, SMALL)
        keys(captured, "schema status scope diagnostic records registration_sha256 before_gate_sha256 "
             "availability_sha256 started_ns ended_ns covered_classes uncovered_classes")
        require(coverage(captured) == coverage(reg), "class_coverage")
        require(captured["schema"] == "coreav.videotoolbox-timing/2" and captured["scope"] == SCOPE and
                captured["status"] == "awaiting_evidence" and
                captured["registration_sha256"] == reg_hash, "capture_status")
        judging.timings(captured["records"], reg)
        fixture_capture = verify_fixture_capture(args.fixture_capture, reg, file_hash(args.capture), args.artifacts)
        require(fixture_capture["registration_sha256"] == reg_hash, "fixture_capture")
        expected = {f"call-{position:04d}" for position in range(len(captured["records"]))}
        expected |= {f"fixture.{cell}.{fixture}.{side}" for cell in reg["cells"] for fixture in reg["fixtures"]
                     for side in ("coreav", "ffmpeg")}
        require(not args.artifacts.is_symlink() and {p.name for p in args.artifacts.iterdir()} == expected,
                "artifact_coverage")
        available = timing.availability_file(args.availability, reg, args.root)
        require(available["registration_sha256"] == reg_hash and file_hash(args.availability) ==
                captured["availability_sha256"], "availability_binding")
        before, a = timing.bound_gate(args.before_gate, reg, reg_hash, "before")
        after, b = timing.bound_gate(args.after_gate, reg, reg_hash, "after")
        require(file_hash(args.before_gate) == captured["before_gate_sha256"] and
                before["ended_ns"] <= captured["started_ns"] < captured["ended_ns"] <= after["started_ns"] and
                abs(a / b - 1) <= 0.03, "host_gate")
        spec = load(args.root / reg["baseline_build"]["file"])["observability"]
        report["storage"] = capacity(args, reg)
        write(args.output / "producer-status.json", report)
        input_hashes(args.root, reg)
        input_hashes(args.root, reg, fixtures=True)
        index = {"schema": "coreav.videotoolbox-evidence/2", **coverage(reg), "scope": SCOPE,
                 "registration_sha256": reg_hash,
                 "capture_sha256": file_hash(args.capture), "files": {}, "outputs": {}, "fixtures": {}}
        for source, name in ((args.before_gate, "before.json"), (args.after_gate, "after.json"),
                             (args.availability, "availability.json"), (args.fixture_capture, "fixtures.capture.json")):
            copy_immutable(source, args.output / name, args.resume)
        for source, phase in ((args.before_gate, "before"), (args.after_gate, "after")):
            name = f"gate-{phase}.artifacts"
            destination = args.output / name
            require(not destination.is_symlink() and all(not p.is_symlink() for p in destination.rglob("*")), "symlink")
            shutil.copytree(source.parent / name, destination, symlinks=True, dirs_exist_ok=args.resume,
                            copy_function=lambda source, target: copy_immutable(source, target, args.resume))
            timing.bound_gate(args.output / (phase + ".json"), reg, reg_hash, phase)
        for cell in ("hardware", "software"):
            for side in ("coreav", "ffmpeg"):
                name = f"preflight.{cell}.{side}"
                copy_immutable(args.availability.parent / (name + ".json"), args.output / (name + ".json"), args.resume)
                destination = args.output / (name + ".artifacts")
                require(not destination.is_symlink() and all(not p.is_symlink() for p in destination.rglob("*")), "symlink")
                shutil.copytree(args.availability.parent / (name + ".artifacts"), destination,
                                symlinks=True, dirs_exist_ok=args.resume,
                                copy_function=lambda source, target: copy_immutable(source, target, args.resume))
        timing.availability_file(args.output / "availability.json", reg, args.root)
        preflight_directories = {f"preflight.{cell}.{side}.artifacts" for cell in ("hardware", "software")
                                 for side in ("coreav", "ffmpeg")}
        require({p.name for p in args.output.iterdir()} <= expected_files | preflight_directories |
                {"gate-before.artifacts", "gate-after.artifacts", ".campaign.lock"},
                "evidence_coverage")
        by_id = {item["id"]: item for item in reg["inputs"]}
        for position, call in enumerate(captured["records"]):
            name = f"call-{position:04d}"
            directory = args.artifacts / name
            require(file_hash(directory / "video.h264", ENCODED, root=args.artifacts) == call["output_sha256"] and
                    (directory / "video.h264").stat().st_size == call["output_bytes"], "output_hash")
            if call["side"] == "coreav":
                require(file_hash(directory / "native.json", SMALL) == call["native_report_sha256"], "native_report_hash")
            else:
                require(file_hash(directory / "packets.stats", SMALL) == call["packet_clock_sha256"] and
                        file_hash(directory / "observations.txt", SMALL) == call["observations_sha256"] and
                        file_hash(directory / "identity-query.json", SMALL) == call["identity_query_sha256"] and
                        file_hash(directory / "requested.json", SMALL) == call["requested_sha256"], "baseline_evidence")
                baseline_identity(directory, reg, call["cell"], spec, call["output_sha256"], call["ended_ns"])
            item = by_id[call["input_id"]]
            facts = analyse(directory, args.root / item["raw_file"], item, call["cell"], call["side"], reg, args, spec, oracle)
            require(facts["output_sha256"] == call["output_sha256"] and facts["payload_bytes"] == call["output_bytes"],
                    "output_changed")
            if call["side"] == "coreav":
                require(facts["retained"]["native.json"]["sha256"] == call["native_report_sha256"], "output_changed")
            else:
                require(facts["retained"]["packets.stats"]["sha256"] == call["packet_clock_sha256"] and
                        facts["retained"]["observations.txt"]["sha256"] == call["observations_sha256"] and
                        facts["retained"]["identity-query.json"]["sha256"] == call["identity_query_sha256"] and
                        facts["retained"]["requested.json"]["sha256"] == call["requested_sha256"], "output_changed")
            filename = name + ".json"
            if (args.output / filename).exists():
                require(args.resume and load(args.output / filename, SMALL) == facts, "resume_evidence_changed")
            else:
                write(args.output / filename, facts)
            index["outputs"][name] = filename
            report["completed"].append(name)
            write(args.output / "producer-status.json", report)
        for cell in reg["cells"]:
            for fixture, item in reg["fixtures"].items():
                facts = {}
                for side in ("coreav", "ffmpeg"):
                    directory = args.artifacts / f"fixture.{cell}.{fixture}.{side}"
                    facts[side] = analyse(directory, args.root / item["raw_file"], item, cell, side, reg, args, spec, oracle)
                filename = f"fixture.{cell}.{fixture}.json"
                fixture_value = {"schema": "coreav.videotoolbox-fixture-quality/1", "fixture": fixture,
                    "cell": cell, "producer_sha256": reg["metric_tools"]["evidence_producer"],
                    "registration_sha256": reg_hash, **facts}
                if (args.output / filename).exists():
                    require(args.resume and load(args.output / filename, SMALL) == fixture_value, "resume_evidence_changed")
                else:
                    write(args.output / filename, fixture_value)
                index["fixtures"][f"{cell}.{fixture}"] = filename
                report["completed"].append(f"fixture.{cell}.{fixture}")
                write(args.output / "producer-status.json", report)
        require(file_hash(args.registration) == reg_hash, "registration_changed")
        require(file_hash(args.output / "before.json") == captured["before_gate_sha256"] and
                file_hash(args.output / "availability.json") == captured["availability_sha256"], "evidence_changed")
        timing.availability_file(args.output / "availability.json", reg, args.root)
        verify_fixture_capture(args.output / "fixtures.capture.json", reg, file_hash(args.capture), args.artifacts)
        analysis_preflight(args)
        report.update(status="complete", diagnostic="evidence_captured")
        write(args.output / "producer-status.json", report)
        names = {path.name for path in args.output.iterdir() if path.is_file() and
                 path.name not in ("evidence.json", ".campaign.lock")}
        require(names == expected_files - {"evidence.json"}, "evidence_coverage")
        index["files"] = records(args.output, names, {name: SMALL for name in names})
        if (args.output / "evidence.json").exists():
            require(args.resume and load(args.output / "evidence.json", SMALL) == index, "resume_evidence_changed")
        else:
            write(args.output / "evidence.json", index)
    except BaseException as error:
        report["status"] = "failed"
        report["diagnostic"] = error.code if isinstance(error, Invalid) and error.code in {
            "ffmpeg_observability_missing", "decoder_disagreement", "clock", "metric_nonfinite", "psnr_pixels",
            "timeout", "child_crash", "child_failure", "oracle_changed", "host_gate", "resume_binding",
            "resume_evidence_changed", "analysis_failed_final", "storage_budget", "storage_location",
            "storage_capacity"} else "analysis_failed"
        write(args.output / "producer-status.json", report)
        raise
    finally:
        os.close(lock)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands_parser = parser.add_subparsers(dest="command", required=True)
    host = commands_parser.add_parser("host", help="Write sanitized hardware observations, never raw identifiers")
    host.add_argument("--report", required=True, type=Path)
    pre = commands_parser.add_parser("preflight")
    pre.add_argument("--output", required=True, type=Path)
    pre.add_argument("--ffprobe", required=True, type=Path)
    fixtures = commands_parser.add_parser("fixtures")
    fixtures.add_argument("--availability", required=True, type=Path)
    fixtures.add_argument("--capture", required=True, type=Path)
    fixtures.add_argument("--artifacts", required=True, type=Path)
    fixtures.add_argument("--report", required=True, type=Path)
    bundle = commands_parser.add_parser("analyse")
    for name in ("capture", "fixture-capture", "artifacts", "availability", "before-gate", "after-gate", "oracle-bin", "coreav-decoder", "model", "output"):
        bundle.add_argument("--" + name, required=True, type=Path)
    bundle.add_argument("--install-oracle", action="store_true")
    bundle.add_argument("--resume", action="store_true", help="Revalidate completed units; rebuild only owned incomplete analysis")
    bundle.add_argument("--storage-root", required=True, type=Path,
                        help="Existing storage containing separate artifacts and evidence directories; nothing is provisioned")
    bundle.add_argument("--storage-budget-bytes", required=True, type=int,
                        help="Cap retained artifacts/evidence plus a 32 GiB scratch reserve; raw references are separate")
    for sub in (pre, fixtures, bundle):
        sub.add_argument("--root", type=Path, default=ROOT)
        sub.add_argument("--registration", required=True, type=Path)
    args = parser.parse_args()
    for name, value in vars(args).items():
        if isinstance(value, Path):
            setattr(args, name, value.absolute())
    try:
        if args.command == "host":
            write(args.report, {"schema": "coreav.videotoolbox-host-observation/1", "observed": host_observation()})
        elif args.command == "preflight":
            capture_preflights(args)
        elif args.command == "fixtures":
            capture_fixtures(args)
        else:
            build_bundle(args)
        return 0
    except (Invalid, OSError, ValueError, KeyError, TypeError, UnicodeError):
        print("videotoolbox-producer: failed", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
