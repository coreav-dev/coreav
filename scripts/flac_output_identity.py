"""Compare complete FLAC files produced by the same fixture code on two builds."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

BASELINE = "e3d1b79286c0764fed27fd9060093f881c3ebd52"


class IdentityFailure(Exception):
    pass


def run(binary, argument, stage):
    try:
        return subprocess.run([str(binary), str(argument)], check=True, capture_output=True,
                              timeout=180).stdout
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        raise IdentityFailure(stage) from error


def roster(data):
    try:
        names = data.decode("ascii").splitlines()
    except UnicodeError as error:
        raise IdentityFailure("invalid-roster") from error
    if not names or len(names) != len(set(names)) or any(
            not re.fullmatch(r"s(?:16|24)-c[1-8]-n[0-9]+-p[0-7]\.flac", name) for name in names):
        raise IdentityFailure("invalid-roster")
    return names


def compare(reference, candidate, names):
    expected = set(names)
    for directory in (reference, candidate):
        paths = list(directory.iterdir())
        if {path.name for path in paths} != expected or any(
                not path.is_file() or path.is_symlink() for path in paths):
            raise IdentityFailure("incomplete-inventory")
    rows = []
    for name in names:
        left = (reference / name).read_bytes()
        right = (candidate / name).read_bytes()
        if not left.startswith(b"fLaC") or not right.startswith(b"fLaC"):
            raise IdentityFailure("invalid-file")
        rows.append({"case": name, "reference_bytes": len(left), "candidate_bytes": len(right),
                     "reference_sha256": hashlib.sha256(left).hexdigest(),
                     "candidate_sha256": hashlib.sha256(right).hexdigest(), "equal": left == right})
    return rows


def capture(reference, candidate, candidate_commit, producer, output):
    report = {"complete": False, "equal": False, "reference_commit": BASELINE,
              "candidate_commit": None, "cases": [], "failure": None}
    stage = "provenance"
    try:
        if not re.fullmatch(r"[0-9a-f]{40}", candidate_commit):
            raise IdentityFailure("invalid-source-commit")
        report["candidate_commit"] = candidate_commit
        report["producer_sha256"] = hashlib.sha256(producer.read_bytes()).hexdigest()
        report["reference_binary_sha256"] = hashlib.sha256(reference.read_bytes()).hexdigest()
        report["candidate_binary_sha256"] = hashlib.sha256(candidate.read_bytes()).hexdigest()
        stage = "roster"
        names = roster(run(reference, "--list", "reference-roster"))
        if names != roster(run(candidate, "--list", "candidate-roster")):
            raise IdentityFailure("changed-roster")
        stage = "fixture-generation"
        with tempfile.TemporaryDirectory(prefix="flac-identity-") as root:
            left, right = Path(root) / "reference", Path(root) / "candidate"
            run(reference, left, "reference-generation")
            run(candidate, right, "candidate-generation")
            stage = "comparison"
            report["cases"] = compare(left, right, names)
        report["complete"] = True
        report["equal"] = all(row["equal"] for row in report["cases"])
        if not report["equal"]:
            report["failure"] = "changed-bytes"
    except IdentityFailure as error:
        report["failure"] = str(error)
    except OSError:
        report["failure"] = stage + "-io"
    finally:
        output.write_text(json.dumps(report, indent=2) + "\n")
    return report["complete"] and report["equal"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", action="store_true")
    parser.add_argument("--reference", type=Path)
    parser.add_argument("--candidate", type=Path)
    parser.add_argument("--candidate-commit")
    parser.add_argument("--producer", type=Path)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if args.baseline:
        print(BASELINE)
        return 0
    if any(value is None for value in (args.reference, args.candidate, args.candidate_commit,
                                       args.producer, args.out)):
        parser.error("both builds, the source commit, producer and report path are required")
    return 0 if capture(args.reference.resolve(), args.candidate.resolve(), args.candidate_commit,
                        args.producer, args.out) else 1


if __name__ == "__main__":
    raise SystemExit(main())
