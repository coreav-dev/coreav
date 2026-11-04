"""Bind the tested executable arms to the merged cost-cache source."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import tempfile


def command(argv, root):
    return subprocess.check_output(argv, cwd=root, text=True, timeout=30).strip()


def build_variants(source, bundle):
    encoder = source / "crates/coreav-audio/src/flac/encode.rs"
    original = encoder.read_bytes()
    baseline = "cbe3f6db46668fbd910c8a3a21d92802fe759e3a"
    subprocess.run(["git", "fetch", "--no-tags", "--depth=1", "origin", baseline],
                   cwd=source, check=True, timeout=120)
    try:
        for arm, expected in (
            ("candidate", "1ccb0c4a52f24bc9e107a7f295476e8368f2961b2b0547720b2d2485fc87b983"),
            ("before", "a12f68790e31ce8bbb4567ee86f750b7ae99f04cdadf7da286597519ddfac665"),
        ):
            if arm == "before":
                encoder.write_bytes(subprocess.check_output(
                    ["git", "show", f"{baseline}:crates/coreav-audio/src/flac/encode.rs"],
                    cwd=source, timeout=30))
                if command(["git", "diff", "--name-only"], source) != str(encoder.relative_to(source)):
                    raise RuntimeError("the before arm changes more than the bound encoder")
                (bundle / "before-source.patch").write_bytes(subprocess.check_output(
                    ["git", "diff", "--binary"], cwd=source, timeout=30))
            if hashlib.sha256(encoder.read_bytes()).hexdigest() != expected:
                raise RuntimeError("encoder source does not match its bound hash")
            with (bundle / f"{arm}-tests.log").open("wb") as log:
                subprocess.run(["cargo", "test", "--locked", "--no-fail-fast", "-p", "coreav-audio",
                                "--lib", "--test", "flac_encode"], cwd=source, check=True,
                               stdout=log, stderr=subprocess.STDOUT, timeout=900)
            subprocess.run(["cargo", "build", "--locked", "--release", "-p", "coreav-cli"],
                           cwd=source, check=True, timeout=1200)
            subprocess.run(["cargo", "build", "--locked", "--release", "-p", "coreav-audio",
                            "--example", "flac_identity"], cwd=source, check=True, timeout=1200)
            destination = bundle / arm
            destination.mkdir()
            shutil.copy2(source / "target/release/coreav", destination / "coreav")
            shutil.copy2(source / "target/release/examples/flac_identity", destination / "flac_identity")
    finally:
        encoder.write_bytes(original)
    if command(["git", "status", "--porcelain"], source):
        raise RuntimeError("builds left changes outside the encoder")
    archive = subprocess.check_output(["git", "archive", "HEAD"], cwd=source, timeout=60)
    (bundle / "source.tar.gz").write_bytes(gzip.compress(archive, mtime=0))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    args = parser.parse_args()
    source, bundle = args.source.resolve(), args.bundle.resolve()
    merged = "62f4af2a4a3adb477583f91d396f6daeaebc2055"
    if command(["git", "rev-parse", "HEAD"], source) != merged:
        raise RuntimeError("source does not match the bound merged revision")
    if command(["git", "status", "--porcelain"], source):
        raise RuntimeError("merged source is not clean after restoring the candidate")
    if os.environ.get("RUSTFLAGS") != "" or os.environ.get("CARGO_INCREMENTAL") != "0":
        raise RuntimeError("build flags do not match the fixed workflow")
    if any(name.startswith("CARGO_PROFILE_") or name in (
            "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_BUILD_TARGET")
            for name in os.environ):
        raise RuntimeError("unexpected Cargo build override")
    build_variants(source, bundle)
    tests = {}
    for arm, expected in (("candidate", [61, 7]), ("before", [58, 7])):
        tests[arm] = [int(value) for value in re.findall(
            r"test result: ok\. (\d+) passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;",
            (bundle / f"{arm}-tests.log").read_text())]
        if tests[arm] != expected:
            raise RuntimeError("both complete test targets must pass before freezing")
    helpers = runpy.run_path(str(source / "scripts/flac_output_identity.py"))
    reference, candidate = bundle / "before/flac_identity", bundle / "candidate/flac_identity"
    names = helpers["roster"](helpers["run"](reference, "--list", "reference-roster"))
    if names != helpers["roster"](helpers["run"](candidate, "--list", "candidate-roster")):
        raise RuntimeError("changed fixture inventory")
    with tempfile.TemporaryDirectory(prefix="flac-build-identity-") as directory:
        left, right = Path(directory) / "before", Path(directory) / "candidate"
        helpers["run"](reference, left, "before-generation")
        helpers["run"](candidate, right, "candidate-generation")
        rows = helpers["compare"](left, right, names)
    identity = {"complete": True, "equal": all(row["equal"] for row in rows),
                "reference_source": merged, "reference_variant": "before-encoder-restored",
                "candidate_commit": merged, "cases": rows, "failure": None}
    (bundle / "build-identity.json").write_text(json.dumps(identity, indent=2) + "\n")
    if (identity.get("complete") is not True or identity.get("equal") is not True
            or identity.get("candidate_commit") != merged or identity.get("failure") is not None
            or len(identity.get("cases", [])) != 1152
            or any(row.get("equal") is not True for row in identity["cases"])):
        raise RuntimeError("complete before/candidate file equality is required")
    files = {str(path.relative_to(bundle)): hashlib.sha256(path.read_bytes()).hexdigest()
             for path in sorted(bundle.rglob("*")) if path.is_file()}
    required = {"candidate/coreav", "before/coreav", "candidate/flac_identity",
                "before/flac_identity", "oracle/ffmpeg", "oracle/ffprobe",
                "candidate-tests.log", "before-tests.log", "before-source.patch", "source.tar.gz"}
    if not required.issubset(files):
        raise RuntimeError("the tested build bundle is incomplete")
    manifest = {
        "merged_source": merged,
        "merged_tree": command(["git", "rev-parse", "HEAD^{tree}"], source),
        "before_encoder_source": "cbe3f6db46668fbd910c8a3a21d92802fe759e3a",
        "before_encoder_blob": "8626b99fb83072129160cf99f35ff2805398c3a0",
        "encoder_sha256": {
            "candidate": "1ccb0c4a52f24bc9e107a7f295476e8368f2961b2b0547720b2d2485fc87b983",
            "before": "a12f68790e31ce8bbb4567ee86f750b7ae99f04cdadf7da286597519ddfac665",
        },
        "rustc": command(["rustc", "-vV"], source),
        "cargo": command(["cargo", "-vV"], source),
        "profile": {"mode": "release", "locked": True, "RUSTFLAGS": "", "CARGO_INCREMENTAL": "0"},
        "tests": tests,
        "cargo_manifest_sha256": hashlib.sha256((source / "Cargo.toml").read_bytes()).hexdigest(),
        "cargo_lock_sha256": hashlib.sha256((source / "Cargo.lock").read_bytes()).hexdigest(),
        "tool_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "files": files,
    }
    (bundle / "build.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print("Both tested arms and all 1152 equal files are bound to the merged source")


if __name__ == "__main__":
    main()
