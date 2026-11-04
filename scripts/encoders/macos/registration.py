"""Pre-input gates for the fixed physical reference host."""

from fractions import Fraction
import os
from pathlib import Path
import plistlib
import re
import shlex
import sys
import tempfile

from evidence import Invalid, digest, file_hash, integer, keys, load, read, relative
from evidence import require, sha, token
from process import run

ROOT = Path(__file__).resolve().parents[3]
PLAN = "scripts/encoders/macos/videotoolbox-plan.md"
CLASSES = ((360, 1000000), (720, 3000000), (1080, 6000000), (2160, 20000000))
COVERED_CLASSES = (360, 720, 1080)
SCOPE = "prepared-raw-i420-encoder-only"
COLOUR = {"primaries": "bt709", "transfer": "bt709", "matrix": "bt709", "range": "limited"}
SETTINGS = {"profile": "high", "reordering": False, "keyframe_seconds": 2,
            "cadence_hint": "nearest_whole_fps_ties_up",
            "pixel_format": "yuv420p", "warmups": 1, "pairs": 5,
            "bootstrap_resamples": 10000, "bootstrap_seed": 0,
            "output_storage": "ram", "input_cache": "warm"}
HOST = {"profile": "ENC-MAC-1", "provider": "scaleway", "type": "M1-M",
        "model": "Macmini9,1", "cpu": "Apple M1", "cores": 8, "memory": 8589934592,
        "os_version": "15.7", "virtual": False, "dedicated": True}


def clock(value):
    keys(value, "num den")
    return Fraction(integer(value["num"], 1, 2**31 - 1),
                    integer(value["den"], 1, 2**31 - 1))


def cadence_hint(item):
    rate = item["rate"]
    return (2 * rate["num"] + rate["den"]) // (2 * rate["den"])


def revision(value):
    require(type(value) is str and re.fullmatch(r"[0-9a-f]{40}", value), "revision")


def coverage(value):
    require(type(value) is dict and "covered_classes" in value and "uncovered_classes" in value,
            "class_coverage")
    covered = value["covered_classes"]
    require(type(covered) is list and covered == list(COVERED_CLASSES) and
            all(type(height) is int for height in covered), "class_coverage")
    uncovered = value["uncovered_classes"]
    require(type(uncovered) is list and len(uncovered) == 1, "class_coverage")
    entry = uncovered[0]
    keys(entry, "class reason verified_inputs required_inputs failure_ledger")
    require(type(entry["class"]) is int and entry["class"] == 2160 and
            entry["reason"] == "insufficient_verified_fixed_pool_inputs" and
            type(entry["verified_inputs"]) is int and entry["verified_inputs"] == 1 and
            type(entry["required_inputs"]) is int and entry["required_inputs"] == 10,
            "class_coverage")
    keys(entry["failure_ledger"], "file sha256 review_commit")
    relative(entry["failure_ledger"]["file"])
    digest(entry["failure_ledger"]["sha256"])
    revision(entry["failure_ledger"]["review_commit"])
    return {"covered_classes": covered, "uncovered_classes": uncovered}


def covered_class(item):
    require(type(item) is dict and "class" in item and "width" in item and "height" in item,
            "height_class")
    integer(item["width"], 2, 3840)
    integer(item["height"], 2, 2160)
    require(item["width"] % 2 == item["height"] % 2 == 0, "geometry")
    height = next(bound for bound, _ in CLASSES if item["height"] <= bound)
    require(type(item["class"]) is int and item["class"] == height and height in COVERED_CLASSES,
            "uncovered_class")
    return height


def roster_coverage(value):
    declaration = coverage(value)
    require(type(value.get("inputs")) is list and len(value["inputs"]) == 30, "roster")
    counts = {height: 0 for height in COVERED_CLASSES}
    seen = set()
    for item in value["inputs"]:
        height = covered_class(item)
        digest(item.get("id"))
        require(item["id"] not in seen, "roster_identity")
        seen.add(item["id"])
        counts[height] += 1
    require(all(count == 10 for count in counts.values()), "class_coverage")
    return declaration


def validate(value):
    keys(value, "schema candidate tooling plan_sha256 tools host executables baseline_build coreav_builds "
         "settings environment cells inputs fixtures metric_tools session_id command_controls commands eligibility cache_observer "
         "covered_classes uncovered_classes source_freeze")
    require(value["schema"] == "coreav.videotoolbox-registration/2", "schema")
    roster_coverage(value)
    from source_freeze import reference
    reference(value["source_freeze"], reviewed=True)
    revision(value["candidate"])
    revision(value["tooling"])
    from cache_residency import declaration
    declaration(value["cache_observer"])
    keys(value["eligibility"], "file sha256 cutoff_commit")
    relative(value["eligibility"]["file"])
    digest(value["eligibility"]["sha256"])
    revision(value["eligibility"]["cutoff_commit"])
    digest(value["plan_sha256"])
    token(value["session_id"])
    require(type(value["tools"]) is dict and value["tools"], "tools")
    for path, expected in value["tools"].items():
        relative(path)
        digest(expected)
    keys(value["host"], "profile provider type model cpu cores memory os_version os_build "
         "virtual dedicated allocation_sha256 hardware_sha256")
    require(all(type(value["host"][k]) is type(v) and value["host"][k] == v
                for k, v in HOST.items()), "host")
    token(value["host"]["os_build"])
    digest(value["host"]["allocation_sha256"])
    digest(value["host"]["hardware_sha256"])
    require(value["settings"] == SETTINGS and
            all(type(value["settings"][k]) is type(v) for k, v in SETTINGS.items()), "settings")
    require(type(value["environment"]) is dict and set(value["environment"]) ==
            {"OMP_NUM_THREADS", "VECLIB_MAXIMUM_THREADS"} and
            all(v == "1" for v in value["environment"].values()), "environment")
    keys(value["executables"], "coreav ffmpeg")
    for record in value["executables"].values():
        keys(record, "file sha256 configuration_sha256")
        relative(record["file"])
        digest(record["sha256"])
        digest(record["configuration_sha256"])
    keys(value["baseline_build"], "file sha256 review_commit")
    relative(value["baseline_build"]["file"])
    digest(value["baseline_build"]["sha256"])
    revision(value["baseline_build"]["review_commit"])
    keys(value["coreav_builds"], "macos linux")
    for record in value["coreav_builds"].values():
        keys(record, "file sha256 review_commit")
        relative(record["file"])
        digest(record["sha256"])
        revision(record["review_commit"])
    require(value["coreav_builds"]["macos"]["file"] != value["coreav_builds"]["linux"]["file"],
            "coreav_build_records")
    require(type(value["cells"]) is list and value["cells"] in
            (["hardware"], ["hardware", "software"]), "cells")
    keys(value["command_controls"], "hardware software")
    for cell, arguments in value["command_controls"].items():
        require(type(arguments) is list and all(type(arg) is str for arg in arguments), "controls")
        require(arguments == ["-profile:v", "high", "-bf", "0", "-allow_sw", "1" if cell == "software" else "0",
                              "-require_sw", "1" if cell == "software" else "0"], "controls")
    seen = set()
    ordering = []
    for item in value["inputs"]:
        keys(item, "id class source_sha256 raw_file raw_sha256 stream width height frames rate "
             "colour source_frame_sha256 audio_start_ticks source_start_ticks source_tick")
        digest(item["id"])
        digest(item["source_sha256"])
        require(item["id"] == item["source_sha256"] and item["id"] not in seen, "roster_identity")
        seen.add(item["id"])
        relative(item["raw_file"])
        digest(item["raw_sha256"])
        integer(item["stream"], 0, 1024)
        height = covered_class(item)
        frames = integer(item["frames"], 240, 1800)
        rate = clock(item["rate"])
        require(8 <= Fraction(frames, 1) / rate <= 30, "duration")
        require(item["colour"] == COLOUR, "colour")
        require(type(item["source_frame_sha256"]) is list and
                len(item["source_frame_sha256"]) == frames, "frame_coverage")
        for expected in item["source_frame_sha256"]:
            digest(expected)
        clock(item["source_tick"])
        integer(item["source_start_ticks"], -(2**53), 2**53)
        require(item["audio_start_ticks"] is None or
                type(item["audio_start_ticks"]) is int, "audio_clock")
        ordering.append((height, sha(("coreav-encoders-v1:" + item["source_sha256"]).encode())))
    require(ordering == sorted(ordering), "roster_order")
    require(type(value["fixtures"]) is dict and value["fixtures"], "fixtures")
    for fixture, item in value["fixtures"].items():
        token(fixture)
        keys(item, "id class width height frames rate colour raw_file raw_sha256 source_frame_sha256 source_tick")
        require(item["id"] == fixture, "fixture_identity")
        covered_class(item)
        integer(item["frames"], 1, 1800)
        clock(item["rate"])
        clock(item["source_tick"])
        require(item["colour"] == COLOUR and type(item["source_frame_sha256"]) is list and
                len(item["source_frame_sha256"]) == item["frames"], "fixture_frames")
        digest(item["raw_sha256"])
        relative(item["raw_file"])
        for expected in item["source_frame_sha256"]:
            digest(expected)
    keys(value["metric_tools"], "ffmpeg ffprobe coreav_decoder metrics model evidence_producer")
    for expected in value["metric_tools"].values():
        digest(expected)
    require(type(value["commands"]) is dict and set(value["commands"]) == {"hardware", "software"}, "commands")
    for cell in ("hardware", "software"):
        require(value["commands"][cell] == command_templates(value, cell), "commands")
    return value


def command_templates(registration, cell):
    return {
        "coreav": ["{coreav}", "encode", "--input", "{raw}", "--width", "{width}",
                   "--height", "{height}", "--rate-num", "{rate_num}", "--rate-den", "{rate_den}",
                   "--frames", "{frames}", "--bitrate", "{bitrate}", "--mode", cell,
                   "--output", "{output}", "--report", "{report}"],
        "ffmpeg": ["{ffmpeg}", "-nostdin", "-v", "trace", "-xerror", "-threads", "1",
                   "-f", "rawvideo", "-pixel_format", "yuv420p", "-video_size", "{geometry}",
                   "-framerate", "{rate}", "-color_primaries", "bt709", "-color_trc", "bt709",
                   "-colorspace", "bt709", "-color_range", "tv", "-i", "{raw}",
                   "-map", "0:v:0", "-an", "-frames:v", "{frames}", "-c:v", "h264_videotoolbox",
                   "-b:v", "{bitrate}", "-g", "{keyframe_frames}", "-force_key_frames",
                   "expr:gte(t,n_forced*2)", *registration["command_controls"][cell],
                   "-color_primaries", "bt709", "-color_trc", "bt709", "-colorspace", "bt709",
                   "-color_range", "tv", "-fps_mode", "passthrough", "-stats_enc_post", "{report}",
                   "-stats_enc_post_fmt", "{pts} {tb} {dts} {size}\n", "-f", "h264", "{output}"]}


def caller_settings(registration):
    return {"coreav_owner_threads": 1, "ffmpeg_input_decoder_threads": 1,
            "environment": dict(registration["environment"])}


def git(root, *arguments):
    with tempfile.TemporaryDirectory() as temporary:
        run(["git", "-C", str(root), *arguments], temporary, 30)
        return read(temporary, "stdout", 16 * 1024**2)


def committed(root, path, commit):
    relative(path)
    revision(commit)
    require(git(root, "show", f"{commit}:{path}") == read(root, path, 16 * 1024**2),
            "uncommitted_registration")


def hardware_fingerprint():
    require(sys.platform == "darwin", "host_platform")
    with tempfile.TemporaryDirectory() as temporary:
        run(["/usr/sbin/ioreg", "-r", "-d", "1", "-c", "IOPlatformExpertDevice", "-a"], temporary, 10,
            stdout="identity", stderr="identity-error")
        try:
            value = plistlib.loads(read(temporary, "identity", 1024**2))
        except (ValueError, plistlib.InvalidFileException):
            raise Invalid("host_fingerprint") from None
        require(type(value) is list and len(value) == 1 and type(value[0]) is dict, "host_fingerprint")
        identity = value[0].get("IOPlatformUUID")
        require(type(identity) is str and re.fullmatch(
            r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", identity) is not None, "host_fingerprint")
        require(identity.lower().replace("-", "") not in ("0" * 32, "f" * 32), "host_fingerprint")
        return sha(("coreav.videotoolbox-host/1:" + identity.lower()).encode("ascii"))


def host_observation():
    require(sys.platform == "darwin", "host_platform")
    commands = {
        "model": ["/usr/sbin/sysctl", "-n", "hw.model"],
        "cpu": ["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"],
        "cores": ["/usr/sbin/sysctl", "-n", "hw.physicalcpu"],
        "memory": ["/usr/sbin/sysctl", "-n", "hw.memsize"],
        "virtual": ["/usr/sbin/sysctl", "-n", "kern.hv_vmm_present"],
        "os_version": ["/usr/bin/sw_vers", "-productVersion"],
        "os_build": ["/usr/bin/sw_vers", "-buildVersion"],
    }
    result = {}
    with tempfile.TemporaryDirectory() as temporary:
        for key, command in commands.items():
            run(command, temporary, 10, stdout=key, stderr=key + "-error")
            text = read(temporary, key, 256).decode().strip()
            result[key] = int(text) if key in ("cores", "memory", "virtual") else text
    require(result["virtual"] == 0, "virtual_host")
    result["virtual"] = False
    result["hardware_sha256"] = hardware_fingerprint()
    return result


def source_registration(registration_path, root=ROOT):
    root = Path(root).resolve(strict=True)
    path = Path(registration_path).resolve(strict=True)
    require(path.is_relative_to(root), "registration_path")
    require(not git(root, "status", "--porcelain", "--untracked-files=all"), "dirty_checkout")
    registration = validate(load(path, 16 * 1024**2))
    head = git(root, "rev-parse", "HEAD").decode().strip()
    committed(root, path.relative_to(root).as_posix(), head)
    main = git(root, "rev-parse", "origin/main").decode().strip()
    remote = git(root, "ls-remote", "origin", "refs/heads/main").decode().split()
    require(len(remote) == 2 and remote[0] == main, "stale_main")
    for commit in (registration["candidate"], registration["tooling"], head,
                   registration["baseline_build"]["review_commit"], registration["eligibility"]["cutoff_commit"],
                   *(record["review_commit"] for record in registration["coreav_builds"].values())):
        git(root, "merge-base", "--is-ancestor", commit, main)
    require(file_hash(root / PLAN) == registration["plan_sha256"], "plan_changed")
    from source_freeze import verify_seal
    verify_seal(registration, root)
    build = registration["baseline_build"]
    committed(root, build["file"], build["review_commit"])
    require(file_hash(root / build["file"]) == build["sha256"], "build_changed")
    build_record = load(root / build["file"])
    baseline_build(build_record, registration, root)
    coreav_build_records(registration, root)
    return registration


def coreav_build_records(registration, root):
    import coreav_build
    import tomllib

    root = Path(root)
    pins = coreav_build.pin(read(root, coreav_build.PIN, 32768))
    for host_class, reference in registration["coreav_builds"].items():
        committed(root, reference["file"], reference["review_commit"])
        require(file_hash(root / reference["file"], root=root) == reference["sha256"], "coreav_build_changed")
        value = coreav_build.validate(load(root / reference["file"], coreav_build.LIMIT),
                                      registration["candidate"], host_class, pins)
        candidate = value["source"]["commit"]
        require(git(root, "rev-parse", candidate + "^{tree}").decode().strip() == value["source"]["tree"],
                "coreav_build_source")
        config = value["configuration"]
        expected_inputs = coreav_build.BASE_INPUTS | {
            package["manifest"] for package in config["packages"] if package["source"] is None}
        require(set(config["inputs"]) == expected_inputs, "coreav_build_inputs")
        for name, expected in config["inputs"].items():
            require(sha(git(root, "show", f"{candidate}:{name}")) == expected, "coreav_build_input_changed")
        manifest = tomllib.loads(git(root, "show", f"{candidate}:{coreav_build.MANIFEST}").decode())
        require(coreav_build.canonical(manifest.get("profile", {})) ==
                coreav_build.canonical({"dist": coreav_build.PROFILE}), "coreav_build_profile")
        locked = tomllib.loads(git(root, "show", f"{candidate}:{coreav_build.LOCK}").decode())["package"]
        for package in config["packages"]:
            matching = [entry for entry in locked if entry["name"] == package["name"] and
                        entry["version"] == package["version"] and entry.get("source") == package["source"]]
            require(len(matching) == 1 and matching[0].get("checksum") == package["checksum"],
                    "coreav_build_lock")
        require(config["inputs"][coreav_build.PIN] == file_hash(root / coreav_build.PIN), "coreav_build_pin")
        executable = registration["executables"]["coreav"] if host_class == "macos" else None
        expected_binary = executable["sha256"] if executable else registration["metric_tools"]["coreav_decoder"]
        require(value["binary"]["sha256"] == expected_binary, "coreav_build_binary")
        if executable:
            require(value["configuration_sha256"] == executable["configuration_sha256"],
                    "coreav_build_configuration")


def preflight(registration_path, root=ROOT):
    event = os.environ.get("GITHUB_EVENT_NAME", "")
    require(event not in ("pull_request", "pull_request_target", "merge_group") and
            not os.environ.get("GITHUB_ACTIONS") and sys.platform == "darwin", "judged_event")
    root = Path(root).resolve(strict=True)
    registration = source_registration(registration_path, root)
    observed = host_observation()
    require(all(type(observed[k]) is type(registration["host"][k]) and
                observed[k] == registration["host"][k] for k in observed), "host_mismatch")
    require(os.environ.get("COREAV_MAC_ALLOCATION_SHA256") ==
            registration["host"]["allocation_sha256"], "allocation")
    for record in registration["executables"].values():
        require(file_hash(root / record["file"], root=root) == record["sha256"], "binary_changed")
    build_record = load(root / registration["baseline_build"]["file"])
    with tempfile.TemporaryDirectory() as temporary:
        binary = root / registration["executables"]["ffmpeg"]["file"]
        require((binary.parent / "ffprobe").resolve(strict=True) ==
                (root / build_record["ffprobe_file"]).resolve(strict=True), "baseline_probe_changed")
        run([sys.executable, str(root / "scripts/oracle_pin_macos.py"), "--root", str(root),
             "--verify-bin", str(binary.parent)], temporary, 60, stdout="pin", stderr="pin-error")
        require(not read(temporary, "pin-error", 65536), "baseline_pin")
        run([str(binary), "-version"], temporary, 10, stdout="version", stderr="version-error")
        version = read(temporary, "version", 65536).decode().splitlines()
        require(version and version[0].split()[:3] == ["ffmpeg", "version", build_record["macos_build"]],
                "baseline_version")
        configuration = [line.removeprefix("configuration: ") for line in version
                         if line.startswith("configuration: ")]
        require(len(configuration) == 1 and shlex.split(configuration[0]) ==
                build_record["configuration"], "baseline_configuration")
        run([str(binary), "-hide_banner", "-h", "encoder=h264_videotoolbox"], temporary, 10,
            stdout="help", stderr="help-error")
        require(sha(read(temporary, "help", 1024**2)) == build_record["encoder_help_sha256"],
                "baseline_controls")
    return registration


def baseline_build(value, registration, root):
    require(type(value) is dict and "observability" in value, "ffmpeg_observability_missing")
    keys(value, "schema version macos_build macos_pin_sha256 binary_sha256 ffprobe_file ffprobe_sha256 "
         "configuration configuration_sha256 encoder_help_sha256 release_review source_removed examples_removed "
         "source_acquired binary_redistributed acquisition architectures signatures observability")
    require(value["schema"] == "coreav.videotoolbox-ffmpeg-build/2", "build_schema")
    pins = []
    for name in ("scripts/oracle-ffmpeg.env", "scripts/oracle-ffmpeg-macos.env"):
        pin = {}
        for line in read(root, name, 32768).decode().splitlines():
            if line and not line.startswith("#"):
                match = re.fullmatch(r"([A-Z][A-Z0-9_]*)=(\S+)", line)
                require(match is not None and match[1] not in pin, "baseline_pin")
                pin[match[1]] = match[2]
        pins.append(pin)
    linux, pin = pins
    require(value["version"] == linux["VERSION"] == pin["MACOS_BUILD"].split("-", 1)[0] and
            value["macos_build"] == pin["MACOS_BUILD"] and value["macos_pin_sha256"] ==
            file_hash(root / "scripts/oracle-ffmpeg-macos.env"), "baseline_pin")
    require(value["binary_sha256"] == registration["executables"]["ffmpeg"]["sha256"] ==
            pin["MACOS_FFMPEG_BINARY_SHA256"] and value["ffprobe_sha256"] == pin["MACOS_FFPROBE_BINARY_SHA256"],
            "baseline_binary")
    relative(value["ffprobe_file"])
    require(Path(value["ffprobe_file"]).name == "ffprobe" and Path(value["ffprobe_file"]).parent ==
            Path(registration["executables"]["ffmpeg"]["file"]).parent, "baseline_binary")
    keys(value["acquisition"], "kind ffmpeg ffprobe")
    require(value["acquisition"]["kind"] == "pinned-prebuilt-zip", "baseline_acquisition")
    keys(value["architectures"], "ffmpeg ffprobe")
    keys(value["signatures"], "ffmpeg ffprobe")
    for name in ("ffmpeg", "ffprobe"):
        entry = value["acquisition"][name]
        keys(entry, "url archive_sha256 binary_sha256")
        require(entry == {"url": pin[f"MACOS_{name.upper()}_URL"],
                          "archive_sha256": pin[f"MACOS_{name.upper()}_SHA256"],
                          "binary_sha256": pin[f"MACOS_{name.upper()}_BINARY_SHA256"]}, "baseline_acquisition")
        require(value["architectures"][name] == "arm64", "baseline_architecture")
        signature = value["signatures"][name]
        keys(signature, "verified strict anchor team")
        require(signature["verified"] is True and signature["strict"] is True and
                signature["anchor"] == "apple generic" and signature["team"] == pin["MACOS_SIGNING_TEAM"],
                "baseline_signature")
    require(type(value["configuration"]) is list and
            value["configuration"] and all(type(arg) is str for arg in value["configuration"]), "baseline_configuration")
    require(value["configuration_sha256"] == sha("\0".join(value["configuration"]).encode()) ==
            registration["executables"]["ffmpeg"]["configuration_sha256"], "baseline_configuration")
    digest(value["encoder_help_sha256"])
    digest(value["ffprobe_sha256"])
    keys(value["release_review"], "role binary_sha256")
    require(value["release_review"]["role"] == "release-owner" and
            value["release_review"]["binary_sha256"] == value["binary_sha256"],
            "baseline_review")
    require(value["source_removed"] is True and value["examples_removed"] is True and
            value["source_acquired"] is False and value["binary_redistributed"] is False, "baseline_source")
    from observations import specification
    specification(value["observability"])


def raw_size(item):
    size = item["width"] * item["height"] * 3 // 2 * item["frames"]
    require(0 < size <= 32 * 1024**3, "raw_bounds")
    return size


def input_hashes(root, registration, fixtures=False):
    roster_coverage(registration)
    for item in (list(registration["fixtures"].values()) if fixtures else registration["inputs"]):
        covered_class(item)
        path = Path(root) / item["raw_file"]
        size = item["width"] * item["height"] * 3 // 2
        require(path.stat().st_size == size * item["frames"] and
                file_hash(path, limit=raw_size(item), root=Path(root)) == item["raw_sha256"], "input_hash")
        with path.open("rb") as stream:
            for expected in item["source_frame_sha256"]:
                require(sha(stream.read(size)) == expected, "source_frame_hash")
            require(not stream.read(1), "source_frame_count")
