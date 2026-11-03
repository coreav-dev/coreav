"""Run the registered generated FLAC comparison on merged main in Actions."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import random
import statistics
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(ROOT / "tools" / "compat-corpus"))

from flac_output_check import make_source, probe, run
from oracle_audio import INPUT_ARGS as AUDIO_INPUT
from oracle_pin import verify_bin
from speed import timed


def require_main():
    if (os.environ.get("GITHUB_ACTIONS") != "true"
            or os.environ.get("GITHUB_EVENT_NAME") != "workflow_dispatch"
            or os.environ.get("GITHUB_REF") != "refs/heads/main"
            or os.environ.get("GITHUB_REPOSITORY") != "coreav-dev/coreav"):
        raise RuntimeError("judged FLAC inputs require a main workflow_dispatch in Actions")
    head = run(["git", "-C", ROOT, "rev-parse", "HEAD"]).decode().strip()
    if head != os.environ.get("GITHUB_SHA"):
        raise RuntimeError("the checkout does not match the main event")
    return head


def roster():
    return [{"class": kind, "bits": bits, "channels": channels, "rate": rate,
             "case": f"{kind}-{bits}-{channels}-{rate}", "pairs": [], "complete": False}
            for kind in ("tones", "polynomial", "noise") for bits in (16, 24)
            for channels in (1, 2) for rate in (44100, 48000)]


def samples(case, seconds=30):
    bound = (1 << (case["bits"] - 1)) - 1
    result = bytearray()
    state = 7
    for frame in range(case["rate"] * seconds):
        for channel in range(case["channels"]):
            if case["class"] == "tones":
                time = frame / case["rate"]
                value = round(bound * (0.43 * math.sin(2 * math.pi * (233 + channel * 17) * time)
                                      + 0.21 * math.sin(2 * math.pi * 997 * time)
                                      + 0.12 * math.sin(2 * math.pi * 3203 * time)))
            elif case["class"] == "polynomial":
                position = (frame + channel * 37) % 4093
                value = ((position * position * 13 + position * 71) % (2 * bound + 1)) - bound
            else:
                state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
                value = (state >> (32 - case["bits"])) - (1 << (case["bits"] - 1))
            result.extend(value.to_bytes(case["bits"] // 8, "little", signed=True))
    return bytes(result)


def commands(coreav, ffmpeg, source, output):
    return {
        "native": [str(coreav), "--strict-safe", "ffmpeg", "-i", str(source), "-y",
                   "-map_metadata", "-1", "-threads", "1", str(output["native"])],
        "reference": [str(ffmpeg), "-v", "error", *AUDIO_INPUT, "-i", str(source), "-y",
                      "-map_metadata", "-1", "-threads", "1", str(output["reference"])],
    }


def validate(ffmpeg, ffprobe, output, expected, case):
    fields = []
    for side in ("native", "reference"):
        path = output[side]
        actual = run([ffmpeg, "-v", "error", *AUDIO_INPUT, "-i", path,
                      "-c:a", f"pcm_s{case['bits']}le", "-f", f"s{case['bits']}le", "-"])
        if actual != expected:
            raise RuntimeError("complete decoded samples differ")
        fields.append(probe(ffprobe, path))
    if fields[0] != fields[1]:
        raise RuntimeError("output probe fields differ")
    if output["native"].read_bytes()[26:42] != hashlib.md5(expected).digest():
        raise RuntimeError("native STREAMINFO checksum differs")


def summary(rows):
    if not rows or any(not row["complete"] or len(row["pairs"]) != 9 for row in rows):
        raise RuntimeError("incomplete FLAC measurements")
    if any(not pair.get("verified") or any(pair.get(f"{side}_exit") != 0
               or not isinstance(pair.get(side), (int, float)) or not math.isfinite(pair[side])
               or pair[side] <= 0 for side in ("native", "reference"))
           for row in rows for pair in row["pairs"]):
        raise RuntimeError("unverified or failed FLAC measurement pair")
    ratios = [[pair["native"] / pair["reference"] for pair in row["pairs"]] for row in rows]
    medians = [statistics.median(values) for values in ratios]
    rng = random.Random(0)
    bootstrap = []
    for _ in range(10000):
        chosen = rng.choices(ratios, k=len(ratios))
        bootstrap.append(statistics.median(statistics.median(rng.choices(values, k=len(values))) for values in chosen))
    bootstrap.sort()
    return {"cases": len(rows), "median_ratio": statistics.median(medians),
            "interval_95": [bootstrap[250], bootstrap[9750]],
            "share_faster": sum(value < 1 for value in medians) / len(medians)}


def measure(coreav, oracle, root, report):
    ffmpeg, ffprobe = oracle / "ffmpeg", oracle / "ffprobe"
    cpu = min(os.sched_getaffinity(0))
    os.sched_setaffinity(0, {cpu})
    cpuinfo = Path("/proc/cpuinfo").read_text()
    report["host"] = {"cpu_model": next((line.split(":", 1)[1].strip() for line in cpuinfo.splitlines()
                                         if line.startswith("model name")), "unknown"),
                      "logical_cpu": cpu, "runner": "ubuntu-24.04", "storage": "runner temporary directory"}
    env = {**os.environ, "COREAV_STRICT_SAFE": "1", "COREAV_FALLBACK": str(root / "missing-encoder")}
    null = []
    for _ in range(9):
        elapsed, code = timed([str(ffmpeg), "-version"], env, raise_oserror=True)
        if code != 0:
            raise RuntimeError("null calibration failed")
        null.append(elapsed)
    report["null_seconds"] = null

    for row in report["rows"]:
        expected = samples(row)
        row["input_pcm_sha256"] = hashlib.sha256(expected).hexdigest()
        source = make_source(ffmpeg, root, row["bits"], row["channels"], row["rate"], expected, row["case"])
        output = {side: root / f"{row['case']}-{side}.flac" for side in ("native", "reference")}
        argv = commands(coreav, ffmpeg, source, output)
        for side in ("native", "reference"):
            run(argv[side], env=env)
        validate(ffmpeg, ffprobe, output, expected, row)
        if "aa_pairs" not in report:
            aa = []
            for _ in range(9):
                pair = []
                for _ in range(2):
                    output["reference"].unlink(missing_ok=True)
                    elapsed, code = timed(argv["reference"], env, raise_oserror=True)
                    if code != 0:
                        raise RuntimeError("A/A calibration command failed")
                    validate(ffmpeg, ffprobe, output, expected, row)
                    pair.append(elapsed)
                aa.append(pair)
            report["aa_pairs"] = aa
            ratio = statistics.median(a / b for a, b in aa)
            if not 0.90 <= ratio <= 1.10:
                raise RuntimeError("A/A calibration is outside its registered range")
        for round_number in range(9):
            pair = {"order": ["native", "reference"] if round_number % 2 == 0 else ["reference", "native"]}
            row["pairs"].append(pair)
            for side in pair["order"]:
                output[side].unlink(missing_ok=True)
                elapsed, code = timed(argv[side], env, raise_oserror=True)
                pair[side] = elapsed
                pair[f"{side}_exit"] = code
                if code != 0:
                    raise RuntimeError("an encode invocation failed or timed out")
            validate(ffmpeg, ffprobe, output, expected, row)
            pair["verified"] = True
        row["bytes"] = {side: path.stat().st_size for side, path in output.items()}
        row["work_dominates_startup"] = statistics.median(pair["reference"] for pair in row["pairs"]) >= 5 * statistics.median(null)
        row["complete"] = True
        source.unlink()
        source.with_suffix(".raw").unlink()
        for path in output.values():
            path.unlink()

    report["statistics"] = {kind: summary([row for row in report["rows"] if row["class"] == kind])
                            for kind in ("tones", "polynomial", "noise")}
    report["statistics"]["overall"] = summary(report["rows"])
    size_ratio = sum(row["bytes"]["native"] for row in report["rows"]) / sum(row["bytes"]["reference"] for row in report["rows"])
    report["size_ratio"] = size_ratio
    report["size_bar"] = size_ratio <= 1.05 and all(row["bytes"]["native"] <= 1.10 * row["bytes"]["reference"]
                                                  for row in report["rows"] if row["class"] != "noise")
    report["speed_bar_observed"] = (all(row["work_dominates_startup"] for row in report["rows"])
                                     and all(group["median_ratio"] <= 1.00 for group in report["statistics"].values()))
    report["speed_verdict"] = "diagnostic-only-shared-host"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coreav", type=Path, required=True)
    parser.add_argument("--oracle-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    report = {"state": "incomplete", "rows": [], "judged": not args.smoke}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    try:
        if args.smoke:
            if os.environ.get("GITHUB_ACTIONS") != "true":
                raise RuntimeError("FLAC tooling smoke runs in Actions")
        else:
            report["head"] = require_main()
        problems = list(verify_bin(args.oracle_dir))
        if problems:
            raise RuntimeError("oracle verification failed")
        with tempfile.TemporaryDirectory(prefix="flac-measure-") as directory:
            root = Path(directory)
            coreav, oracle = args.coreav.resolve(), args.oracle_dir.resolve()
            if args.smoke:
                for kind in ("tones", "polynomial", "noise"):
                    row = {"class": kind, "bits": 24, "channels": 2, "rate": 32, "case": kind}
                    expected = samples(row, seconds=1)
                    source = make_source(oracle / "ffmpeg", root, 24, 2, 48000, expected, kind)
                    output = {side: root / f"{kind}-{side}.flac" for side in ("native", "reference")}
                    for argv in commands(coreav, oracle / "ffmpeg", source, output).values():
                        run(argv)
                    validate(oracle / "ffmpeg", oracle / "ffprobe", output, expected, row)
            else:
                report["rows"] = roster()
                report["plan_sha256"] = hashlib.sha256((ROOT / "docs/design/flac-output.md").read_bytes()).hexdigest()
                report["driver_sha256"] = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
                for label, path in (("coreav", coreav), ("reference", oracle / "ffmpeg")):
                    with path.open("rb") as binary:
                        report[f"{label}_sha256"] = hashlib.file_digest(binary, "sha256").hexdigest()
                report["reference_version"] = run([oracle / "ffmpeg", "-version"]).decode().splitlines()[0]
                measure(coreav, oracle, root, report)
        report["state"] = "complete"
    except BaseException as error:
        report["state"] = "aborted"
        report["error"] = type(error).__name__
        raise
    finally:
        args.out.write_text(json.dumps(report, indent=2) + "\n")
    return 0 if args.smoke else int(not report["size_bar"] or not report["speed_bar_observed"])


if __name__ == "__main__":
    raise SystemExit(main())
