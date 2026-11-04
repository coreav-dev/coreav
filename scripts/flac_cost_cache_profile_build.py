"""Build a tested symbolized copy of the merged encoder for development profiling."""

import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


def main():
    source, output = map(lambda value: Path(value).resolve(), sys.argv[1:])
    output.mkdir(parents=True, exist_ok=False)
    merged = "62f4af2a4a3adb477583f91d396f6daeaebc2055"

    def text(argv):
        return subprocess.check_output(argv, cwd=source, text=True, timeout=30).strip()

    if text(["git", "rev-parse", "HEAD"]) != merged or text(["git", "status", "--porcelain"]):
        raise RuntimeError("profiling requires the exact clean merged source")
    encoder = source / "crates/coreav-audio/src/flac/encode.rs"
    checksum = hashlib.sha256(encoder.read_bytes()).hexdigest()
    if checksum != "1ccb0c4a52f24bc9e107a7f295476e8368f2961b2b0547720b2d2485fc87b983":
        raise RuntimeError("wrong merged encoder")
    if any(name.startswith("CARGO_PROFILE_") or name in (
            "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_BUILD_TARGET") for name in os.environ):
        raise RuntimeError("unrecorded build override")
    flags = "-C debuginfo=2 -C force-frame-pointers=yes"
    env = {**os.environ, "RUSTFLAGS": flags, "CARGO_INCREMENTAL": "0", "CARGO_PROFILE_RELEASE_STRIP": "false"}
    with (output / "profile-tests.log").open("wb") as log:
        subprocess.run(["cargo", "test", "--locked", "--release", "--no-fail-fast", "-p", "coreav-audio",
                        "--lib", "--test", "flac_encode"], cwd=source, env=env, check=True, timeout=1200,
                       stdout=log, stderr=subprocess.STDOUT)
    counts = [int(value) for value in re.findall(
        r"test result: ok\. (\d+) passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;",
        (output / "profile-tests.log").read_text())]
    if counts != [61, 7]:
        raise RuntimeError("both complete optimized test targets must pass")
    for crate, extra, executable in (("coreav-cli", [], "coreav"),
                                      ("coreav-audio", ["--example", "flac_identity"], "examples/flac_identity")):
        subprocess.run(["cargo", "build", "--locked", "--release", "-p", crate, *extra],
                       cwd=source, env=env, check=True, timeout=1200)
        shutil.copy2(source / "target/release" / executable, output / Path(executable).name)
    if text(["git", "status", "--porcelain"]):
        raise RuntimeError("profiling build changed source")
    archive = subprocess.check_output(["git", "archive", "HEAD"], cwd=source, timeout=60)
    (output / "source.tar.gz").write_bytes(gzip.compress(archive, mtime=0))
    manifest = {"merged_source": merged, "merged_tree": text(["git", "rev-parse", "HEAD^{tree}"]),
                "encoder_sha256": checksum, "rustc": text(["rustc", "-vV"]), "cargo": text(["cargo", "-vV"]),
                "profile": {"mode": "release", "locked": True, "RUSTFLAGS": flags,
                            "CARGO_INCREMENTAL": "0", "CARGO_PROFILE_RELEASE_STRIP": "false"},
                "tests": counts, "tool_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                "files": {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                          for path in sorted(output.iterdir()) if path.is_file()}}
    (output / "profile-build.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
