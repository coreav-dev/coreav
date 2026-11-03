"""Immutable prepared-source coverage, content bindings and post-merge preparation."""

import argparse
import sys
from collections import Counter
from fractions import Fraction
from pathlib import Path

from evidence import (
    digest,
    file_hash,
    integer,
    keys,
    load,
    relative,
    require,
    sha,
    write,
)

LIMIT = 16 * 1024**2
SALT = "coreav-encoders-v1:"
INPUT_FIELDS = ["id", "class", "source_sha256", "raw_sha256", "stream", "width", "height", "frames",
                "rate", "colour", "source_frame_sha256", "audio_start_ticks", "source_start_ticks", "source_tick"]
NATIVE_ROOTS = ("crates/coreav-backend-apple", "crates/coreav-h264",
                "crates/coreav-kernels-h264", "crates/coreav-aac")
NATIVE_FILES = {"Cargo.toml", "rust-toolchain.toml", "scripts/rust-dist.env"}


def reference(value, reviewed=False):
    keys(value, "file sha256 review_commit" if reviewed else "file sha256")
    relative(value["file"])
    digest(value["sha256"])
    if reviewed:
        from registration import revision
        revision(value["review_commit"])


def content_map(value):
    require(type(value) is dict and 0 < len(value) <= 8192, "freeze_content_map")
    for name, expected in value.items():
        relative(name)
        digest(expected)


def class_declaration(value):
    from registration import COVERED_CLASSES
    require(value["covered_classes"] == list(COVERED_CLASSES) and
            all(type(n) is int for n in value["covered_classes"]), "class_coverage")
    uncovered = value["uncovered_classes"]
    require(type(uncovered) is list and len(uncovered) == 1, "class_coverage")
    entry = uncovered[0]
    keys(entry, "class reason verified_inputs required_inputs failure_ledger")
    require(type(entry["class"]) is int and entry["class"] == 2160 and
            entry["reason"] == "insufficient_verified_fixed_pool_inputs" and
            type(entry["verified_inputs"]) is int and entry["verified_inputs"] == 1 and
            type(entry["required_inputs"]) is int and entry["required_inputs"] == 10,
            "class_coverage")
    reference(entry["failure_ledger"])
    return {"covered_classes": value["covered_classes"], "uncovered_classes": uncovered}


def fixture_frames(name):
    require(name in ("fixture", "fractional"), "fixture_identity")
    count = 60 if name == "fixture" else 4
    for frame in range(count):
        if name == "fixture":
            y = bytes(16 + (3 * x + 5 * row + 7 * frame) % 220
                      for row in range(64) for x in range(64))
        else:
            y = bytes([(37, 91, 145, 199)[frame]]) * 4096
        yield y + bytes([128]) * 2048


def fixtures():
    import hashlib

    from registration import COLOUR
    result = {}
    for name in ("fixture", "fractional"):
        whole, hashes = hashlib.sha256(), []
        for data in fixture_frames(name):
            whole.update(data)
            hashes.append(sha(data))
        rate = {"num": 30, "den": 1} if name == "fixture" else {"num": 30000, "den": 1001}
        result[name] = {"id": name, "class": 360, "width": 64, "height": 64,
                        "frames": len(hashes), "rate": rate, "colour": dict(COLOUR),
                        "raw_sha256": whole.hexdigest(), "source_frame_sha256": hashes,
                        "source_tick": {"num": rate["den"], "den": rate["num"]}}
    return result


def source_metadata(value, item):
    keys(value, "codec_name pixel_format field_order declared_frames observed_colour audio_starts scan")
    require(value["codec_name"] == "h264" and value["field_order"] == "progressive" and
            value["pixel_format"] == item["preparation"]["source_pixel_format"], "freeze_header")
    integer(value["declared_frames"], item["frames"], 2**31 - 1)
    observed = value["observed_colour"]
    require(type(observed) is dict and set(observed) <= set(item["preparation"]["source_colour"]),
            "freeze_observed_colour")
    assumed = {row["field"] for row in item["preparation"]["assumptions"]}
    require(set(observed).isdisjoint(assumed) and set(observed) | assumed ==
            set(item["preparation"]["source_colour"]), "freeze_assumption_as_observation")
    for name, entry in observed.items():
        require(entry == item["preparation"]["source_colour"][name], "freeze_colour_disagreement")
    require(type(value["audio_starts"]) is list and len(value["audio_starts"]) <= 1024,
            "freeze_audio_metadata")
    for row in value["audio_starts"]:
        keys(row, "stream start_pts time_base")
        integer(row["stream"], 0, 1024)
        require(row["start_pts"] is None or type(row["start_pts"]) is int, "freeze_audio_metadata")
        from registration import clock
        clock(row["time_base"])
    scan = value["scan"]
    keys(scan, "sps pps packet_count slice_count")
    require(type(scan["sps"]) is list and 0 < len(scan["sps"]) <= 32 and
            type(scan["pps"]) is list and 0 < len(scan["pps"]) <= 256, "freeze_scan")
    ids = set()
    for sps in scan["sps"]:
        keys(sps, "id frame_mbs_only_flag profile_idc level_idc")
        identity = integer(sps["id"], 0, 31)
        require(identity not in ids and type(sps["frame_mbs_only_flag"]) is int and
                sps["frame_mbs_only_flag"] == 1, "freeze_scan")
        ids.add(identity)
        integer(sps["profile_idc"], 1, 255)
        integer(sps["level_idc"], 1, 255)
    pps_ids = set()
    for pps in scan["pps"]:
        keys(pps, "id sps_id")
        identity = integer(pps["id"], 0, 255)
        require(identity not in pps_ids and type(pps["sps_id"]) is int and pps["sps_id"] in ids,
                "freeze_scan")
        pps_ids.add(identity)
    integer(scan["packet_count"], 1, 2**31 - 1)
    integer(scan["slice_count"], 1, item["frames"] * 4096)


def validate(value):
    from coreav_build import canonical
    from registration import (
        COLOUR,
        HOST,
        SCOPE,
        SETTINGS,
        clock,
        command_templates,
        covered_class,
    )
    keys(value, "schema scope covered_classes uncovered_classes plan_sha256 tools native_inputs "
         "settings environment command_controls commands host_profile oracle_sha256 metric_model "
         "verification selection inputs fixtures")
    require(value["schema"] == "coreav.videotoolbox-source-freeze/1" and value["scope"] == SCOPE,
            "source_freeze_schema")
    class_declaration(value)
    digest(value["plan_sha256"])
    for mapping in (value["tools"], value["native_inputs"]):
        content_map(mapping)
    require(canonical(value["settings"]) == canonical(SETTINGS) and
            canonical(value["host_profile"]) == canonical(HOST) and
            value["environment"] == {"OMP_NUM_THREADS": "1", "VECLIB_MAXIMUM_THREADS": "1"},
            "freeze_settings")
    keys(value["oracle_sha256"], "ffmpeg ffprobe")
    for expected in value["oracle_sha256"].values():
        digest(expected)
    keys(value["metric_model"], "name sha256")
    require(value["metric_model"]["name"] == "vmaf_v0.6.1", "metric_model")
    digest(value["metric_model"]["sha256"])
    keys(value["verification"], "method_sha256 plan_sha256 membership_sha256")
    for expected in value["verification"].values():
        digest(expected)
    reference(value["selection"])
    keys(value["command_controls"], "hardware software")
    keys(value["commands"], "hardware software")
    for cell in ("hardware", "software"):
        expected = ["-profile:v", "high", "-bf", "0", "-allow_sw", "1" if cell == "software" else "0",
                    "-require_sw", "1" if cell == "software" else "0"]
        require(value["command_controls"][cell] == expected and
                value["commands"][cell] == command_templates(value, cell), "freeze_commands")
    require(type(value["inputs"]) is list and len(value["inputs"]) == 30, "roster")
    ids, raw_ids, counts, ordering = set(), set(), Counter(), []
    for item in value["inputs"]:
        keys(item, " ".join(INPUT_FIELDS) + " source_bytes preparation source_metadata capture")
        height = covered_class(item)
        for field in ("id", "source_sha256", "raw_sha256"):
            digest(item[field])
        require(item["id"] == item["source_sha256"] and item["id"] not in ids and
                item["raw_sha256"] not in raw_ids, "freeze_duplicate_source_or_raw")
        ids.add(item["id"])
        raw_ids.add(item["raw_sha256"])
        counts[height] += 1
        ordering.append((height, sha((SALT + item["id"]).encode())))
        integer(item["source_bytes"], 1, 8 * 1024**3)
        integer(item["stream"], 0, 1024)
        rate = clock(item["rate"])
        frames = integer(item["frames"], 240, 1800)
        require(frames == max(240, (10 * rate.numerator + rate.denominator - 1) // rate.denominator)
                and 8 <= Fraction(frames, 1) / rate <= 30 and item["colour"] == COLOUR,
                "freeze_prefix")
        clock(item["source_tick"])
        integer(item["source_start_ticks"], -(2**53), 2**53)
        require(item["audio_start_ticks"] is None or type(item["audio_start_ticks"]) is int,
                "audio_clock")
        require(type(item["source_frame_sha256"]) is list and len(item["source_frame_sha256"]) == frames,
                "frame_coverage")
        for expected in item["source_frame_sha256"]:
            digest(expected)
        recipe = item["preparation"]
        keys(recipe, "source_pixel_format source_colour assumptions container_name field_order oracle_sha256")
        require(recipe["field_order"] == "progressive" and recipe["oracle_sha256"] == value["oracle_sha256"],
                "freeze_preparation_identity")
        from preparation import validate_recipe
        validate_recipe(item)
        source_metadata(item["source_metadata"], item)
        keys(item["capture"], "row_sha256 header_sha256 packets_sha256 frames_sha256 "
             "mapping_sha256 scan_sha256 preparation_sha256")
        for expected in item["capture"].values():
            digest(expected)
    require(dict(counts) == {360: 10, 720: 10, 1080: 10} and ordering == sorted(ordering), "roster_order")
    require(canonical(value["fixtures"]) == canonical(fixtures()), "freeze_fixture_definition")
    return value


def validate_selection(value, frozen):
    from registration import COVERED_CLASSES
    keys(value, "schema candidates")
    require(value["schema"] == "coreav.videotoolbox-selection/2" and
            type(value["candidates"]) is list and len(value["candidates"]) == 407, "freeze_selection")
    ids, by_class = set(), {height: [] for height in COVERED_CLASSES}
    for row in value["candidates"]:
        keys(row, "id class disposition reason evidence_sha256")
        digest(row["id"])
        digest(row["evidence_sha256"])
        require(row["id"] not in ids and type(row["class"]) is int and
                row["class"] in (0, 360, 720, 1080, 2160), "freeze_selection_identity")
        ids.add(row["id"])
        require(row["disposition"] in ("header_refused", "failed", "verified", "unvisited"),
                "freeze_selection_disposition")
        require(row["class"] != 0 or row["disposition"] == "header_refused", "freeze_selection_class")
        require((row["reason"] is None and row["disposition"] in ("verified", "unvisited")) or
                (row["disposition"] in ("header_refused", "failed") and type(row["reason"]) is str and
                 0 < len(row["reason"]) <= 256), "freeze_selection_reason")
        if row["class"] in by_class:
            by_class[row["class"]].append(row)
    require(sha("\n".join(sorted(ids)).encode()) == frozen["verification"]["membership_sha256"],
            "freeze_membership_changed")
    selected = []
    for height in COVERED_CLASSES:
        rows = by_class[height]
        require(rows == sorted(rows, key=lambda row: sha((SALT + row["id"]).encode())),
                "freeze_selection_order")
        chosen = []
        for row in rows:
            if len(chosen) == 10:
                require(row["disposition"] in ("unvisited", "header_refused"), "freeze_late_verification")
            elif row["disposition"] == "verified":
                chosen.append(row["id"])
            else:
                require(row["disposition"] != "unvisited", "freeze_incomplete_failure_prefix")
        require(len(chosen) == 10, "freeze_selection_shortfall")
        selected.extend(chosen)
    require(selected == [item["id"] for item in frozen["inputs"]], "freeze_selection_changed")


def validate_uncovered(value, selection):
    keys(value, "schema class required_inputs verified_inputs rows")
    require(value["schema"] == "coreav.videotoolbox-uncovered/1" and
            value["class"] == 2160 and type(value["class"]) is int and
            type(value["required_inputs"]) is int and value["required_inputs"] == 10 and
            type(value["verified_inputs"]) is int and value["verified_inputs"] == 1 and
            type(value["rows"]) is list and len(value["rows"]) == 22, "freeze_uncovered")
    ids, counts = set(), Counter()
    candidates = {row["id"]: row for row in selection["candidates"]}
    for row in value["rows"]:
        keys(row, "id disposition reason evidence_sha256")
        digest(row["id"])
        digest(row["evidence_sha256"])
        require(row["id"] not in ids, "freeze_uncovered_duplicate")
        ids.add(row["id"])
        require((row["disposition"] == "verified" and row["reason"] is None) or
                (row["disposition"] in ("failed", "reader_unsupported") and type(row["reason"]) is str),
                "freeze_uncovered_disposition")
        expected = "verified" if row["reason"] is None else "reader_unsupported" if (
            row["reason"] == "unsupported_codec_progressive_header_reader") else "failed"
        require(row["disposition"] == expected, "freeze_uncovered_disposition")
        candidate = candidates.get(row["id"])
        require(candidate is not None and candidate["class"] == 2160 and
                candidate["evidence_sha256"] == row["evidence_sha256"] and
                candidate["reason"] == row["reason"] and candidate["disposition"] ==
                ("header_refused" if expected == "reader_unsupported" else expected),
                "freeze_uncovered_membership")
        counts[row["reason"]] += 1
    require(ids == {row["id"] for row in selection["candidates"] if row["class"] == 2160},
            "freeze_uncovered_membership")
    require(counts == Counter({None: 1, "missing_prefix_pictures_or_packet_metadata": 5,
                              "prefix_picture_duration_not_cfr": 5,
                              "trace_warning_or_error_before_sanitization": 4,
                              "unsupported_codec_progressive_header_reader": 7}), "freeze_uncovered_reasons")


def native_paths(root, commit):
    from registration import git
    paths = set(git(root, "ls-tree", "-r", "--name-only", commit, "--", *NATIVE_ROOTS,
                    *sorted(NATIVE_FILES)).decode().splitlines())
    require(NATIVE_FILES <= paths and all(any(name.startswith(prefix + "/") for name in paths)
                                        for prefix in NATIVE_ROOTS), "freeze_native_coverage")
    return paths


def tool_paths(root):
    from registration import PLAN, git
    paths = {name for name in git(root, "ls-files", "scripts/encoders/macos").decode().splitlines()
             if name.endswith(".py") or name == PLAN}
    return paths | {"scripts/oracle-ffmpeg.env", "scripts/oracle_pin.py", "scripts/rust-dist.env",
                    "scripts/oracle-ffmpeg-macos.env", "scripts/oracle_pin_macos.py",
                    "scripts/install-oracle-ffmpeg-macos.sh", "scripts/install-oracle-ffmpeg.sh",
                    "docs/design/encoders.md", "docs/design/encoders/macos.md"}


def reviewed_file(root, value):
    from registration import committed, git
    reference(value, reviewed=True)
    git(root, "merge-base", "--is-ancestor", value["review_commit"], "origin/main")
    committed(root, value["file"], value["review_commit"])
    require(file_hash(Path(root) / value["file"], LIMIT, root=root) == value["sha256"], "freeze_file_changed")
    return load(Path(root) / value["file"], LIMIT)


def read_stage1(root, value):
    from registration import PLAN, committed
    frozen = validate(reviewed_file(root, value))
    require(file_hash(Path(root) / PLAN, root=root) == frozen["plan_sha256"], "plan_changed")
    require(set(frozen["tools"]) == tool_paths(root), "tool_coverage")
    for name, expected in frozen["tools"].items():
        committed(root, name, value["review_commit"])
        require(file_hash(Path(root) / name, root=root) == expected, "tool_changed")
    selection = reviewed_file(root, {**frozen["selection"], "review_commit": value["review_commit"]})
    validate_selection(selection, frozen)
    uncovered = reviewed_file(root, {**frozen["uncovered_classes"][0]["failure_ledger"],
                                     "review_commit": value["review_commit"]})
    validate_uncovered(uncovered, selection)
    return frozen


def verify_seal(registration, root):
    from registration import coverage, git
    ref = registration["source_freeze"]
    frozen = read_stage1(root, ref)
    declaration = coverage(registration)
    normalized = {"covered_classes": declaration["covered_classes"], "uncovered_classes": []}
    for entry in declaration["uncovered_classes"]:
        require(entry["failure_ledger"]["review_commit"] == ref["review_commit"], "freeze_uncovered_review")
        normalized["uncovered_classes"].append({**entry, "failure_ledger": {
            key: entry["failure_ledger"][key] for key in ("file", "sha256")}})
    require(normalized == class_declaration(frozen), "freeze_coverage_changed")
    for field in ("plan_sha256", "tools", "settings", "environment", "command_controls", "commands"):
        require(registration[field] == frozen[field], "freeze_immutable_changed")
    require({key: registration["host"][key] for key in frozen["host_profile"]} == frozen["host_profile"],
            "freeze_host_profile_changed")
    require(registration["eligibility"] == {**frozen["selection"], "cutoff_commit": ref["review_commit"]},
            "freeze_selection_reference")
    require([{key: item[key] for key in INPUT_FIELDS} for item in registration["inputs"]] ==
            [{key: item[key] for key in INPUT_FIELDS} for item in frozen["inputs"]], "freeze_source_changed")
    require({name: {key: item[key] for key in item if key != "raw_file"}
             for name, item in registration["fixtures"].items()} == frozen["fixtures"], "freeze_fixture_changed")
    for name in ("ffmpeg", "ffprobe"):
        require(registration["metric_tools"][name] == frozen["oracle_sha256"][name], "freeze_oracle_changed")
    require(registration["metric_tools"]["model"] == frozen["metric_model"]["sha256"], "freeze_model_changed")
    expected_paths = native_paths(root, registration["candidate"])
    require(set(frozen["native_inputs"]) == expected_paths, "freeze_native_coverage")
    for name, expected in frozen["native_inputs"].items():
        require(sha(git(root, "show", registration["candidate"] + ":" + name)) == expected,
                "freeze_native_changed")
    for name, expected in frozen["tools"].items():
        require(sha(git(root, "show", registration["tooling"] + ":" + name)) == expected,
                "freeze_tooling_changed")
    return frozen


def main():
    from registration import ROOT, git
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--freeze", required=True, type=Path)
    parser.add_argument("--review-commit", required=True)
    parser.add_argument("--source-map", type=Path)
    parser.add_argument("--oracle-bin", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    path = args.freeze.resolve(strict=True)
    require(path.is_relative_to(root), "freeze_path")
    require(not git(root, "status", "--porcelain", "--untracked-files=all"), "dirty_checkout")
    main_revision = git(root, "rev-parse", "origin/main").decode().strip()
    remote = git(root, "ls-remote", "origin", "refs/heads/main").decode().split()
    require(len(remote) == 2 and remote[0] == main_revision, "stale_main")
    frozen = read_stage1(root, {"file": path.relative_to(root).as_posix(),
                               "sha256": file_hash(path), "review_commit": args.review_commit})
    if args.source_map is None:
        require(args.oracle_bin is None and args.output is None, "preparation_arguments")
        print("source freeze verified; complete reviewed seal is still required for encoding")
        return
    require(sys.platform == "linux" and args.oracle_bin is not None and args.output is not None,
            "preparation_platform_or_arguments")
    source_map = load(args.source_map, LIMIT)
    require(type(source_map) is dict and set(source_map) == {item["id"] for item in frozen["inputs"]},
            "preparation_source_map")
    output = args.output.resolve()
    require(not output.is_relative_to(root) and not output.exists(), "preparation_output")
    output.mkdir()
    from preparation import materialize_one
    for item in frozen["inputs"]:
        materialize_one(source_map[item["id"]], item, args.oracle_bin, output / item["id"])
    for name, item in frozen["fixtures"].items():
        directory = output / name
        directory.mkdir()
        target = directory / "source.yuv"
        with target.open("xb") as raw:
            for data in fixture_frames(name):
                raw.write(data)
        require(file_hash(target) == item["raw_sha256"], "fixture_hash")
    write(output / "materialized.json", {"schema": "coreav.videotoolbox-materialized/1",
                                         "source_freeze_sha256": file_hash(path),
                                         "covered_classes": frozen["covered_classes"],
                                         "inputs": [item["id"] for item in frozen["inputs"]]})


if __name__ == "__main__":
    main()
