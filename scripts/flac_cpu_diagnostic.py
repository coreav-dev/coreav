"""Diagnostic-only FLAC timings, counters and native profiles on spent synthetic inputs."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(ROOT / "tools/compat-corpus"))
from flac_output_measure import samples, commands
from flac_output_check import make_source, probe
from oracle_audio import INPUT_ARGS
from oracle_pin import verify_bin
from speed import timed


def run(argv, **kwargs):
    return subprocess.run([str(x) for x in argv], check=True, capture_output=True,
                          timeout=300, **kwargs).stdout


def verify(ffmpeg, ffprobe, output, expected, bits):
    raw = run([ffmpeg, "-v", "error", "-err_detect", "crccheck", "-xerror", *INPUT_ARGS,
               "-i", output, "-c:a", f"pcm_s{bits}le", "-f", f"s{bits}le", "-"])
    if raw != expected or output.read_bytes()[26:42] != hashlib.md5(expected).digest():
        raise RuntimeError("sample or MD5 mismatch")
    return probe(ffprobe, output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plain", type=Path)
    parser.add_argument("--profile", type=Path)
    parser.add_argument("--oracle", type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--worker", type=Path)
    parser.add_argument("--instance-receipt", type=Path)
    args = parser.parse_args()
    if args.worker:
        command = json.loads(args.worker.read_text())
        for _ in range(12):
            run(command)
        return
    if any(value is None for value in (args.plain, args.profile, args.oracle, args.out)):
        parser.error("plain/profile builds, oracle and output directory are required")
    host = None
    if os.environ.get("GITHUB_ACTIONS") != "true":
        if args.instance_receipt is None:
            raise RuntimeError("profiling requires Actions or a staged remote-host receipt")
        host = json.loads(args.instance_receipt.read_text())
        features = host.get("advanced_machine_features", {})
        if (host.get("machine_type") != "c4-standard-8" or host.get("provisioning") != "STANDARD"
                or features.get("threadsPerCore") != 1
                or features.get("performanceMonitoringUnit") not in ("STANDARD", "ENHANCED")):
            raise RuntimeError("diagnostic host contract mismatch")
        request = urllib.request.Request(
            "http://metadata.google.internal/computeMetadata/v1/instance/machine-type",
            headers={"Metadata-Flavor": "Google"})
        with urllib.request.urlopen(request, timeout=5) as response:
            machine = response.read().decode().rsplit("/", 1)[-1]
        if machine != host["machine_type"]:
            raise RuntimeError("metadata host mismatch")
        for path in Path("/sys/devices/system/cpu").glob("cpu[0-9]*/topology/thread_siblings_list"):
            if not path.read_text().strip().isdigit():
                raise RuntimeError("SMT remains enabled")
    root = args.out.resolve()
    root.mkdir(parents=True, exist_ok=False)
    report = {"state": "incomplete", "judged": False, "rows": [],
              "source": run(["git", "rev-parse", "HEAD"]).decode().strip(), "host": host}
    try:
        if list(verify_bin(args.oracle)):
            raise RuntimeError("oracle pin mismatch")
        ffmpeg, ffprobe = args.oracle.resolve() / "ffmpeg", args.oracle.resolve() / "ffprobe"
        help_text = run([ffmpeg, "-hide_banner", "-h", "encoder=flac"]).decode()
        (root / "reference-help.txt").write_text(help_text)
        aligned = ["-compression_level", "5", "-frame_size", "4096", "-lpc_type", "levinson",
                   "-lpc_coeff_precision", "15", "-min_prediction_order", "0",
                   "-max_prediction_order", "8", "-prediction_order_method", "search",
                   "-min_partition_order", "0", "-max_partition_order", "4"]
        for option in aligned[4::2]:
            if option not in help_text:
                raise RuntimeError("reference alignment option missing")
        report["aligned_reference_options"] = aligned
        report["reference_version"] = run([ffmpeg, "-version"]).decode().splitlines()[0]
        report["cpuinfo"] = Path("/proc/cpuinfo").read_text()
        report["load_before"] = Path("/proc/loadavg").read_text().strip()
        cpu = min(os.sched_getaffinity(0))
        os.sched_setaffinity(0, {cpu})
        report["logical_cpu"] = cpu
        perf = next(iter(sorted(Path("/usr/lib/linux-tools").glob("*/perf"))), None) or shutil.which("perf")
        if not perf:
            raise RuntimeError("perf executable missing")
        report["perf_version"] = run([perf, "--version"]).decode().strip()
        for name, binary in (("plain", args.plain), ("profile", args.profile), ("reference", ffmpeg)):
            report[name + "_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        report["profile_flags"] = "release, debug=2, force-frame-pointers=yes"
        spent = json.loads((ROOT / "docs/measurements/flac-output-round2.json").read_text())
        hashes = {row["case"]: row["input_pcm_sha256"] for row in spent["rows"]}
        env = {**os.environ, "COREAV_STRICT_SAFE": "1", "COREAV_FALLBACK": str(root / "missing")}
        cells = [("tones", 16, 1), ("tones", 24, 2), ("polynomial", 24, 2), ("noise", 16, 2)]
        for kind, bits, channels in cells:
            case = {"class": kind, "bits": bits, "channels": channels, "rate": 48000,
                    "case": f"{kind}-{bits}-{channels}-48000", "calls": []}
            report["rows"].append(case)
            expected = samples(case)
            case["input_pcm_sha256"] = hashlib.sha256(expected).hexdigest()
            if case["input_pcm_sha256"] != hashes[case["case"]]:
                raise RuntimeError("diagnostic input differs from the spent case")
            source = make_source(ffmpeg, root, bits, channels, 48000, expected, case["case"])
            outputs = {side: root / f"{case['case']}-{side}.flac" for side in ("native", "reference", "aligned", "profile")}
            argv = commands(args.plain.resolve(), ffmpeg, source, outputs)
            argv["aligned"] = [*argv["reference"][:-1], *aligned, str(outputs["aligned"])]
            argv["profile"] = [str(args.profile.resolve()), *argv["native"][1:-1], str(outputs["profile"])]
            for side in argv:
                run(argv[side], env=env)
            fields = [verify(ffmpeg, ffprobe, outputs[side], expected, bits) for side in argv]
            if any(value != fields[0] for value in fields):
                raise RuntimeError("probe mismatch")
            if outputs["native"].read_bytes() != outputs["profile"].read_bytes():
                raise RuntimeError("profile build changed encoded bytes")
            baseline_hash = hashlib.sha256(outputs["native"].read_bytes()).hexdigest()
            case["output_bytes"] = {side: p.stat().st_size for side, p in outputs.items()}
            for index in range(5):
                order = ["native", "reference", "aligned", "profile"]
                if index % 2:
                    order.reverse()
                for side in order:
                    elapsed, code = timed(argv[side], env, raise_oserror=True)
                    if code != 0:
                        raise RuntimeError("timed diagnostic command failed")
                    verify(ffmpeg, ffprobe, outputs[side], expected, bits)
                    counter = root / f"{case['case']}-{side}-{index}.stat"
                    run(["sudo", perf, "stat", "-x", ";", "-o", counter,
                         "-e", "instructions:u,cycles:u,branches:u,branch-misses:u,task-clock,context-switches,page-faults",
                         "--", *argv[side]], env=env)
                    counters = counter.read_text()
                    if "<not supported>" in counters or "<not counted>" in counters:
                        raise RuntimeError("hardware counters unavailable")
                    verify(ffmpeg, ffprobe, outputs[side], expected, bits)
                    case["calls"].append({"side": side, "round": index, "seconds": elapsed,
                                          "exit": code, "verified": True, "counters": counters})
            worker = root / f"{case['case']}-command.json"
            worker.write_text(json.dumps(argv["profile"]))
            data = root / f"{case['case']}.data"
            run(["sudo", perf, "record", "-q", "-F", "999", "-e", "cycles:u",
                 "--call-graph", "fp", "-o", data, "--", sys.executable, __file__, "--worker", worker], env=env)
            run(["sudo", "chmod", "a+r", data])
            text = run(["sudo", perf, "report", "--stdio", "--no-children", "--inline", "--comms", "coreav",
                        "--percent-limit", "0", "--sort", "symbol,srcline", "-i", data])
            (root / f"{case['case']}-profile.txt").write_bytes(text)
            if hashlib.sha256(outputs["profile"].read_bytes()).hexdigest() != baseline_hash:
                raise RuntimeError("profile repetition changed bytes")
            verify(ffmpeg, ffprobe, outputs["profile"], expected, bits)
            case["profile_repetitions"] = 12
            case["verified"] = True
            source.unlink()
            source.with_suffix(".raw").unlink(missing_ok=True)
            for path in outputs.values():
                path.unlink()
        report["state"] = "complete"
        report["load_after"] = Path("/proc/loadavg").read_text().strip()
    except BaseException as error:
        report["state"] = "aborted"
        report["failure_type"] = type(error).__name__
        if isinstance(error, subprocess.CalledProcessError):
            (root / "failure-stderr.txt").write_bytes((error.stderr or b"")[-16384:])
        raise
    finally:
        (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
