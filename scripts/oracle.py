#!/usr/bin/env python3
"""Test-only black-box capture and candidate orchestration (standard library)."""
import argparse
import collections
import copy
from fractions import Fraction
import json
from pathlib import Path
import shutil
import subprocess
import sys

STREAM_MAP = {
    "kind": "codec_type", "codec_fourcc": "codec_tag_string",
    "codec_name": "codec_name", "width": "width", "height": "height",
    "color_primaries": "color_primaries", "color_transfer": "color_transfer",
    "color_matrix": "color_space", "color_range": "color_range",
    "sample_rate": "sample_rate", "channels": "channels",
    "aspect_ratio": "sample_aspect_ratio",
}


def integer(value):
    return None if value is None or value == "N/A" else int(value)


def rational(value):
    if value is None or value == "N/A" or value in ("0/0", "0:1"):
        return None
    number = Fraction(str(value).replace(":", "/"))
    return [number.numerator, number.denominator]


def invoke(command, timeout):
    # Tools are external black boxes. Timeouts and signals are infrastructure
    # errors, never disguised as ordinary probe failures.
    result = subprocess.run(command, capture_output=True, timeout=timeout, check=False)
    if result.returncode < 0:
        raise RuntimeError(f"{command[0]} terminated by signal {-result.returncode}")
    return result


def probe(binary, fixture, timeout, packets=False, raw=False):
    command = [binary, "-v", "error", "-show_error", "-of", "json"]
    if packets:
        command += ["-show_packets"]
    else:
        command += ["-show_streams", "-show_format"]
    if raw:
        command += ["-ignore_editlist", "1"]
    result = invoke(command + [str(fixture)], timeout)
    value = json.loads(result.stdout or b"{}")
    if result.returncode == 0 and "error" in value:
        raise RuntimeError(f"{fixture}: ffprobe reported an error with a successful exit")
    return value, result.returncode == 0


def rotation(stream):
    for side in stream.get("side_data_list", []):
        if side.get("side_data_type") != "Display Matrix":
            continue
        rows = [line.split(":", 1)[1].split() for line in side.get("displaymatrix", "").splitlines() if ":" in line]
        matrix = [int(cell) for row in rows for cell in row]
        rotations = {
            (65536, 0, 0, 0, 65536, 0, 0, 0, 1073741824): 0,
            (0, -65536, 0, 65536, 0, 0, 0, 0, 1073741824): 270,
            (-65536, 0, 0, 0, -65536, 0, 0, 0, 1073741824): 180,
            (0, 65536, 0, -65536, 0, 0, 0, 0, 1073741824): 90,
        }
        # The fixture input option is counter-clockwise; the schema reports
        # clockwise degrees. Other transforms stay unknown.
        return rotations.get(tuple(matrix))
    return 0


def normalize_stream(stream):
    value = {key: stream.get(source) for key, source in STREAM_MAP.items()}
    for key in ("width", "height", "sample_rate", "channels"):
        value[key] = integer(value[key])
    tags = stream.get("tags", {})
    value.update(index=int(stream["index"]), rotation=rotation(stream),
                 time_base=rational(stream.get("time_base")),
                 duration=rational(stream.get("duration")),
                 language=tags.get("language"), handler_name=tags.get("handler_name"))
    # duration_ts is exact; use the rounded decimal only if ticks are absent.
    ticks = integer(stream.get("duration_ts"))
    if ticks is not None and value["time_base"] is not None:
        value["duration"] = rational(Fraction(ticks) * Fraction(*value["time_base"]))
    return value


def packet_key(packet):
    return int(packet["stream_index"]), integer(packet.get("pos"))


def normalize_packets(packets, media_packets):
    raw = collections.defaultdict(collections.deque)
    for packet in media_packets:
        raw[packet_key(packet)].append(packet)
    ordinals = collections.Counter()
    result = []
    for packet in packets:
        key = packet_key(packet)
        media = raw[key].popleft() if raw[key] else {}
        pts, dts = integer(packet.get("pts")), integer(packet.get("dts"))
        media_pts, media_dts = integer(media.get("pts")), integer(media.get("dts"))
        flags = packet.get("flags", "")
        result.append({
            "stream_index": key[0], "position": key[1], "ordinal": ordinals[key[0]],
            "pts": pts, "dts": dts, "media_pts": media_pts, "media_dts": media_dts,
            "pts_offset": None if pts is None or media_pts is None else pts - media_pts,
            "dts_offset": None if dts is None or media_dts is None else dts - media_dts,
            "size": integer(packet.get("size")), "duration": integer(packet.get("duration")),
            "keyframe": "K" in flags, "discard": "D" in flags,
        })
        ordinals[key[0]] += 1
    return result


def decode(binary, fixture, streams, timeout):
    frames, succeeded = [], True
    videos = [stream for stream in streams if stream["kind"] == "video"]
    if not videos:
        return [], None
    for stream in videos:
        result = invoke([binary, "-nostdin", "-hide_banner", "-loglevel", "error",
                         "-noautorotate", "-i", str(fixture), "-map", f"0:{stream['index']}",
                         "-fps_mode", "passthrough", "-threads", "1", "-bitexact",
                         "-f", "framemd5", "-"], timeout)
        succeeded = succeeded and result.returncode == 0
        time_base, ordinal = None, 0
        for line in result.stdout.decode("utf-8").splitlines():
            if line.startswith("#tb 0:"):
                time_base = rational(line.split(":", 1)[1].strip())
            elif line and not line.startswith("#"):
                parts = [part.strip() for part in line.split(",")]
                if len(parts) != 6 or time_base is None:
                    raise RuntimeError(f"{fixture}: invalid framemd5 output")
                frames.append({"stream_index": stream["index"], "ordinal": ordinal,
                               "time_base": time_base, "dts": int(parts[1]),
                               "pts": int(parts[2]), "duration": int(parts[3]),
                               "size": int(parts[4]), "md5": parts[5].lower()})
                ordinal += 1
    return frames, succeeded


def capture_one(fixture, args):
    metadata, success = probe(args.ffprobe, fixture, args.timeout)
    packet_data, packet_success = probe(args.ffprobe, fixture, args.timeout, packets=True)
    media_data, media_success = probe(args.ffprobe, fixture, args.timeout, packets=True, raw=True)
    streams = [normalize_stream(stream) for stream in metadata.get("streams", [])]
    frames, frames_success = decode(args.ffmpeg, fixture, streams, args.timeout)
    warnings = []
    if not success:
        warnings.append("ffprobe stream capture failed")
    if not packet_success or not media_success:
        warnings.append("ffprobe packet capture failed")
    if frames_success is False:
        warnings.append("ffmpeg decode failed; partial frame output is retained but excluded from agreement")
    return {"schema_version": 1, "fixture": fixture.name, "probe_success": success,
            "packet_success": packet_success and media_success, "frames_success": frames_success,
            "format_duration": rational(metadata.get("format", {}).get("duration")),
            "streams": streams,
            "packets": normalize_packets(packet_data.get("packets", []), media_data.get("packets", [])),
            "frames": frames, "warnings": warnings}


def fixtures(directory):
    directory = directory.resolve()
    manifest = directory / "MANIFEST"
    if not manifest.is_file():
        raise RuntimeError("fixture directory needs a MANIFEST with 'generated NAME' lines")
    names = []
    for line in manifest.read_text().splitlines():
        parts = line.split()
        if parts and parts[0] == "generated" and len(parts) == 2:
            name = parts[1]
            if Path(name).name != name or name in names:
                raise RuntimeError(f"invalid or duplicate fixture name: {name}")
            names.append(name)
        elif parts and parts[0] == "skipped":
            # Only two pre-existing optional interoperability cases are exempt.
            if len(parts) < 2 or parts[1] not in {"h264_avc3.mp4", "pcm_s16le.mp4"}:
                raise RuntimeError(f"required fixture skipped: {line}")
        else:
            raise RuntimeError(f"invalid manifest line: {line}")
    if not names:
        raise RuntimeError("empty fixture manifest")
    paths = [directory / name for name in names]
    for path in paths:
        if not path.is_file():
            raise RuntimeError(f"missing fixture: {path}")
    return paths


def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def fresh(directory):
    directory.mkdir(parents=True, exist_ok=False)
    return directory


def capture(paths, directory, args):
    for path in paths:
        write(directory / (path.name + ".json"), capture_one(path, args))
        print(f"captured {path.name}", file=sys.stderr)


def compare(reference, candidate, args, report):
    result = invoke([str(args.comparator.resolve()), str(reference), str(candidate), args.profile], args.timeout)
    value = json.loads(result.stdout or b"{}")
    write(report, value)
    if result.returncode not in (0, 1):
        raise RuntimeError(result.stderr.decode("utf-8", errors="replace"))
    return result.returncode, value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["capture", "run", "self-check"])
    parser.add_argument("fixtures", type=Path)
    parser.add_argument("output", type=Path, help="new directory; existing outputs are never overwritten")
    parser.add_argument("--ffprobe", default="ffprobe")
    parser.add_argument("--ffmpeg", default="ffmpeg")
    parser.add_argument("--timeout", type=float, default=120, help="seconds per subprocess")
    parser.add_argument("--comparator", type=Path, default=Path("target/debug/coreav-oracle-json"))
    parser.add_argument("--profile", choices=["fixtures", "real"], default="fixtures")
    candidate = parser.add_mutually_exclusive_group()
    candidate.add_argument("--candidate-exe", type=Path)
    candidate.add_argument("--candidate-dir", type=Path)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    if args.command == "run" and not (args.candidate_exe or args.candidate_dir):
        parser.error("run requires --candidate-exe or --candidate-dir")
    paths = fixtures(args.fixtures)
    output = fresh(args.output.resolve())
    if args.command == "capture":
        capture(paths, output, args)
        return 0
    reference = fresh(output / "reference")
    capture(paths, reference, args)
    if args.command == "run":
        if args.candidate_dir:
            candidate_dir = args.candidate_dir.resolve()
        else:
            candidate_dir = fresh(output / "candidate")
            for path in paths:
                result = invoke([str(args.candidate_exe.resolve()), str(path)], args.timeout)
                if result.returncode != 0:
                    raise RuntimeError(f"candidate failed for {path.name}: {result.stderr.decode(errors='replace')}")
                write(candidate_dir / (path.name + ".json"), json.loads(result.stdout))
        status, value = compare(reference, candidate_dir, args, output / "report.json")
        print(json.dumps(value, indent=2))
        return status
    candidate_dir = fresh(output / "candidate")
    # Independent captures exercise normalization, not just copying the oracle.
    capture(paths, candidate_dir, args)
    status, value = compare(reference, candidate_dir, args, output / "self-check.json")
    if status != 0 or any(value[level]["agreement_percent"] != 100 for level in ("streams", "packets", "frames")):
        raise RuntimeError("independent ffprobe/framemd5 captures did not give 100%")
    selected = next(path for path in candidate_dir.glob("*.json") if json.loads(path.read_text())["frames"])
    original = json.loads(selected.read_text())
    perturbations = {
        "stream": lambda v: v["streams"][0].update(width=(v["streams"][0]["width"] or 0) + 1),
        "packet": lambda v: v["packets"][0].update(pts=(v["packets"][0]["pts"] or 0) + 1),
        "missing-packet": lambda v: v["packets"].pop(0),
        "grouping": lambda v: v["packets"][0].update(size=(v["packets"][0]["size"] or 0) + 1),
        "frame": lambda v: v["frames"][0].update(md5="0" * 32),
        "probe-status": lambda v: v.update(probe_success=False),
        "schema": lambda v: v.pop("format_duration"),
    }
    for name, perturb in perturbations.items():
        altered = copy.deepcopy(original)
        perturb(altered)
        write(selected, altered)
        status, _ = compare(reference, candidate_dir, args, output / f"perturbed-{name}.json")
        if status != 1:
            raise RuntimeError(f"perturbed {name} unexpectedly passed")
    write(selected, original)
    print(json.dumps({"passed": True, "agreement_percent": 100,
                      "frames_checked": value["frames"]["agreeing"],
                      "perturbations_rejected": list(perturbations)}, indent=2))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(json.dumps({"error": str(error)}), file=sys.stderr)
        sys.exit(2)
