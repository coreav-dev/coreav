"""Check native FLAC samples, STREAMINFO and probe fields against the pinned oracle."""

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))

from oracle_audio import INPUT_ARGS as AUDIO_INPUT
from oracle_pin import verify_bin


FIELDS = ("codec_name", "sample_fmt", "sample_rate", "channels", "channel_layout",
          "bits_per_sample", "bits_per_raw_sample", "time_base", "duration_ts", "duration")
LAYOUTS = {1: "mono", 2: "stereo", 3: "3.0", 4: "quad", 5: "5.0",
           6: "5.1", 7: "6.1", 8: "7.1"}


def run(args, *, env=None):
    result = subprocess.run([str(arg) for arg in args], capture_output=True, timeout=60, env=env)
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {args}\n{result.stderr.decode(errors='replace')[-4000:]}")
    return result.stdout


def pcm(bits, channels, frames, kind):
    result = bytearray()
    bound = 1 << (bits - 1)
    random = 7
    for frame in range(frames):
        for channel in range(channels):
            random = (random * 1664525 + 1013904223) & 0xFFFFFFFF
            value = round(bound * (0.61 * math.sin(frame * 0.13) + 0.23 * math.sin(frame * 0.31 + channel * 0.07))) if kind == "tones" else {
                "zero": 0,
                "constant": -bound,
                "ramp": ((frame * 17 + channel * 11) % (2 * bound)) - bound,
                "extremes": -bound if frame % 2 else bound - 1,
                "impulse": bound - 1 if frame % 71 == 0 else 0,
                "noise": (random >> (32 - bits)) - bound,
            }[kind]
            result.extend(value.to_bytes(bits // 8, "little", signed=True))
    return bytes(result)


def probe(program, path):
    report = json.loads(run([program, "-v", "error", "-show_streams", "-show_format", "-of", "json", path]))
    [stream] = report["streams"]
    return {"stream": {key: stream.get(key) for key in FIELDS},
            "tags": {key.lower(): value for key, value in report["format"].get("tags", {}).items()
                     if key.lower() != "encoder"}}


def make_source(ffmpeg, root, bits, channels, rate, samples, label, container="mka"):
    raw = root / f"{label}.raw"
    raw.write_bytes(samples)
    source = root / f"{label}.{container}"
    run([ffmpeg, "-v", "error", "-f", f"s{bits}le", "-sample_rate", str(rate),
         "-ch_layout", LAYOUTS[channels], *AUDIO_INPUT, "-i", raw,
         "-c:a", "flac", "-strict", "-2", source])
    return source


def check(coreav, oracle, root):
    ffmpeg, ffprobe = oracle / "ffmpeg", oracle / "ffprobe"
    env = {**os.environ, "COREAV_STRICT_SAFE": "1", "COREAV_FALLBACK": str(root / "missing-encoder")}
    outcomes = []

    def compare(source, samples, bits, channels, label, options=(), before=()):
        ours, reference = (root / f"{label}-{name}.flac" for name in ("native", "reference"))
        stdout = run([coreav, "ffmpeg", *before, "-i", source, *options, ours], env=env)
        if stdout:
            raise AssertionError("native FLAC output printed standard output")
        run([ffmpeg, "-v", "error", *before, *AUDIO_INPUT, "-i", source, *options, reference])
        fields = probe(ffprobe, ours)
        expected_fields = probe(ffprobe, reference)
        if fields != expected_fields:
            raise AssertionError(f"probe mismatch {label}: {fields} != {expected_fields}")
        for path in (ours, reference):
            decoded = run([ffmpeg, "-v", "error", *AUDIO_INPUT, "-i", path,
                           "-c:a", f"pcm_s{bits}le", "-f", f"s{bits}le", "-"])
            if decoded != samples:
                raise AssertionError(f"decoded sample mismatch: {label}/{path.name}")
        encoded = ours.read_bytes()
        if encoded[:8] != b"fLaC\x80\0\0\x22" or encoded[26:42] != hashlib.md5(samples).digest():
            raise AssertionError(f"STREAMINFO MD5 mismatch: {label}")
        count = int.from_bytes(encoded[18:26], "big") & ((1 << 36) - 1)
        if count != len(samples) // (channels * bits // 8):
            raise AssertionError(f"STREAMINFO sample count mismatch: {label}")
        outcomes.append({"case": label, "samples": count, "fields": fields, "result": "equal"})

    kinds = ("zero", "constant", "ramp", "extremes", "impulse", "noise")
    for bits in (16, 24):
        for channels in range(1, 9):
            frames = (1, 15, 16, 4095, 4096, 4097, 8243, 4097)[channels - 1]
            rate = (8000, 44100, 48000, 96000, 12345)[channels % 5]
            samples = pcm(bits, channels, frames, kinds[channels % len(kinds)])
            label = f"{bits}-{channels}-{rate}"
            source = make_source(ffmpeg, root, bits, channels, rate, samples, label)
            compare(source, samples, bits, channels, label)

    for bits in (16, 24):
        tone = pcm(bits, 2, 8243, "tones")
        tone_source = make_source(ffmpeg, root, bits, 2, 48000, tone, f"tones-{bits}")
        compare(tone_source, tone, bits, 2, f"tones-{bits}")
        samples = pcm(bits, 2, 8243, "ramp")
        source = make_source(ffmpeg, root, bits, 2, 48000, samples, f"options-{bits}")
        for number, options in enumerate([
            ["-c:a", "flac"], ["-f", "flac"], ["-map", "0:a:0", "-c:0", "flac"],
            ["-c:a", "pcm_u8", "-c:a", "flac"], ["-c:a:1", "pcm_u8", "-c:a:0", "flac"],
            ["-threads", "auto"], ["-ar", "48000", "-ac", "2"], ["-ss", "0"],
            ["-map_metadata", "-1"],
        ]):
            compare(source, samples, bits, 2, f"options-{bits}-{number}", options)
        compare(source, samples[:480 * 2 * (bits // 8)], bits, 2, f"cap-{bits}", ["-t", "0.01"])
        compare(source, b"", bits, 2, f"empty-{bits}", ["-t", "0"])
        compare(source, samples, bits, 2, f"zero-input-{bits}", before=["-t", "0"])
        compare(source, samples, bits, 2, f"seek-input-{bits}", before=["-ss", "0"])
        mp4 = make_source(ffmpeg, root, bits, 2, 48000, samples, f"mp4-{bits}", "mp4")
        compare(mp4, samples, bits, 2, f"mp4-{bits}")

        for number, options in enumerate([
            ["-ss", "1"], ["-ar", "44100"], ["-ac", "1"], ["-b:a", "128k"],
            ["-c:a", "pcm_s16le"], ["-c:a", "copy"], ["-map", "0:a", "-map", "0:a"],
        ]):
            output = root / f"refusal-{bits}-{number}.flac"
            output.write_bytes(b"original")
            result = subprocess.run([str(coreav), "ffmpeg", "-i", str(source), "-y", *options, str(output)],
                                    capture_output=True, timeout=60, env=env)
            if result.returncode != 4 or result.stdout or output.read_bytes() != b"original":
                raise AssertionError(f"unsupported FLAC command replaced its destination: {options}")

    samples = pcm(16, 2, 8243, "ramp")
    source = root / "options-16.mka"
    tagged = root / "tagged.mka"
    run([ffmpeg, "-v", "error", *AUDIO_INPUT, "-i", source,
         "-c", "copy", "-metadata", "title=FLAC check", tagged])
    stream_tagged = root / "stream-tagged.mka"
    run([ffmpeg, "-v", "error", *AUDIO_INPUT, "-i", source,
         "-c", "copy", "-metadata:s:a:0", "title=FLAC stream check", stream_tagged])
    video = root / "video.mkv"
    run([ffmpeg, "-v", "error", *AUDIO_INPUT, "-i", source,
         "-f", "lavfi", "-i", "color=size=16x16:duration=0.04",
         "-map", "0:a", "-map", "1:v", "-c:a", "copy", "-c:v", "libx264", video])
    for number, (input_path, options) in enumerate([
        (tagged, []), (stream_tagged, []), (video, []),
    ]):
        output = root / f"preserve-{number}.flac"
        output.write_bytes(b"original")
        result = subprocess.run([str(coreav), "ffmpeg", "-i", str(input_path), "-y", *options, str(output)],
                                capture_output=True, timeout=60, env=env)
        if result.returncode != 4 or result.stdout or output.read_bytes() != b"original":
            raise AssertionError("FLAC metadata or cover refusal changed the destination")
    compare(tagged, samples, 16, 2, "strip-tags", ["-map_metadata", "-1"])
    compare(stream_tagged, samples, 16, 2, "strip-stream-tags", ["-map_metadata", "-1"])
    compare(video, samples, 16, 2, "omit-cover", ["-vn"])

    for encoder in ("libmp3lame", "libopus", "aac"):
        lossy = root / f"lossy-{encoder}.mka"
        run([ffmpeg, "-v", "error", *AUDIO_INPUT, "-i", source, "-c:a", encoder, lossy])
        output = root / f"lossy-{encoder}.flac"
        output.write_bytes(b"original")
        result = subprocess.run([str(coreav), "ffmpeg", "-i", str(lossy), "-y", str(output)],
                                capture_output=True, timeout=60, env=env)
        if result.returncode != 4 or result.stdout or output.read_bytes() != b"original":
            raise AssertionError("lossy-to-FLAC source changed its refusal or destination")

    return outcomes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coreav", type=Path, required=True)
    parser.add_argument("--oracle-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    problems = list(verify_bin(args.oracle_dir))
    if problems:
        parser.error("; ".join(problems))
    with tempfile.TemporaryDirectory(prefix="coreav-flac-") as directory:
        outcomes = check(args.coreav.resolve(), args.oracle_dir.resolve(), Path(directory))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps({"outcomes": outcomes}, indent=2) + "\n")
    print(f"FLAC output: {len(outcomes)} commands matched complete samples, MD5 and probe fields")


if __name__ == "__main__":
    main()
