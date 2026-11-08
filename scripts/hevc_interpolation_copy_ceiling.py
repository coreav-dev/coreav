import argparse
import hashlib
import json
import os
import pathlib
import platform
import re
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_reference as reference
import test_oracle


def instructions(text):
    if re.search(r"^fn=.*(?:coreav_hevc|coreav_kernels_hevc)", text, re.M) is None:
        raise ValueError("missing_decoder_records")
    events = re.findall(r"^events: (.+)$", text, re.M)
    summaries = re.findall(r"^summary: (.+)$", text, re.M)
    if len(events) != 1 or len(summaries) != 1:
        raise ValueError("count_summary")
    names, values = events[0].split(), summaries[0].split()
    if len(names) != len(values) or "Ir" not in names:
        raise ValueError("count_events")
    value = int(values[names.index("Ir")])
    if value <= 0:
        raise ValueError("empty_count")
    return value


def check_work(raw, expected):
    reference.check_work("coreav", raw, expected)
    receipt = json.loads(raw)
    if receipt["thread_limit"] != 1 or receipt["max_workers"] > 1:
        raise ValueError("not_one_thread")
    return receipt


def main():
    if sys.argv[1:] == ["self-test"]:
        assert instructions("events: Ir\nsummary: 123\nfn=(1) coreav_hevc::decode\n") == 123
        for invalid in ("", "events: Dr\nsummary: 1\n", "events: Ir\nsummary: 0\n", "events: Ir\nsummary: 1\n"):
            try:
                instructions(invalid)
            except (ValueError, KeyError, IndexError):
                pass
            else:
                raise AssertionError(invalid)
        print("instruction parser self-test passed")
        return
    parser = argparse.ArgumentParser()
    for name in ("baseline", "ablated", "oracle-bin", "out"):
        parser.add_argument("--" + name, type=pathlib.Path, required=True)
    for name in ("baseline-sha", "ablated-sha"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    if any(re.fullmatch("[0-9a-f]{40}", value) is None for value in (args.baseline_sha, args.ablated_sha)):
        parser.error("full_commit_sha_required")
    cpu = min(os.sched_getaffinity(0))
    os.sched_setaffinity(0, {cpu})
    oracle = test_oracle.require(args.oracle_bin / "ffmpeg", args.oracle_bin / "ffprobe")
    binaries = dict(baseline=args.baseline, ablated=args.ablated)
    args.out.mkdir(parents=True, exist_ok=True)
    report = dict(
        baseline=args.baseline_sha, ablated=args.ablated_sha,
        oracle=reference.oracle_pin.load()["BUILD"], complete=False, cpu=cpu,
        cpu_model=next(line.split(":", 1)[1].strip() for line in pathlib.Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")),
        kernel=platform.release(), load=os.getloadavg(),
        binaries={name: hashlib.sha256(path.read_bytes()).hexdigest() for name, path in binaries.items()},
        scope="whole-process Ir for caller-owned initialized interpolation scratch instead of per-block initialization; row copies, filters, support validation and arithmetic retained; not timing or a wall-time speedup",
        cells=[])
    pool = reference.round_files.pool()
    pool.limits.max_requests = 4000
    pool.limits.max_bytes = 32 << 30

    def save():
        report["r2_requests"] = pool.limits.requests
        (args.out / "results.json").write_text(json.dumps(report, indent=2) + "\n")

    try:
        with tempfile.TemporaryDirectory(prefix="interpolation-copy-") as temp:
            selection = argparse.Namespace(only={"c00", "c01", "c02", "c03"}, frames=8)
            for identity, source, source_sha in reference.inputs(selection, oracle, pathlib.Path(temp), pool):
                checks = {name: reference.verify(binary, oracle, source) for name, binary in binaries.items()}
                if checks["baseline"] != checks["ablated"]:
                    raise ValueError("arm_pixels_or_work")
                if checks["baseline"]["depth"] != (8 if identity in {"c00", "c01"} else 10):
                    raise ValueError("unexpected_depth")
                cell = dict(id=identity, source_sha256=source_sha,
                            input_sha256=reference.conformance.digest(source),
                            **checks["baseline"], arms={})
                report["cells"].append(cell)
                for name, binary in binaries.items():
                    output = args.out / (identity + "-" + name + ".callgrind")
                    raw, _ = reference.command([
                        "taskset", "-c", str(cpu), "valgrind", "--tool=callgrind",
                        "--error-exitcode=99", "--collect-jumps=no", "--dump-instr=no",
                        "--cache-sim=no", "--callgrind-out-file=" + str(output),
                        binary, source, "decode", "1"], timeout=1200)
                    cell["arms"][name] = dict(instructions=instructions(output.read_text()),
                                             work=check_work(raw, cell["frames"]))
                    annotation, _ = reference.command(["callgrind_annotate", "--auto=no", "--inclusive=no", "--threshold=100", output], timeout=120)
                    (args.out / (identity + "-" + name + ".functions.txt")).write_bytes(annotation)
                    save()
                baseline = cell["arms"]["baseline"]["instructions"]
                ablated = cell["arms"]["ablated"]["instructions"]
                cell["removed_instructions"] = baseline - ablated
                cell["erase_ceiling_percent"] = 100 * (baseline - ablated) / baseline
                save()
            if {cell["id"] for cell in report["cells"]} != selection.only:
                raise ValueError("incomplete_selection")
            report["complete"] = True
    finally:
        pool.close()
        save()


if __name__ == "__main__":
    main()
