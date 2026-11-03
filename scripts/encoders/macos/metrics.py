"""Strict per-frame metric logs, bound to actual source and decoded I420 bytes."""

import math
import re
from contextlib import nullcontext

from evidence import integer, keys, loads, number, require, sha


def decimal(value):
    require(re.fullmatch(r"-?[0-9]+(?:\.[0-9]+)?", value) is not None, "metric_nonfinite")
    result = float(value)
    number(result, -1e12, 1e12)
    return result


def metric_logs(vmaf_bytes, psnr_bytes, ssim_bytes, frames):
    value = loads(vmaf_bytes)
    require(type(value) is dict and "frames" in value and
            set(value) <= {"version", "fps", "frames", "pooled_metrics", "aggregate_metrics"}, "vmaf_shape")
    require(type(value["frames"]) is list and len(value["frames"]) == frames, "metric_frames")
    vmaf = []
    for index, row in enumerate(value["frames"]):
        keys(row, "frameNum metrics")
        require(type(row["frameNum"]) is int and row["frameNum"] == index and
                type(row["metrics"]) is dict and "vmaf" in row["metrics"], "metric_identity")
        vmaf.append(number(row["metrics"]["vmaf"], 0, 100))
    try:
        psnr_lines = psnr_bytes.decode().splitlines()
        ssim_lines = ssim_bytes.decode().splitlines()
    except UnicodeError:
        require(False, "metric_log")
    require(psnr_lines and psnr_lines[0].startswith("psnr_log_version:2 fields:"), "psnr_shape")
    psnr = []
    for index, line in enumerate(psnr_lines[1:]):
        row = {}
        for entry in line.split():
            parts = entry.split(":")
            require(len(parts) == 2 and parts[0] not in row, "psnr_shape")
            row[parts[0]] = parts[1]
        require(set(row) >= {"n", "mse_avg", "mse_y", "mse_u", "mse_v", "psnr_avg",
                             "psnr_y", "psnr_u", "psnr_v"} and
                set(row) <= {"n", "mse_avg", "mse_y", "mse_u", "mse_v", "psnr_avg",
                             "psnr_y", "psnr_u", "psnr_v", "max_avg", "max_y", "max_u", "max_v"}, "psnr_shape")
        require(re.fullmatch(r"[0-9]+", row["n"]) is not None and int(row["n"]) == index + 1,
                "metric_identity")
        for field, entry in row.items():
            if field != "n":
                if field.startswith("psnr_") and entry in ("inf", "+inf"):
                    continue
                decimal(entry)
        psnr.append(row)
    ssim = []
    for index, line in enumerate(ssim_lines):
        match = re.fullmatch(r"n:([0-9]+) Y:([^ ]+) U:([^ ]+) V:([^ ]+) All:([^ ]+) \(([^()]+)\)", line)
        require(match is not None and int(match[1]) == index + 1, "ssim_shape")
        values = [number(decimal(match[n]), -1, 1) for n in range(2, 6)]
        require(abs((4 * values[0] + values[1] + values[2]) / 6 - values[3]) <= 0.000002,
                "ssim_planes")
        if match[6] not in ("inf", "+inf"):
            decimal(match[6])
        ssim.append(values[3])
    require(len(psnr) == len(ssim) == frames, "metric_frames")
    return vmaf, psnr, ssim


def frames(source, decoded, item, logs=None, errors=None, independent=None):
    size = item["width"] * item["height"] * 3 // 2
    require(0 < size <= 3840 * 2160 * 3 // 2, "frame_bounds")
    result = []
    decoded_hashes = []
    if logs is not None:
        error_values = loads(errors)
        keys(error_values, "schema width height frames")
        require(error_values["schema"] == "coreav.videotoolbox-error/1" and
                type(error_values["width"]) is int and error_values["width"] == item["width"] and
                type(error_values["height"]) is int and error_values["height"] == item["height"] and
                type(error_values["frames"]) is list and len(error_values["frames"]) == item["frames"], "compare_report")
        for index, frame in enumerate(error_values["frames"]):
            keys(frame, "id samples squared_error")
            require(type(frame["id"]) is int and frame["id"] == index and
                    type(frame["samples"]) is int and frame["samples"] == size, "compare_report")
            integer(frame["squared_error"], 0, 255**2 * size)
    with source.open("rb") as original, decoded.open("rb") as reconstruction, \
            (independent.open("rb") if independent is not None else nullcontext()) as other:
        for index, expected in enumerate(item["source_frame_sha256"]):
            raw, pixels = original.read(size), reconstruction.read(size)
            require(len(raw) == len(pixels) == size and sha(raw) == expected, "source_frame_hash")
            if other is not None:
                require(pixels == other.read(size), "decoder_disagreement")
            identity = sha(pixels)
            decoded_hashes.append(identity)
            if logs is None:
                continue
            vmaf, psnr, ssim = logs
            squared = error_values["frames"][index]["squared_error"]
            value = 10 * math.log10(255**2 * size / squared) if squared else None
            reported = psnr[index]["psnr_avg"]
            if squared:
                precision = 10 ** (-len(reported.partition(".")[2])) / 2
                require(abs(decimal(reported) - value) <= precision + 1e-9, "psnr_pixels")
            else:
                require(reported in ("inf", "+inf"), "psnr_pixels")
            reported_mse = psnr[index]["mse_avg"]
            precision = 10 ** (-len(reported_mse.partition(".")[2])) / 2
            require(abs(decimal(reported_mse) - squared / size) <= precision + 1e-9, "psnr_pixels")
            result.append({"id": index, "source_sha256": expected, "decoded_sha256": identity,
                           "vmaf": vmaf[index], "psnr": value, "ssim": ssim[index],
                           "psnr_status": "finite" if squared else "perfect",
                           "squared_error": squared, "samples": size})
        require(not original.read(1) and not reconstruction.read(1), "decoded_frame_count")
        if other is not None:
            require(not other.read(1), "decoded_frame_count")
    require(len(decoded_hashes) == item["frames"], "decoded_frame_count")
    return decoded_hashes, result
