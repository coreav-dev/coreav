"""Compare complete native raw-audio outputs with the pinned oracle."""

import argparse
import os
from pathlib import Path
import platform
import struct
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT


def run(args, *, data=None, env=None):
    result = subprocess.run([str(arg) for arg in args], input=data, capture_output=True,
                            env=env, timeout=30)
    if result.returncode:
        raise AssertionError(f"command failed ({result.returncode}): {args}\n"
                             f"{result.stderr.decode(errors='replace')}")
    return result.stdout


def require_equal(actual, expected, case):
    if actual != expected:
        raise AssertionError(f"complete raw bytes differ: {case}")


def native_command(coreav, before, input_path, after, output):
    return [str(coreav), "ffmpeg", *before, "-i", str(input_path), *after, str(output)]


def check(coreav, oracle, root, smoke=False):
    ffmpeg = oracle / "ffmpeg"
    env = {**os.environ, "COREAV_STRICT_SAFE": "1",
           "COREAV_FALLBACK": str(root / "missing-fallback")}
    count = 0
    for bits, channels, rate in [(16, 1, 44100), (16, 2, 48000), (24, 1, 48000), (24, 2, 44100)]:
        values = [((i * 7919 + c * 11717) % (1 << bits)) - (1 << (bits - 1))
                  for i in range(4097) for c in range(channels)]
        values[:6] = [-(1 << (bits - 1)), (1 << (bits - 1)) - 1, -1, 0, 1, 3]
        samples = b"".join(value.to_bytes(bits // 8, "little", signed=True) for value in values)
        source = root / f"input-{bits}-{channels}.s{bits}"
        source.write_bytes(samples)
        raw_options = ["-f", f"s{bits}le", "-ar", str(rate), "-ac", str(channels)]
        inputs = [(source, raw_options)] if bits == 16 else []
        for container in ("mka", "mp4"):
            compressed = root / f"input-{bits}-{channels}.{container}"
            run([ffmpeg, "-v", "error", *raw_options, *AUDIO_DECODE_INPUT, "-i", source,
                 "-c:a", "flac", "-strict", "-2", compressed])
            inputs.append((compressed, []))
        for input_path, before in inputs:
            for output_format in ("s16le", "f32le"):
                after = ["-ac", str(channels), "-ar", str(rate),
                         "-c:a", f"pcm_{output_format}", "-f", output_format]
                expected = run([ffmpeg, "-v", "error", *before, *AUDIO_DECODE_INPUT,
                                "-i", input_path, *after, "pipe:"])
                packed = (b"".join(struct.pack("<h", value >> (bits - 16)) for value in values)
                          if output_format == "s16le" else
                          b"".join(struct.pack("<f", value / (1 << (bits - 1))) for value in values))
                require_equal(expected, packed, "oracle identity against original samples")
                modes = [(str(input_path), None), ("pipe:", input_path.read_bytes())]
                if smoke:
                    modes = modes[-1:]
                for input_name, data in modes:
                    actual = run(native_command(coreav, ["-threads", "2", *before], input_name,
                                                after, "pipe:"), data=data, env=env)
                    require_equal(actual, expected, f"{input_path.name}/{output_format}/{input_name}")
                    count += 1
                output = root / "out.raw"
                run(native_command(coreav, ["-y", *before], input_path, after, output), env=env)
                require_equal(output.read_bytes(), expected, "file output")
                count += 1
            rate_formats = ["s16le"] if bits == 16 else []
            if channels == 1 and not before:
                rate_formats = ["s16le", "f32le"]
            for output_format in rate_formats if not smoke else []:
                after = ["-ar", "16000", "-ac", str(channels), "-f", output_format]
                expected = run([ffmpeg, "-v", "error", *before, *AUDIO_DECODE_INPUT,
                                "-i", input_path, *after, "pipe:"])
                command = native_command(coreav, before, "pipe:", after, "pipe:")
                if (sys.platform, platform.machine()) != ("linux", "x86_64"):
                    refused = subprocess.run(command, input=input_path.read_bytes(),
                                             capture_output=True, env=env, timeout=30)
                    if refused.returncode != 4 or refused.stdout:
                        raise AssertionError("unregistered platform rate stage did not refuse cleanly")
                else:
                    actual = run(command, data=input_path.read_bytes(), env=env)
                    require_equal(actual, expected, f"S16 rate/{input_path.name}")
                count += 1
    return count


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coreav", type=Path, required=True)
    parser.add_argument("--oracle-bin", type=Path, required=True)
    parser.add_argument("--smoke", action="store_true")
    args = parser.parse_args()
    if sys.platform == "darwin":
        from oracle_pin_macos import verify_bin
    else:
        from oracle_pin import verify_bin
    problems = list(verify_bin(args.oracle_bin))
    if problems:
        raise SystemExit("\n".join(problems))
    with tempfile.TemporaryDirectory(prefix="coreav-raw-audio-") as directory:
        count = check(args.coreav.resolve(), args.oracle_bin.resolve(), Path(directory), args.smoke)
    print(f"raw audio pipes: {count} exact-output and eligibility checks passed")


if __name__ == "__main__":
    main()
