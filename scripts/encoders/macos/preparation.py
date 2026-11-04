"""Reproduce frozen raw identities from local sources; no selection or CLI."""

import copy
import hashlib
import os
import platform
import re
import shutil
import stat
import sys
import time
from fractions import Fraction
from pathlib import Path

import preparation_process as process
from evidence import digest, file_hash, integer, keys, require
from process import _SignalScope

ROOT = Path(__file__).resolve().parents[3]
MAX_PIXELS = 3840 * 2160
MAX_ALLOC = 256 << 20
SOURCE_LIMIT = 8 << 30
COLOUR = {
    "primaries": "bt709",
    "transfer": "bt709",
    "matrix": "bt709",
    "range": "limited",
}
SOURCE_COLOURS = {
    "color_primaries": {
        "bt709",
        "bt470m",
        "bt470bg",
        "smpte170m",
        "smpte240m",
        "film",
        "smpte431",
        "smpte432",
        "ebu3213",
    },
    "color_transfer": {
        "bt709",
        "bt470m",
        "smpte170m",
        "smpte240m",
        "linear",
        "log100",
        "log316",
        "iec61966-2-4",
        "iec61966-2-1",
    },
    "color_space": {"bt709", "fcc", "bt470bg", "smpte170m", "smpte240m"},
    "color_range": {"tv", "pc"},
}


def rational(value):
    keys(value, "num den")
    return Fraction(
        integer(value["num"], 1, 2**31 - 1), integer(value["den"], 1, 2**31 - 1)
    )


def validate_recipe(item):
    required = {
        "id",
        "class",
        "source_sha256",
        "stream",
        "width",
        "height",
        "frames",
        "rate",
        "colour",
        "raw_sha256",
        "source_frame_sha256",
        "source_tick",
        "source_start_ticks",
        "audio_start_ticks",
        "source_bytes",
        "preparation",
    }
    require(
        type(item) is dict and required <= set(item),
        "preparation_item",
    )
    require(digest(item["id"]) == digest(item["source_sha256"]), "source_identity")
    digest(item["raw_sha256"])
    integer(item["stream"], 0, 1024)
    width, height = integer(item["width"], 2, 3840), integer(item["height"], 2, 1080)
    require(width % 2 == height % 2 == 0, "preparation_geometry")
    require(
        type(item["class"]) is int
        and item["class"] == next(h for h in (360, 720, 1080) if height <= h),
        "preparation_class",
    )
    count = integer(item["frames"], 240, 1800)
    rate = rational(item["rate"])
    rational(item["source_tick"])
    integer(item["source_start_ticks"], -(2**53), 2**53)
    require(
        item["audio_start_ticks"] is None or type(item["audio_start_ticks"]) is int,
        "preparation_audio_clock",
    )
    ten = 10 * rate
    require(
        count == max(240, (ten.numerator + ten.denominator - 1) // ten.denominator)
        and 8 <= count / rate <= 30,
        "preparation_prefix",
    )
    require(item["colour"] == COLOUR, "preparation_colour")
    integer(item["source_bytes"], 1, SOURCE_LIMIT)
    require(
        type(item["source_frame_sha256"]) is list
        and len(item["source_frame_sha256"]) == count,
        "preparation_frame_hashes",
    )
    for expected in item["source_frame_sha256"]:
        digest(expected)
    recipe = keys(
        item["preparation"],
        "source_pixel_format source_colour assumptions container_name field_order oracle_sha256",
    )
    require(
        type(recipe["source_pixel_format"]) is str
        and re.fullmatch(
            r"yuvj?(?:410|411|420|422|440|444)p(?:(?:9|10|12|14|16)le)?",
            recipe["source_pixel_format"],
        ),
        "preparation_source_format",
    )
    require(
        recipe["container_name"]
        in ("mov", "matroska", "avi", "mpegts", "flv", "asf", "mpeg", "ogg", "nut"),
        "preparation_container",
    )
    require(recipe["field_order"] == "progressive", "preparation_scan")
    colour = keys(
        recipe["source_colour"],
        "color_primaries color_transfer color_space color_range",
    )
    require(
        all(
            type(value) is str and value in SOURCE_COLOURS[field]
            for field, value in colour.items()
        ),
        "preparation_source_colour",
    )
    require(
        type(recipe["assumptions"]) is list and len(recipe["assumptions"]) <= 4,
        "preparation_assumptions",
    )
    seen = set()
    for assumption in recipe["assumptions"]:
        keys(assumption, "field assumed_value observed rule")
        field = assumption["field"]
        require(
            type(field) is str and field in SOURCE_COLOURS and field not in seen,
            "preparation_assumption_field",
        )
        seen.add(field)
        expected = (
            ("pc" if recipe["source_pixel_format"].startswith("yuvj") else "tv")
            if field == "color_range"
            else "bt709"
        )
        rule = (
            "literal_yuvj_full_otherwise_limited"
            if field == "color_range"
            else "absent_tag_bt709"
        )
        require(
            assumption["observed"] is False
            and assumption["assumed_value"] == expected
            and colour[field] == expected
            and assumption["rule"] == rule,
            "preparation_assumption_label",
        )
    keys(recipe["oracle_sha256"], "ffmpeg ffprobe")
    for expected in recipe["oracle_sha256"].values():
        digest(expected)
    require(
        MAX_PIXELS == 8294400
        and MAX_ALLOC == 256 << 20
        and MAX_ALLOC >= MAX_PIXELS * 8,
        "preparation_resource_constants",
    )
    return recipe


def command(source_path, item, oracle_bin):
    recipe = validate_recipe(item)
    c = recipe["source_colour"]
    conversion = (
        "scale=w=iw:h=ih:flags=bilinear+accurate_rnd+bitexact:sws_dither=none:in_color_matrix="
        + c["color_space"]
        + ":out_color_matrix="
        + c["color_space"]
        + ":in_range="
        + c["color_range"]
        + ":out_range="
        + c["color_range"]
        + ":threads=1,format=pix_fmts=yuv444p16le,zscale=primariesin="
        + c["color_primaries"]
        + ":transferin="
        + c["color_transfer"]
        + ":matrixin="
        + c["color_space"]
        + ":rangein="
        + c["color_range"]
        + ":primaries=bt709:transfer=bt709:matrix=bt709:range=limited:dither=ordered:filter=bilinear,format=pix_fmts=yuv420p"
    )
    discovery = [
        "-probesize",
        "5242880",
        "-analyzeduration",
        "5000000",
        "-protocol_whitelist",
        "file,pipe",
        "-format_whitelist",
        recipe["container_name"],
    ]
    if recipe["container_name"] == "mov":
        discovery += ["-enable_drefs", "0", "-use_absolute_path", "0"]
    return [
        str(Path(oracle_bin) / "ffmpeg"),
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-xerror",
        "-max_alloc",
        str(MAX_ALLOC),
        "-max_pixels",
        str(MAX_PIXELS),
        "-err_detect",
        "explode",
        "-ec",
        "0",
        "-noauto_conversion_filters",
        "-filter_threads",
        "1",
        "-filter_complex_threads",
        "1",
        "-threads",
        "1",
        *discovery,
        "-noautorotate",
        "-copyts",
        "-i",
        str(source_path),
        "-map",
        "0:" + str(item["stream"]),
        "-an",
        "-sn",
        "-dn",
        "-noautoscale",
        "-v",
        "error",
        "-vf",
        conversion,
        "-frames:v",
        str(item["frames"]),
        "-fps_mode",
        "passthrough",
        "-c:v",
        "rawvideo",
        "-threads:v",
        "1",
        "-pix_fmt",
        "yuv420p",
        "-f",
        "rawvideo",
        "pipe:1",
    ]


def copy_source(source_path, destination, item):
    fd = os.open(source_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    count, hasher = 0, hashlib.sha256()
    deadline = time.monotonic() + 60
    try:
        info = os.fstat(fd)
        require(
            stat.S_ISREG(info.st_mode) and info.st_size == item["source_bytes"],
            "preparation_source_size",
        )
        with destination.open("xb") as sink:
            while True:
                require(time.monotonic() < deadline, "preparation_source_deadline")
                data = os.read(fd, min(1024 * 1024, item["source_bytes"] - count + 1))
                if not data:
                    break
                count += len(data)
                require(count <= item["source_bytes"], "preparation_source_size")
                hasher.update(data)
                sink.write(data)
            sink.flush()
            os.fsync(sink.fileno())
        require(
            count == item["source_bytes"]
            and hasher.hexdigest() == item["source_sha256"],
            "preparation_source_hash",
        )
        after = os.fstat(fd)
        require(
            (info.st_size, info.st_mtime_ns, info.st_ctime_ns)
            == (after.st_size, after.st_mtime_ns, after.st_ctime_ns),
            "preparation_source_changed",
        )
        destination.chmod(0o400)
    finally:
        os.close(fd)


def oracle_identity(oracle_bin, expected):
    for name, value in expected.items():
        require(
            file_hash(Path(oracle_bin) / name, limit=1 << 30) == value,
            "preparation_oracle_changed",
        )


def materialize_one(source_path, item, oracle_bin, output_dir):
    """Create one private output directory; a refusal never retries or replaces it."""
    require(
        sys.platform == "linux" and platform.machine() == "x86_64",
        "preparation_linux_required",
    )
    require(hasattr(os, "pidfd_open"), "preparation_pidfd_required")
    item = copy.deepcopy(item)
    recipe = validate_recipe(item)
    output = Path(output_dir)
    output.mkdir(mode=0o700, parents=False, exist_ok=False)
    source = output / "source.bin"
    partial, raw = output / "prepare.raw", output / "reference.yuv"
    result = {
        "schema": "coreav.raw-materialization/1",
        "input_id": item["id"],
        "source_sha256": item["source_sha256"],
        "source_bytes": item["source_bytes"],
        "expected_raw_sha256": item["raw_sha256"],
        "status": "starting",
    }
    published = False
    with _SignalScope() as guard:
        try:
            process.publish(output / "materialization.json", result)
            extent = item["width"] * item["height"] * 3 * item["frames"] // 2
            require(
                shutil.disk_usage(output).free
                >= item["source_bytes"] + extent + (65 << 20),
                "preparation_disk_bound",
            )
            copy_source(source_path, source, item)
            expected = recipe["oracle_sha256"]
            oracle_identity(oracle_bin, expected)
            process.capture(
                [
                    sys.executable,
                    "-I",
                    str(ROOT / "scripts/oracle_pin.py"),
                    "--verify-bin",
                    str(oracle_bin),
                ],
                output,
                "oracle",
                {"oracle_sha256": expected},
                time.time() + 60,
                guard=guard,
            )
            oracle_identity(oracle_bin, expected)
            hashes = process.RawHashes(
                item["width"] * item["height"] * 3 // 2, item["frames"]
            )
            captured = process.capture(
                command(source, item, oracle_bin),
                output,
                "prepare",
                {
                    "source_sha256": item["source_sha256"],
                    "source_bytes": item["source_bytes"],
                    "oracle_sha256": expected,
                },
                time.time() + 60,
                hashes,
                guard,
            )
            oracle_identity(oracle_bin, expected)
            observed = captured["raw_hashes"]
            require(
                observed["whole_sha256"] == item["raw_sha256"]
                and observed["frame_sha256"] == item["source_frame_sha256"],
                "preparation_raw_identity",
            )
            require(partial.stat().st_size == extent, "preparation_raw_extent")
            source.unlink()
            os.link(partial, raw)
            published = True
            partial.unlink()
            raw.chmod(0o400)
            result.update(status="complete", raw_file=str(raw), raw_hashes=observed)
            process.publish(output / "materialization.json", result)
            return result
        except BaseException as error:
            cleanup_errors = []
            for path in (source, partial, *((raw,) if published else ())):
                try:
                    path.unlink(missing_ok=True)
                except BaseException as cleanup:
                    cleanup_errors.append(process.diagnostic(cleanup))
            original = error
            if cleanup_errors:
                error = process.InfrastructureFailure(
                    "materialization_cleanup_uncertain",
                    process.diagnostic(original),
                    cleanup_errors[0],
                    capture_record={
                        "cleanup_failures": cleanup_errors,
                        "original_infrastructure": getattr(original, "details", None),
                    },
                )
            elif isinstance(error, OSError):
                error = process.InfrastructureFailure(
                    "materialization_infrastructure_failure",
                    process.diagnostic(original),
                )
            result.pop("raw_file", None)
            if "raw_hashes" in result:
                result["unaccepted_raw_hashes"] = result.pop("raw_hashes")
            result.update(
                status="fatal_infrastructure"
                if isinstance(error, process.InfrastructureFailure)
                else "refused",
                failure=process.diagnostic(error),
            )
            if isinstance(error, process.InfrastructureFailure):
                result["infrastructure_failure"] = error.details
            try:
                process.publish(output / "materialization.json", result)
            except BaseException as publication:
                raise process.InfrastructureFailure(
                    "materialization_record_publication_failed",
                    process.diagnostic(original),
                    cleanup_errors[0] if cleanup_errors else None,
                    process.diagnostic(publication),
                    getattr(error, "details", None),
                ) from original
            if error is original:
                raise
            raise error from original
