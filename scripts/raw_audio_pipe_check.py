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


def wav_samples(path):
    data = path.read_bytes()
    if data[:4] != b"RIFF" or data[8:12] != b"WAVE":
        raise AssertionError("native decoder reference is not WAV")
    offset = 12
    while offset + 8 <= len(data):
        size = int.from_bytes(data[offset + 4:offset + 8], "little")
        end = offset + 8 + size
        if end > len(data):
            raise AssertionError("native decoder reference has a truncated chunk")
        if data[offset:offset + 4] == b"data":
            return data[offset + 8:end]
        offset = end + size % 2
    raise AssertionError("native decoder reference has no samples")


def require_refusal(command, env, data=None):
    refused = subprocess.run(command, input=data, capture_output=True, env=env, timeout=30)
    if refused.returncode != 4 or refused.stdout:
        raise AssertionError(f"unregistered path did not refuse cleanly: {command}")


def conformance_paths(coreav, oracle, root):
    ffmpeg = oracle / "ffmpeg"
    env = {**os.environ, "COREAV_STRICT_SAFE": "1",
           "COREAV_FALLBACK": str(root / "missing-fallback")}
    fallback = root / "ffmpeg"
    fallback.write_text("#!/bin/sh\nprintf '%s\\n' \"$@\"\ncat\nexit 7\n", encoding="utf-8")
    fallback.chmod(0o700)
    replay_env = {**env, "COREAV_FALLBACK": str(fallback)}
    replay_env.pop("COREAV_STRICT_SAFE", None)
    count = 0
    for channels, rate in [(1, 44100), (2, 48000)]:
        samples = b"".join(struct.pack("<h", (i * 7919 % 50001) - 25000)
                           for i in range(8192 * channels))
        source = root / f"mp3-{channels}.s16"
        source.write_bytes(samples)
        encoded = root / f"mp3-{channels}.mp4"
        run([ffmpeg, "-v", "error", "-f", "s16le", "-ar", str(rate), "-ac", str(channels),
             *AUDIO_DECODE_INPUT, "-i", source, "-c:a", "libmp3lame", encoded])
        for form, width in [("f32le", 4), ("s16le", 2)]:
            reference = root / f"native-{channels}-{form}.wav"
            run([coreav, "decode-audio", "--threads", "2", "--format", form[:3],
                 encoded, reference], env=env)
            expected = wav_samples(reference)
            if len(expected) != 8192 * channels * width:
                raise AssertionError("MP3 presentation did not preserve its finite edit count")
            after = ["-f", form]
            for input_path, data in [(encoded, None), ("pipe:", encoded.read_bytes())]:
                actual = run(native_command(coreav, ["-threads", "2"], input_path, after, "pipe:"),
                             data=data, env=env)
                require_equal(actual, expected, "MP3 packing against native conformance decoder")
                count += 1
            output = root / f"mp3-{channels}-{form}.raw"
            run(native_command(coreav, [], encoded, after, output), env=env)
            require_equal(output.read_bytes(), expected, "MP3 native file packing")
            count += 1
            command = native_command(coreav, [], encoded, ["-ar", "16000", *after], "pipe:")
            if channels == 1 and (sys.platform, platform.machine()) == ("linux", "x86_64"):
                floats = root / "native-floats.raw"
                floats.write_bytes(wav_samples(root / "native-1-f32le.wav"))
                converted = run([ffmpeg, "-v", "error", "-f", "f32le", "-ar", str(rate),
                                 "-ac", "1", *AUDIO_DECODE_INPUT, "-i", floats,
                                 "-ar", "16000", *after, "pipe:"])
                require_equal(run(command, env=env), converted,
                              "MP3 exact rate stage over identical native decoded floats")
            else:
                require_refusal(command, env)
            count += 1
        require_refusal(native_command(coreav, [], encoded, ["-ac", str(3 - channels),
                                                            "-f", "f32le"], "pipe:"), env)
        count += 1
    for encoder, rate, containers in [("libmp3lame", 22050, ["mp4"]),
                                       ("libmp3lame", 44100, ["mka"]),
                                       ("aac", 44100, ["mp4", "mka"]),
                                       ("libopus", 48000, ["mp4", "mka"])]:
        for container in containers:
            encoded = root / f"refused-{encoder}-{rate}.{container}"
            run([ffmpeg, "-v", "error", "-f", "s16le", "-ar", str(rate), "-ac", "1",
                 *AUDIO_DECODE_INPUT, "-i", root / "mp3-1.s16", "-c:a", encoder, encoded])
            for form in ["f32le", "s16le"]:
                command = native_command(coreav, [], "pipe:", ["-f", form], "pipe:")
                require_refusal(command, env, encoded.read_bytes())
                count += 1
                replay = subprocess.run(command, input=encoded.read_bytes(), capture_output=True,
                                        env=replay_env, timeout=30)
                expected = ("\n".join(command[2:]) + "\n").encode() + encoded.read_bytes()
                if replay.returncode != 7 or replay.stdout != expected:
                    raise AssertionError(f"decoder replay failed ({replay.returncode}): "
                                         f"{replay.stderr[-2000:].decode(errors='replace')}")
                count += 1
    return count


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
                command = native_command(coreav, ["-threads", "1", *before], "pipe:", after, "pipe:")
                if (sys.platform, platform.machine()) != ("linux", "x86_64"):
                    refused = subprocess.run(command, input=input_path.read_bytes(),
                                             capture_output=True, env=env, timeout=30)
                    if (refused.returncode != 4 or refused.stdout
                            or b"unregistered exact raw" not in refused.stderr
                            or b"rate conversion" not in refused.stderr):
                        raise AssertionError("unregistered platform rate stage did not refuse cleanly")
                    fallback_env = {**env, "COREAV_FALLBACK": str(ffmpeg)}
                    fallback_env.pop("COREAV_STRICT_SAFE", None)
                    for input_name, data in [(input_path, None), ("pipe:", input_path.read_bytes())]:
                        fallback = native_command(coreav, ["-threads", "1", *before],
                                                  input_name, after, "pipe:")
                        actual = run(fallback, data=data, env=fallback_env)
                        require_equal(actual, expected, "unregistered platform oracle fallback")
                        count += 1
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
        conformance = conformance_paths(args.coreav.resolve(), args.oracle_bin.resolve(),
                                        Path(directory))
    print(f"raw audio pipes: {count} exact-output, eligibility and fallback checks passed")
    print(f"raw audio pipes: {conformance} conformance-path and refusal checks passed")


if __name__ == "__main__":
    main()
