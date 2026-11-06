"""Hosted synthetic baseline observations only; no availability or timing verdict."""

import argparse
import sys
from itertools import islice
from pathlib import Path

from evidence import Invalid, file_hash, keys, load, read, require, write
from observations import trace
from process import run
from producer import packet_ledger
from registration import ROOT, command_templates
from source_freeze import fixture_frames, fixtures
from timing import commands

sys.path.insert(0, str(ROOT / "scripts"))
import test_oracle


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oracle-bin", required=True, type=Path)
    parser.add_argument("--exporter", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    require(sys.platform == "darwin", "baseline_smoke_platform")
    oracle, exporter, output = (path.resolve() for path in (args.oracle_bin, args.exporter, args.output))
    output.mkdir()
    try:
        oracle = test_oracle.require(oracle / "ffmpeg", oracle / "ffprobe")
    except test_oracle.QualificationError:
        raise Invalid("child_failure") from None
    before = {name: file_hash(getattr(oracle, name)) for name in ("ffmpeg", "ffprobe")}
    exporter_hash = file_hash(exporter)
    run([sys.executable, "-I", str(ROOT / "scripts/test_oracle.py"), "--resolve",
         "--ffmpeg", str(oracle.ffmpeg), "--ffprobe", str(oracle.ffprobe)],
        output, 60, stdout="oracle.stdout", stderr="oracle.stderr")
    reg = {"executables": {"ffmpeg": {"file": str(oracle.ffmpeg)}, "coreav": {"file": str(exporter)}},
           "command_controls": {"hardware": ["-profile:v", "high", "-bf", "0", "-allow_sw", "0", "-require_sw", "0"]}}
    reg["commands"] = {"hardware": command_templates(reg, "hardware")}
    spec = {"schema": "coreav.videotoolbox-observability/2",
            "encoder_id": {"prefix": "Init the encoder: ", "suffix": ""}, "refusal_fields": None}
    results = []
    for name, item in fixtures().items():
        directory = output / name
        directory.mkdir()
        source = directory / "source.yuv"
        source.write_bytes(b"".join(islice(fixture_frames(name), 4)))
        item = {**item, "frames": 4, "raw_file": str(source)}
        video, stats = directory / "video.h264", directory / "packets.stats"
        argv = commands(reg, item, "hardware", output, video, stats, oracle)["ffmpeg"]
        run(argv, directory, 60, stdout="encode.stdout", stderr="observations.txt")
        observed, _ = trace(read(directory, "observations.txt", 16 * 1024**2), spec)
        ledger = packet_ledger(read(directory, "packets.stats", 16 * 1024**2), item, video.stat().st_size)
        run([str(exporter), "identify", "--encoder-id", observed["encoder_id"],
             "--report", str(directory / "identity.json")], directory, 30,
            stdout="identity.stdout", stderr="identity.stderr")
        identity = load(directory / "identity.json")
        keys(identity, "schema encoder_id hardware")
        require(identity == {"schema": "coreav.videotoolbox-encoder-identity/1",
                             "encoder_id": observed["encoder_id"], "hardware": "hardware"},
                "baseline_smoke_identity")
        results.append({"fixture": name, "rate": item["rate"], "encoder_id": observed["encoder_id"],
                        "hardware": identity["hardware"], "packet_bytes": sum(row["bytes"] for row in ledger),
                        "video_bytes": video.stat().st_size, "video_sha256": file_hash(video)})
    require(before == {name: file_hash(getattr(oracle, name)) for name in before} and
            file_hash(exporter) == exporter_hash, "baseline_smoke_executable_changed")
    write(output / "observations.json", {"schema": "coreav.videotoolbox-baseline-smoke/1",
                                         "scope": "synthetic_observability_only", "oracle_sha256": before,
                                         "exporter_sha256": exporter_hash, "results": results})


if __name__ == "__main__":
    main()
