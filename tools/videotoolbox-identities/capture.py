"""Capture metadata only; never open media, encode, or provision a host."""

import argparse
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/encoders/macos"))
import cache_residency
import registration
from evidence import file_hash, require, sha, write

sys.path.insert(0, str(ROOT / "scripts"))
import oracle_pin_macos
import test_oracle

BOTTLE_SHA = "71ea80f43e78935336818c646b4561db14301e5590d0e726f221a944cb9efb73"
BINARY_SHA = "bbb048645b448aea89db45c7331a09598c82da9941c41ce478b05d52b4c15bb2"
OBSERVER = "target/videotoolbox/bin/vmtouch"


def command(args):
    result = subprocess.run(args, check=True, capture_output=True, timeout=30)
    require(
        len(result.stdout) <= 1024**2 and len(result.stderr) <= 1024**2,
        "metadata_bounds",
    )
    return result.stdout


def configuration(version):
    lines = version.decode("utf-8").splitlines()
    found = [
        line.removeprefix("configuration: ")
        for line in lines
        if line.startswith("configuration: ")
    ]
    require(len(found) == 1, "baseline_configuration")
    return shlex.split(found[0])


def observer_binary():
    url = "https://ghcr.io/token?service=ghcr.io&scope=repository:homebrew/core/vmtouch:pull"
    with urllib.request.urlopen(url, timeout=30) as response:
        token = json.loads(response.read(65537))["token"]
    require(
        isinstance(token, str)
        and 0 < len(token) <= 16384
        and all(32 <= ord(char) < 127 for char in token),
        "observer_token",
    )
    url = "https://ghcr.io/v2/homebrew/core/vmtouch/blobs/sha256:" + BOTTLE_SHA
    request = urllib.request.Request(url, headers={"Authorization": "Bearer " + token})
    with urllib.request.urlopen(request, timeout=30) as response:
        data = response.read(2 * 1024**2 + 1)
    require(len(data) <= 2 * 1024**2 and sha(data) == BOTTLE_SHA, "observer_archive")
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        entries = [
            entry
            for entry in archive.getmembers()
            if entry.isfile() and entry.name.endswith("/bin/vmtouch")
        ]
        require(len(entries) == 1 and entries[0].size <= 1024**2, "observer_archive")
        binary = archive.extractfile(entries[0]).read()
    require(sha(binary) == BINARY_SHA, "observer_binary")
    path = ROOT / OBSERVER
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("xb") as target:
        target.write(binary)
    path.chmod(0o755)
    require(
        command(["/usr/bin/lipo", "-archs", str(path)]).strip() == b"arm64",
        "observer_architecture",
    )
    links = command(["/usr/bin/otool", "-L", str(path)]).decode().splitlines()[1:]
    require(
        links and all(line.strip().startswith("/usr/lib/") for line in links),
        "observer_linkage",
    )
    return path


def capture(oracle, output):
    require(
        sys.platform == "darwin" and not output.exists(), "capture_platform_or_output"
    )
    oracle = oracle.resolve(strict=True)
    try:
        oracle = test_oracle.require(oracle / "ffmpeg", oracle / "ffprobe")
    except test_oracle.QualificationError:
        require(False, "baseline_pin")
    pin = oracle_pin_macos.load(ROOT)
    ffmpeg = oracle.ffmpeg
    version = command([str(ffmpeg), "-version"])
    probe_version = command([str(oracle.ffprobe), "-version"])
    help_text = command(
        [str(ffmpeg), "-hide_banner", "-h", "encoder=h264_videotoolbox"]
    )
    config = configuration(version)
    baseline = {
        "schema": "coreav.videotoolbox-ffmpeg-build/2",
        "version": pin["MACOS_BUILD"].split("-", 1)[0],
        "macos_build": pin["MACOS_BUILD"],
        "macos_pin_sha256": file_hash(ROOT / oracle_pin_macos.PIN),
        "binary_sha256": file_hash(ffmpeg),
        "ffprobe_file": "target/videotoolbox/bin/ffprobe",
        "ffprobe_sha256": file_hash(oracle.ffprobe),
        "configuration": config,
        "configuration_sha256": sha("\0".join(config).encode()),
        "encoder_help_sha256": sha(help_text),
        "release_review": {"role": "release-owner", "binary_sha256": file_hash(ffmpeg)},
        "source_removed": True,
        "examples_removed": True,
        "source_acquired": False,
        "binary_redistributed": False,
        "acquisition": {"kind": "pinned-prebuilt-zip"},
        "architectures": {},
        "signatures": {},
        "observability": {
            "schema": "coreav.videotoolbox-observability/2",
            "encoder_id": {"prefix": "Init the encoder: ", "suffix": ""},
            "refusal_fields": None,
        },
    }
    for name in ("ffmpeg", "ffprobe"):
        baseline["acquisition"][name] = {
            "url": pin[f"MACOS_{name.upper()}_URL"],
            "archive_sha256": pin[f"MACOS_{name.upper()}_SHA256"],
            "binary_sha256": file_hash(getattr(oracle, name)),
        }
        baseline["architectures"][name] = "arm64"
        baseline["signatures"][name] = {
            "verified": True,
            "strict": True,
            "anchor": "apple generic",
            "team": pin["MACOS_SIGNING_TEAM"],
        }
    reg = {
        "executables": {
            "ffmpeg": {
                "file": "target/videotoolbox/bin/ffmpeg",
                "sha256": baseline["binary_sha256"],
                "configuration_sha256": baseline["configuration_sha256"],
            }
        }
    }
    registration.baseline_build(baseline, reg, ROOT)
    observer_binary()
    observer = {
        "file": OBSERVER,
        "sha256": BINARY_SHA,
        "kind": "vmtouch-verbose/1",
        "page_size": os.sysconf("SC_PAGESIZE"),
        "arguments": ["-v", "-m", "32G"],
    }
    cache_residency.preflight({"cache_observer": observer}, ROOT)
    require(
        file_hash(ffmpeg) == baseline["binary_sha256"]
        and file_hash(oracle.ffprobe) == baseline["ffprobe_sha256"],
        "baseline_changed",
    )
    output.mkdir()
    write(output / "baseline.json", baseline)
    write(output / "cache-observer.json", observer)
    for name, data in (
        ("ffmpeg-version.txt", version),
        ("ffprobe-version.txt", probe_version),
        ("encoder-help.txt", help_text),
    ):
        (output / name).write_bytes(data)
    write(
        output / "capture.json",
        {
            "schema": "coreav.videotoolbox-tool-capture/1",
            "baseline_sha256": file_hash(output / "baseline.json"),
            "observer_sha256": file_hash(output / "cache-observer.json"),
            "bottle_sha256": BOTTLE_SHA,
            "observer_binary_sha256": BINARY_SHA,
            "cache_preflight": "generated_nonmedia_file_only",
            "encoder_or_media_run": False,
        },
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oracle-bin", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        capture(args.oracle_bin, args.output)
    except Exception as error:
        print("identity capture refused: " + type(error).__name__, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
