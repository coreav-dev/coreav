#!/usr/bin/env python3
"""Compare dispatched and scalar-forced encoder drivers on generated pictures."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def compare_outputs(auto, scalar, expected_samples):
    result = {}
    for suffix in ("h264", "recon", "decoded"):
        a = auto.with_suffix("." + suffix).read_bytes()
        b = scalar.with_suffix("." + suffix).read_bytes()
        if not a or a != b:
            raise ValueError("dispatched and scalar outputs differ")
        if suffix != "h264" and len(a) != expected_samples:
            raise ValueError("incomplete reconstruction")
        result[suffix + "_sha256"] = hashlib.sha256(a).hexdigest()
    if result["recon_sha256"] != result["decoded_sha256"]:
        raise ValueError("native decoder differs from reconstruction")
    return result


def pictures(width, height, frames, pattern):
    out = bytearray()
    for frame in range(frames):
        for scale in (1, 2, 2):
            for y in range(height // scale):
                for x in range(width // scale):
                    if pattern == "edge":
                        value = 40 + 160 * (((x + frame * 3) // 12 + y // 12) % 2)
                    else:
                        value = 90 + (x + frame * 3) % 32 + y % 24
                    out.append(value)
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--auto-driver", type=Path, required=True)
    parser.add_argument("--scalar-driver", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    rows = []
    frames = 4
    for width, height in ((32, 32), (80, 48)):
        for pattern in ("ramp", "edge"):
            raw = args.output / f"{width}x{height}-{pattern}.yuv"
            raw.write_bytes(pictures(width, height, frames, pattern))
            for qp in (10, 28, 51):
                for deblock in ("false", "true"):
                    for interval in (1, 3):
                        case = args.output / f"{width}x{height}-{pattern}-{qp}-{deblock}-{interval}"
                        case.mkdir(exist_ok=True)
                        for name, driver in (("auto", args.auto_driver), ("scalar", args.scalar_driver)):
                            prefix = case / name
                            for suffix in ("h264", "recon", "decoded"):
                                prefix.with_suffix("." + suffix).unlink(missing_ok=True)
                            command = [str(driver.resolve()), str(width), str(height), str(qp), str(frames), str(raw.resolve()), str(prefix.with_suffix(".h264").resolve()), str(prefix.with_suffix(".recon").resolve()), str(prefix.with_suffix(".decoded").resolve()), deblock, str(interval)]
                            result = subprocess.run(command, capture_output=True, timeout=120)
                            if result.returncode or result.stderr:
                                raise ValueError("encoder driver failed or emitted diagnostics")
                        hashes = compare_outputs(case / "auto", case / "scalar", width * height * 3 // 2 * frames)
                        rows.append({"width": width, "height": height, "pattern": pattern, "qp": qp, "deblocking": deblock == "true", "interval": interval, "frames": frames, **hashes})
    (args.output / "summary.json").write_text(json.dumps({"cases": rows}, indent=2) + "\n")
    print(f"SIMD identity: {len(rows)} cases, {len(rows) * frames} pictures, exact streams and reconstruction")


if __name__ == "__main__":
    main()
