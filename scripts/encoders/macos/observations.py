"""Parse only explicit, release-registered VideoToolbox trace observations."""

import re
from fractions import Fraction

from evidence import integer, require

FIELDS = {"encoder_id", "hardware", "profile", "reordering", "realtime", "bitrate", "width", "height",
          "fps_num", "fps_den", "keyframe_frames", "keyframe_seconds", "pixel_format", "range",
          "primaries", "transfer", "matrix", "submitted", "status"}
NUMBERS = {"bitrate", "width", "height", "fps_num", "fps_den", "keyframe_frames", "keyframe_seconds", "submitted"}


def specification(value):
    require(type(value) is dict and set(value) == {"schema", "fields"}, "ffmpeg_observability_missing")
    require(value["schema"] == "coreav.videotoolbox-observability/1" and
            type(value["fields"]) is dict and set(value["fields"]) == FIELDS,
            "ffmpeg_observability_missing")
    prefixes = set()
    for field, rule in value["fields"].items():
        require(type(rule) is dict and set(rule) == {"prefix", "suffix"}, "ffmpeg_observability_missing")
        require(type(rule["prefix"]) is str and 1 <= len(rule["prefix"]) <= 256 and
                type(rule["suffix"]) is str and len(rule["suffix"]) <= 128 and
                all(32 <= ord(c) < 127 and c not in "/\\" for c in rule["prefix"] + rule["suffix"]),
                "ffmpeg_observability_missing")
        require(rule["prefix"] not in prefixes, "ffmpeg_observability_missing")
        prefixes.add(rule["prefix"])
    return value


def trace(data, spec):
    specification(spec)
    require(len(data) <= 16 * 1024**2, "trace_bounds")
    try:
        lines = data.decode("utf-8").splitlines()
    except UnicodeError:
        require(False, "ffmpeg_observability_missing")
    found = {}
    retained = []
    for raw in lines:
        require(len(raw) <= 8192, "trace_bounds")
        line = re.sub(r"^\[h264_videotoolbox @ 0x[0-9a-fA-F]+\]\s*", "", raw)
        for field, rule in spec["fields"].items():
            if not line.startswith(rule["prefix"]) or not line.endswith(rule["suffix"]):
                continue
            end = len(line) - len(rule["suffix"]) if rule["suffix"] else len(line)
            value = line[len(rule["prefix"]):end]
            require(re.fullmatch(r"[A-Za-z0-9_.+-]{1,512}", value) is not None,
                    "ffmpeg_observability_missing")
            if field in NUMBERS:
                require(re.fullmatch(r"[0-9]{1,12}(?:\.[0-9]{1,12})?(?:[eE][+-]?[0-9]{1,2})?", value) is not None,
                        "ffmpeg_observability_missing")
                exact = Fraction(value)
                require(exact.denominator == 1, "ffmpeg_effective_settings")
                value = integer(int(exact), 0, 10**12)
            elif field in ("reordering", "realtime"):
                value = value.lower()
                require(value in ("true", "false", "1", "0"), "ffmpeg_observability_missing")
                value = value in ("true", "1")
            elif field == "hardware":
                value = value.lower()
                require(value in ("hardware", "software", "true", "false", "1", "0"),
                        "ffmpeg_observability_missing")
                value = "hardware" if value in ("hardware", "true", "1") else "software"
            elif field == "status":
                value = {"0": "encoded", "-12908": "no_encoder", "-12915": "temporarily_unavailable"}.get(value, value)
            elif field == "profile":
                value = {"High": "high", "H264_High_AutoLevel": "high"}.get(value, value)
            require(field not in found or found[field] == value, "ffmpeg_observation_changed")
            found[field] = value
            retained.append(line)
    require({"status", "submitted"} <= set(found), "ffmpeg_observability_missing")
    if found["status"] == "encoded":
        require(set(found) == FIELDS, "ffmpeg_observability_missing")
    else:
        require(found["status"] in ("no_encoder", "temporarily_unavailable") and found["submitted"] == 0,
                "ffmpeg_observability_missing")
    return found, ("\n".join(dict.fromkeys(retained)) + "\n").encode()


def effective(value, item, cell, bitrate):
    expected = {"hardware": cell, "profile": "high", "reordering": False, "realtime": False,
                "bitrate": bitrate, "width": item["width"], "height": item["height"],
                "fps_num": item["rate"]["num"], "fps_den": item["rate"]["den"],
                "keyframe_frames": 2 * item["rate"]["num"] // item["rate"]["den"],
                "keyframe_seconds": 2, "pixel_format": "yuv420p", "range": "limited",
                "primaries": "bt709", "transfer": "bt709", "matrix": "bt709",
                "submitted": item["frames"], "status": "encoded"}
    require(all(type(value.get(k)) is type(v) and value[k] == v for k, v in expected.items()),
            "ffmpeg_effective_settings")
    require(type(value.get("encoder_id")) is str and
            re.fullmatch(r"[A-Za-z0-9_.-]{1,512}", value["encoder_id"]) is not None,
            "ffmpeg_observability_missing")
