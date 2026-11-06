#!/usr/bin/env python3
"""Generate only deterministic thumbnail preflight inputs, outside the checkout.

Linux only: --oracle-bin DIR --x264 FILE --output DIR [--hostile-manifest FILE].
--plan prints the complete pinned invocation plan without running any program.
"""

import argparse
import hashlib
import json
import pathlib
import re
import struct
import subprocess
import sys

import oracle_pin
import test_oracle

ROOT = pathlib.Path(__file__).resolve().parent.parent
ROSTER = ROOT / "packages/wasm-h264/test/installed/roster.json"


class GenerationFailure(ValueError):
    def __init__(self, stage, item):
        self.stage = stage if stage in {
            "commands", "pattern", "encode", "inspect-sps", "mux", "variant", "remove-pattern",
            "probe", "codec", "frame-count", "keyframes", "rotation", "geometry", "external", "manifest",
        } else "unexpected"
        self.item = item if type(item) is int and item >= 0 else 0
        super().__init__(self.stage, self.item)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def registry():
    return json.loads(ROSTER.read_text())


def fixture_rows():
    roster = registry()
    for geometry in [g["id"] for g in roster["geometries"]] + roster["variants"] + ["4k", "4k-small-container"]:
        for entropy in roster["entropy"]:
            for container in roster["containers"]:
                yield {"id": f"{geometry}-{entropy}-{container}", "geometry": geometry,
                       "entropy": entropy, "container": container}


def commands(oracle, x264, output):
    roster = registry()
    ffmpeg = str(oracle.ffmpeg)
    geometries = roster["geometries"] + [{"id": "4k", "width": 3840, "height": 2160},
                                         {"id": "sar", "width": 640, "height": 360, "sar": "4:3"}]
    for geometry in geometries:
        y4m = output / (geometry["id"] + ".y4m")
        yield {"kind": "pattern", "geometry": geometry["id"], "argv": [
            ffmpeg, "-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-threads", "1",
            "-f", "lavfi", "-i", f"testsrc2=size={geometry['width']}x{geometry['height']}:rate={roster['fps']}",
            "-frames:v", str(roster["frames"]), "-an", "-pix_fmt", "yuv420p", "-f", "yuv4mpegpipe", str(y4m)]}
        for entropy in roster["entropy"]:
            elementary = output / f"{geometry['id']}-{entropy}.264"
            args = [str(x264), "--demuxer", "y4m", "--preset", "veryfast", "--crf", "24",
                    "--threads", "1", "--lookahead-threads", "1", "--frames", str(roster["frames"]),
                    "--fps", str(roster["fps"]), "--keyint", str(roster["keyint"]),
                    "--min-keyint", str(roster["keyint"]), "--scenecut", "0", "--bframes", "0", "--ref", "1"]
            if entropy == "cavlc":
                args += ["--no-cabac"]
            if "crop" in geometry:
                args += ["--crop-rect", geometry["crop"]]
            if "sar" in geometry:
                args += ["--sar", geometry["sar"]]
            args += ["--output", str(elementary), str(y4m)]
            yield {"kind": "encode", "geometry": geometry["id"], "entropy": entropy, "argv": args}
            yield {"kind": "inspect-sps", "geometry": geometry["id"], "entropy": entropy, "argv": [
                ffmpeg, "-hide_banner", "-nostdin", "-i", str(elementary), "-map", "0:v:0", "-an", "-c:v", "copy",
                "-bsf:v", "trace_headers", "-frames:v", "1", "-f", "null", "-"]}
            for container in roster["containers"]:
                extension = "mp4" if container == "mp4" else "mkv"
                target = output / f"{geometry['id']}-{entropy}-{container}.{extension}"
                yield {"kind": "mux", "geometry": geometry["id"], "argv": [
                    ffmpeg, "-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-fflags", "+bitexact",
                    "-r", str(roster["fps"]), "-i", str(elementary), "-map", "0:v:0", "-an", "-c:v", "copy",
                    "-map_metadata", "-1", "-fflags", "+bitexact", "-flags:v", "+bitexact", *(["-write_crc32", "0"] if container == "matroska" else []), "-f", container, str(target)]}
        yield {"kind": "remove-pattern", "geometry": geometry["id"], "path": str(y4m)}
    for row in fixture_rows():
        variant = row["geometry"]
        if variant not in roster["variants"] or variant == "sar":
            continue
        extension = "mp4" if row["container"] == "mp4" else "mkv"
        source = output / f"360p-{row['entropy']}-{row['container']}.{extension}"
        args = [ffmpeg, "-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-fflags", "+bitexact"]
        if variant == "edits":
            args += ["-itsoffset", "0.08"]
        if variant.startswith("rotate"):
            args += ["-display_rotation:v:0", variant[6:]]
        args += ["-i", str(source), "-map", "0:v:0"]
        if variant == "stream-ties":
            args += ["-map", "0:v:0"]
        args += ["-an", "-c:v", "copy", "-map_metadata", "-1", "-fflags", "+bitexact", "-flags:v", "+bitexact"]
        args += [*(["-write_crc32", "0"] if row["container"] == "matroska" else []), "-f", row["container"], str(output / f"{row['id']}.{extension}")]
        yield {"kind": "variant", "geometry": variant, "argv": args}


def patch_dimensions(data, container, width=640, height=360):
    out = bytearray(data)
    changed = set()
    if container == "mp4":
        def boxes(start, end):
            at = start
            while at < end:
                if at + 8 > end:
                    raise ValueError("truncated generated box")
                size, kind = struct.unpack_from(">I4s", out, at)
                header = 8
                if size == 1:
                    if at + 16 > end:
                        raise ValueError("truncated large generated box")
                    size = struct.unpack_from(">Q", out, at + 8)[0]
                    header = 16
                elif size == 0:
                    size = end - at
                if size < header or at + size > end:
                    raise ValueError("invalid generated box size")
                if kind == b"tkhd":
                    struct.pack_into(">II", out, at + size - 8, width << 16, height << 16)
                    changed.add("track")
                elif kind in (b"avc1", b"avc3"):
                    if size < header + 28:
                        raise ValueError("truncated generated visual entry")
                    struct.pack_into(">HH", out, at + header + 24, width, height)
                    changed.add("entry")
                elif kind in (b"moov", b"trak", b"mdia", b"minf", b"stbl"):
                    boxes(at + header, at + size)
                elif kind == b"stsd":
                    boxes(at + header + 8, at + size)
                at += size
        boxes(0, len(out))
        if changed != {"track", "entry"}:
            raise ValueError("generated MP4 lacks both stored geometry owners")
    elif container == "matroska":
        def vint(at, end, marker=False):
            if at >= end or out[at] == 0:
                raise ValueError("invalid generated EBML integer")
            count = 9 - out[at].bit_length()
            if at + count > end:
                raise ValueError("truncated generated EBML integer")
            value = int.from_bytes(out[at:at + count], "big")
            return (value if marker else value & ((1 << (7 * count)) - 1)), count
        def elements(start, end):
            at = start
            while at < end:
                kind, n = vint(at, end, True)
                size, m = vint(at + n, end)
                body = at + n + m
                stop = end if size == (1 << (7 * m)) - 1 else body + size
                if stop > end:
                    raise ValueError("truncated generated EBML element")
                if kind in (0xB0, 0xBA, 0x54B0, 0x54BA):
                    value = width if kind in (0xB0, 0x54B0) else height
                    out[body:stop] = value.to_bytes(stop - body, "big")
                    changed.add(kind)
                elif kind in (0x18538067, 0x1654AE6B, 0xAE, 0xE0):
                    elements(body, stop)
                at = stop
        elements(0, len(out))
        if not {0xB0, 0xBA}.issubset(changed):
            raise ValueError("generated Matroska lacks stored geometry")
    else:
        raise ValueError("unknown container")
    return bytes(out)


def execute(argv, logs, deadline=300):
    result = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            timeout=deadline, check=False)
    logs.append({"argv": argv, "exit": result.returncode, "stderr": result.stderr.decode("utf8", "replace")})
    if result.returncode:
        raise ValueError("fixture subprocess failed; inspect private generation log")
    return result.stdout


def hostile_rows(path):
    if path is None:
        raise ValueError("the source-owned hostile and boundary manifest is required")
    rows = json.loads(path.read_text())
    required = registry()["hostile"] + registry()["boundaries"]
    if not isinstance(rows, list) or {row["id"] for row in rows} != set(required) or len(rows) != len(required):
        raise ValueError("hostile and valid boundary fixture roster is incomplete or duplicated")
    for row in rows:
        if not isinstance(row.get("options"), dict) or not isinstance(row.get("expectation"), str):
            raise ValueError("source-owned fixture expectations are missing")
        relative = pathlib.Path(row["path"])
        if relative.is_absolute() or ".." in relative.parts or not relative.parts:
            raise ValueError("source-owned fixture paths must be relative and contained")
        source = (path.parent / row["path"]).resolve()
        if digest(source.read_bytes()) != row["sha256"]:
            raise ValueError("source-owned fixture bytes changed")
    return rows


def generate(oracle, x264, output, hostile_manifest=None):
    if sys.platform != "linux":
        raise ValueError("media generation is Linux-only")
    output = output.resolve()
    if output == ROOT or ROOT in output.parents:
        raise ValueError("generated files must stay outside the checkout")
    external = hostile_rows(hostile_manifest)
    if output.exists() and any(output.iterdir()):
        raise ValueError("output directory must be empty")
    try:
        oracle = test_oracle.require(oracle / "ffmpeg", oracle / "ffprobe")
    except test_oracle.QualificationError:
        raise ValueError("the pinned oracle programs are required") from None
    output.mkdir(parents=True, exist_ok=True)
    logs = []
    pin = oracle_pin.load()
    version = execute([str(x264), "--version"], logs, 10).decode("utf8", "replace")
    if pin["X264_COMMIT"][:7] not in version:
        raise ValueError("x264 does not match the shared oracle pin")
    manifest = {"schema": registry()["schema"], "rosterSha256": digest(ROSTER.read_bytes()),
                "generatorSha256": digest(pathlib.Path(__file__).read_bytes()),
                "oraclePinSha256": digest((ROOT / oracle_pin.PIN).read_bytes()),
                "binaries": {name: digest(path.read_bytes()) for name, path in
                             [("ffmpeg", oracle.ffmpeg), ("ffprobe", oracle.ffprobe), ("x264", x264)]},
                "fixtures": [], "hostile": [], "boundary": []}
    stage, item = "commands", 0
    try:
        plan = list(commands(oracle, x264, output))
        grids = {}
        for item, command in enumerate(plan, 1):
            stage = command["kind"]
            if command["kind"] == "remove-pattern":
                pathlib.Path(command["path"]).unlink()
            else:
                execute(command["argv"], logs)
                if command["kind"] == "inspect-sps":
                    fields = {}
                    for name in ("pic_width_in_mbs_minus1", "pic_height_in_map_units_minus1", "frame_mbs_only_flag", "entropy_coding_mode_flag"):
                        values = set(int(value) for value in re.findall(r"\b" + name + r"\s+[01]+\s*=\s*(\d+)", logs[-1]["stderr"]))
                        if len(values) != 1:
                            raise ValueError("pinned oracle did not expose one consistent SPS grid")
                        fields[name] = values.pop()
                    if fields["frame_mbs_only_flag"] != 1:
                        raise ValueError("generated fixture is not progressive")
                    if fields["entropy_coding_mode_flag"] != (command["entropy"] == "cabac"):
                        raise ValueError("generated fixture entropy mode drifted")
                    grids[(command["geometry"], command["entropy"])] = (
                        16 * (fields["pic_width_in_mbs_minus1"] + 1),
                        16 * (fields["pic_height_in_map_units_minus1"] + 1))
        for item, row in enumerate(fixture_rows(), 1):
            stage = "geometry"
            extension = "mp4" if row["container"] == "mp4" else "mkv"
            path = output / f"{row['id']}.{extension}"
            if row["geometry"] == "4k-small-container":
                original = output / f"4k-{row['entropy']}-{row['container']}.{extension}"
                path.write_bytes(patch_dimensions(original.read_bytes(), row["container"]))
            stage = "probe"
            probe = execute([str(oracle.ffprobe), "-v", "error", "-select_streams", "v", "-show_streams",
                             "-show_packets", "-show_entries", "stream=index,codec_name,width,height,pix_fmt,sample_aspect_ratio:stream_side_data:packet=stream_index,pts_time,flags",
                             "-of", "json", str(path)], logs, 30)
            description = json.loads(probe)
            streams = description.get("streams", [])
            stage = "codec"
            if not streams or any(stream.get("codec_name") != "h264" or stream.get("pix_fmt") != "yuv420p" for stream in streams):
                raise ValueError("generated fixture codec or pixel format drifted")
            stage = "frame-count"
            if len(description.get("packets", [])) != 50 * len(streams):
                raise ValueError("generated fixture does not have fifty frames per stream")
            stage = "keyframes"
            for stream in streams:
                packets = [p for p in description["packets"] if p["stream_index"] == stream["index"]]
                if [i for i, packet in enumerate(packets) if "K" in packet["flags"]] != [0, 25]:
                    raise ValueError("generated fixture keyframe cadence drifted")
            stage = "rotation"
            if row["geometry"].startswith("rotate"):
                angles = [item.get("rotation") for stream in streams for item in stream.get("side_data_list", [])]
                if not any(isinstance(angle, (int, float)) and angle % 360 == int(row["geometry"][6:]) % 360 for angle in angles):
                    raise ValueError("generated rotation was not retained by the pinned muxer")
            stage = "geometry"
            geometry = row["geometry"]
            original = "4k" if geometry == "4k-small-container" else geometry
            if original in registry()["variants"] and original != "sar":
                original = "360p"
            width, height = grids[(original, row["entropy"])]
            manifest["fixtures"].append({**row, "path": path.name, "sha256": digest(path.read_bytes()),
                                         "options": {"at": "1", "width": 320, "format": "jpeg"},
                                         "codedWidth": width, "codedHeight": height,
                                         "visibleWidth": streams[0]["width"], "visibleHeight": streams[0]["height"],
                                         "probe": description})
        stage = "external"
        for item, row in enumerate(external, 1):
            source = (hostile_manifest.parent / row["path"]).resolve()
            data = source.read_bytes()
            if digest(data) != row["sha256"]:
                raise ValueError("source-owned fixture bytes changed during generation")
            target = output / ("hostile-" + row["id"] + source.suffix)
            target.write_bytes(data)
            kind = "boundary" if row["id"] in registry()["boundaries"] else "hostile"
            manifest[kind].append({**row, "path": target.name})
        stage = "manifest"
        manifest["generationSha256"] = digest((json.dumps({"commands": logs}, sort_keys=True, indent=2) + "\n").encode())
        (output / "manifest.json").write_text(json.dumps(manifest, sort_keys=True, indent=2) + "\n")
    except (ValueError, OSError, subprocess.SubprocessError, KeyError, TypeError, OverflowError, struct.error) as error:
        raise GenerationFailure(stage, item) from error
    finally:
        (output / "generation.json").write_text(json.dumps({"commands": logs}, sort_keys=True, indent=2) + "\n")
    return len(manifest["fixtures"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oracle-bin", type=pathlib.Path, required=True)
    parser.add_argument("--x264", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    parser.add_argument("--hostile-manifest", type=pathlib.Path)
    parser.add_argument("--plan", action="store_true")
    args = parser.parse_args()
    if args.plan:
        oracle = test_oracle.Oracle(args.oracle_bin / "ffmpeg", args.oracle_bin / "ffprobe")
        print(json.dumps({"roster": list(fixture_rows()), "commands": list(commands(oracle, args.x264, args.output))}, sort_keys=True))
        return
    try:
        count = generate(args.oracle_bin.resolve(), args.x264.resolve(), args.output, args.hostile_manifest)
    except GenerationFailure as failure:
        print(f"thumbnail fixtures: failed stage={failure.stage} item={failure.item}; inspect private generation records", file=sys.stderr)
        raise SystemExit(1) from None
    except (ValueError, OSError, subprocess.SubprocessError, KeyError, TypeError, OverflowError, struct.error):
        print("thumbnail fixtures: failed; inspect private generation records", file=sys.stderr)
        raise SystemExit(1) from None
    print(f"thumbnail fixtures: {count} generated (preflight inputs, not a verdict)")


if __name__ == "__main__":
    main()
