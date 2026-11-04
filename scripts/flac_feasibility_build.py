"""Build and verify disposable exact-result feasibility variants without timing."""

import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile


def main():
    source, output = (Path(value).resolve() for value in sys.argv[1:])
    output.mkdir(parents=True, exist_ok=False)
    control = Path(__file__).resolve().parent
    encoder = source / "crates/coreav-audio/src/flac/encode.rs"
    original = encoder.read_bytes()
    merged = "62f4af2a4a3adb477583f91d396f6daeaebc2055"

    def text(argv):
        return subprocess.check_output(argv, cwd=source, text=True, timeout=30).strip()

    def checksum(path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    if text(["git", "rev-parse", "HEAD"]) != merged or text(["git", "status", "--porcelain"]):
        raise RuntimeError("feasibility builds require the clean measured merged source")
    if checksum(encoder) != "1ccb0c4a52f24bc9e107a7f295476e8368f2961b2b0547720b2d2485fc87b983":
        raise RuntimeError("wrong merged encoder")
    if os.environ.get("RUSTFLAGS") != "" or os.environ.get("CARGO_INCREMENTAL") != "0":
        raise RuntimeError("unexpected release flags")
    if any(name.startswith("CARGO_PROFILE_") or name in (
            "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_BUILD_TARGET") for name in os.environ):
        raise RuntimeError("unregistered Cargo override")
    for name in os.environ:
        if name.startswith(("COREAV_RICE_", "COREAV_SUBFRAME_", "COREAV_FRAME_")):
            raise RuntimeError("diagnostic tape mode must be off during build tests")
    archive = subprocess.check_output(["git", "archive", "HEAD"], cwd=source, timeout=60)
    (output / "source.tar.gz").write_bytes(gzip.compress(archive, mtime=0))
    variants = {}
    try:
        for component in ("plain", "word", "predictor", "stereo", "combined"):
            encoder.write_bytes(original)
            directory = output / component
            directory.mkdir()
            if component != "plain":
                builder = control / "flac_feasibility_variant.py"
                subprocess.run([sys.executable, "-I", str(builder), component, str(encoder)],
                               check=True, timeout=30)
                if text(["git", "diff", "--name-only"]) != str(encoder.relative_to(source)):
                    raise RuntimeError("a disposable variant changed other source")
                (directory / "instrumentation.patch").write_bytes(subprocess.check_output(
                    ["git", "diff", "--binary"], cwd=source, timeout=30))
            (directory / "encode.rs").write_bytes(encoder.read_bytes())
            with (directory / "tests.log").open("wb") as log:
                subprocess.run(["cargo", "test", "--locked", "--release", "--no-fail-fast", "-p", "coreav-audio",
                                "--lib", "--test", "flac_encode"], cwd=source, check=True, timeout=1200,
                               stdout=log, stderr=subprocess.STDOUT)
            counts = [int(value) for value in re.findall(
                r"test result: ok\. (\d+) passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;",
                (directory / "tests.log").read_text())]
            if counts != [61 + {"plain": 0, "word": 1, "predictor": 2, "stereo": 2, "combined": 3}[component], 7]:
                raise RuntimeError("each exact optimized variant must pass both complete test targets")
            for crate, extra, binary in (("coreav-cli", [], "coreav"),
                                         ("coreav-audio", ["--example", "flac_identity"], "examples/flac_identity")):
                subprocess.run(["cargo", "build", "--locked", "--release", "-p", crate, *extra],
                               cwd=source, check=True, timeout=1200)
                shutil.copy2(source / "target/release" / binary, directory / Path(binary).name)
            variants[component] = {"encoder_sha256": checksum(encoder), "tests": counts}
    finally:
        encoder.write_bytes(original)
    if text(["git", "status", "--porcelain"]):
        raise RuntimeError("builds did not restore the measured source")
    if checksum(output / "plain/coreav") != "03343621193f85f29f7e09ea92657821d269e4e26cb4f08f89cfe0a4aaa9126f":
        raise RuntimeError("plain executable differs from the measured candidate")
    helpers = runpy.run_path(str(source / "scripts/flac_output_identity.py"))
    reference = output / "plain/flac_identity"
    names = helpers["roster"](helpers["run"](reference, "--list", "plain-roster"))
    for component in ("word", "predictor", "stereo", "combined"):
        binary = output / component / "flac_identity"
        if helpers["roster"](helpers["run"](binary, "--list", component + "-roster")) != names:
            raise RuntimeError("changed fixture roster")
        with tempfile.TemporaryDirectory(prefix="flac-erasure-identity-") as directory:
            left, right = Path(directory) / "plain", Path(directory) / "component"
            helpers["run"](reference, left, "plain-generation")
            helpers["run"](binary, right, "component-generation")
            rows = helpers["compare"](left, right, names)
        (output / component / "identity.json").write_text(json.dumps({"cases": rows}, indent=2) + "\n")
        if len(rows) != 1152 or not all(row["equal"] for row in rows):
            raise RuntimeError("hook-off variant changed complete fixture bytes")
    manifest = {"merged_source": merged, "merged_tree": text(["git", "rev-parse", "HEAD^{tree}"]),
                "rustc": text(["rustc", "-vV"]), "cargo": text(["cargo", "-vV"]),
                "profile": {"mode": "release", "locked": True, "RUSTFLAGS": "", "CARGO_INCREMENTAL": "0"},
                "variants": variants, "tool_sha256": checksum(Path(__file__)),
                "variant_tool_sha256": checksum(control / "flac_feasibility_variant.py"),
                "files": {str(path.relative_to(output)): checksum(path) for path in sorted(output.rglob("*"))
                          if path.is_file()}}
    (output / "feasibility-build.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
