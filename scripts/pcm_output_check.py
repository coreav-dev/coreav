"""Compare native PCM output with the pinned FFmpeg on generated samples."""

import argparse
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))

from oracle_audio import INPUT_ARGS as AUDIO_DECODE_INPUT
import test_oracle


FIELDS = (
    "codec_name", "sample_fmt", "sample_rate", "channels", "channel_layout",
    "bits_per_sample", "time_base", "duration_ts", "duration",
)


def run(args, *, env=None):
    result = subprocess.run([str(arg) for arg in args], capture_output=True, timeout=30, env=env)
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {args}\n{result.stderr.decode(errors='replace')}")
    return result.stdout


def fields(probe, path, raw, rate, channels):
    options = ["-f", "s16le", "-sample_rate", str(rate), "-ch_layout",
               {1: "mono", 2: "stereo", 6: "5.1"}[channels]] if raw else []
    data = json.loads(run([probe, "-v", "error", *options, "-show_streams", "-show_format", "-of", "json", path]))
    [stream] = data["streams"]
    return {"stream": {key: stream.get(key) for key in FIELDS},
            "tags": {key.lower(): value for key, value in data["format"].get("tags", {}).items()
                     if key.lower() != "encoder"}}


def native_command(coreav, input_path, output, before=(), after=()):
    return [str(coreav), "ffmpeg", *before, "-i", str(input_path), *after, str(output)]


def check(coreav, oracle, root):
    ffmpeg, ffprobe = oracle.ffmpeg, oracle.ffprobe
    env = {**os.environ, "COREAV_STRICT_SAFE": "1", "COREAV_FALLBACK": str(root / "absent-ffmpeg")}
    outcomes = []

    def compare(case, number, input_path, before, after, kind, expected, rate, channels):
        ours = root / f"ours-{case}-{number}.{kind}"
        theirs = root / f"oracle-{case}-{number}.{kind}"
        stdout = run(native_command(coreav, input_path, ours, before, after), env=env)
        if stdout:
            raise AssertionError(f"unexpected standard output: {case}/{number}")
        run([ffmpeg, "-v", "error", *before, *AUDIO_DECODE_INPUT, "-i", input_path, *after, theirs])
        raw = kind == "raw"
        ours_fields = fields(ffprobe, ours, raw, rate, channels)
        theirs_fields = fields(ffprobe, theirs, raw, rate, channels)
        if ours_fields != theirs_fields:
            raise AssertionError(f"fields differ: {case}/{number}: {ours_fields} != {theirs_fields}")
        for output in (ours, theirs):
            decoded = output.read_bytes() if raw else run([
                ffmpeg, "-v", "error", *AUDIO_DECODE_INPUT, "-i", output,
                "-f", "s16le", "-c:a", "pcm_s16le", "-",
            ])
            if decoded != expected:
                raise AssertionError(f"sample mismatch: {case}/{number}/{output.name}")
        outcomes.append({"case": case, "command": number, "samples": len(expected) // (channels * 2),
                         "fields": ours_fields, "result": "equal"})

    for channels, rate in [(1, 16000), (2, 44100), (6, 48000)]:
        source = root / f"source-{channels}-{rate}.s16le"
        samples = b"".join(struct.pack("<h", ((i * 101 + channel * 7919) % 65536) - 32768)
                           for i in range(4096) for channel in range(channels))
        source.write_bytes(samples)
        for container in ("mka", "mp4"):
            case = f"{channels}-{rate}-{container}"
            input_path = root / f"input-{case}.{container}"
            run([ffmpeg, "-v", "error", "-f", "s16le", "-ar", str(rate), "-ac", str(channels),
                 *AUDIO_DECODE_INPUT, "-i", source,
                 "-f", "lavfi", "-i", "color=size=16x16:duration=0.04",
                 "-map", "1:v:0", "-map", "0:a:0", "-c:v", "libx264",
                 "-c:a", "flac", "-strict", "-2", input_path])
            commands = [
                ([], [], "wav", 4096),
                ([], ["-acodec", "pcm_s16le", "-f", "s16le"], "raw", 4096),
                (["-ss", "0", "-t", "30"], [], "wav", 4096),
                ([], ["-ss", "0", "-t", "37"], "wav", 4096),
                ([], ["-ss", "0", "-t", "0.01"], "wav", rate // 100),
                ([], ["-ar", str(rate), *(["-ac", str(channels)] if channels <= 2 else []),
                      "-c:a", "pcm_s16le"], "wav", 4096),
                ([], ["-map", "0:a:0", "-c:0", "pcm_s16le", "-f", "wav"], "wav", 4096),
                (["-t", "0"], [], "wav", 4096),
                (["-t", "0.01"], [], "wav", rate // 100),
                (["-to", "0.01"], [], "wav", rate // 100),
                ([], ["-to", "0.01"], "wav", rate // 100),
                (["-to", "0", "-t", "0"], [], "wav", 4096),
                ([], ["-t", "0"], "wav", 0),
                ([], ["-threads", "0"], "wav", 4096),
                ([], ["-threads", "4"], "wav", 4096),
                ([], ["-threads", "auto", "-f", "s16le"], "raw", 4096),
            ]
            for number, (before, after, kind, count) in enumerate(commands):
                expected = samples[:count * channels * 2]
                compare(case, number, input_path, before, after, kind, expected, rate, channels)

            for before, after in [(["-to", "0"], []), ([], ["-to", "0"])]:
                output = root / f"zero-end-{case}.wav"
                refused = subprocess.run(native_command(coreav, input_path, output, before, after),
                                         capture_output=True, timeout=30, env=env)
                if refused.returncode != 2 or output.exists():
                    raise AssertionError("an effective zero -to must fail without an output")

    mixed = root / "mixed.mka"
    run([ffmpeg, "-v", "error", *AUDIO_DECODE_INPUT, "-i", root / "input-2-44100-mka.mka",
         *AUDIO_DECODE_INPUT, "-i", root / "input-1-16000-mka.mka",
         "-map", "0:a:0", "-map", "1:a:0", "-c", "copy",
         "-disposition:a:0", "0", "-disposition:a:1", "default", mixed])
    for number, (options, channels, rate) in enumerate([
        ([], 1, 16000),
        (["-map", "0:a:0", "-c:0", "pcm_s16le"], 2, 44100),
        (["-map", "0", "-map", "-0:a:0"], 1, 16000),
    ]):
        expected = (root / f"source-{channels}-{rate}.s16le").read_bytes()
        compare("selection", number, mixed, [], options, "wav", expected, rate, channels)

    tagged = root / "tagged.mka"
    run([ffmpeg, "-v", "error", *AUDIO_DECODE_INPUT, "-i", mixed,
         "-map", "0:a:0", "-c", "copy", "-metadata", "title=PCM check", tagged])
    refused = subprocess.run(native_command(coreav, tagged, root / "refused.wav"),
                             capture_output=True, timeout=30, env=env)
    if refused.returncode != 4 or (root / "refused.wav").exists():
        raise AssertionError("default WAV metadata must be refused without an output")
    expected = (root / "source-2-44100.s16le").read_bytes()
    compare("metadata", 0, tagged, [], ["-map_metadata", "-1"], "wav", expected, 44100, 2)
    compare("metadata", 1, tagged, [], ["-f", "s16le"], "raw", expected, 44100, 2)
    for number, (kind, form) in enumerate((("wav", "wav"), ("raw", "s16le"))):
        for indexed in (False, True):
            options = ["-c:a:1", "pcm_u8", "-c:a:0", "pcm_s16le"] if indexed else [
                "-c:a", "pcm_u8", "-c:a", "pcm_s16le",
            ]
            compare("effective-codec", number * 2 + int(indexed), root / "input-2-44100-mka.mka",
                    [], [*options, "-f", form], kind, expected, 44100, 2)
    subtitles = root / "subtitles.srt"
    subtitles.write_text("1\n00:00:00,000 --> 00:00:00,020\nPCM subtitle check\n")
    subtitled = root / "subtitled.mka"
    run([ffmpeg, "-v", "error", "-i", root / "input-2-44100-mka.mka", "-i", subtitles,
         "-map", "0:a:0", "-map", "1:s:0", "-c", "copy", subtitled])
    for number, (kind, form) in enumerate((("wav", "wav"), ("raw", "s16le"))):
        output = root / f"subtitles-refused-{kind}.output"
        output.write_bytes(b"original")
        options = ["-y", "-c:s", "copy", "-f", form]
        reference = subprocess.run([ffmpeg, "-v", "error", *AUDIO_DECODE_INPUT, "-i", subtitled,
                                    *options, root / f"subtitles-reference-{kind}.output"],
                                   capture_output=True, timeout=30)
        if reference.returncode == 0 or b"stream of type subtitle" not in reference.stderr:
            raise AssertionError("the pinned oracle must reject subtitles in PCM output")
        refused = subprocess.run(native_command(coreav, subtitled, output, after=options),
                                 capture_output=True, timeout=30, env=env)
        if refused.returncode != 4 or refused.stdout or output.read_bytes() != b"original":
            raise AssertionError("an explicit subtitle codec must not silently drop subtitles")
        for disabled in (False, True):
            after = ["-c:s", "copy", "-sn"] if disabled else []
            compare("subtitles", number * 2 + int(disabled), subtitled, [], [*after, "-f", form],
                    kind, expected, 44100, 2)
    for encoder in ("libmp3lame", "libopus", "aac"):
        lossy = root / f"lossy-{encoder}.mka"
        run([ffmpeg, "-v", "error", *AUDIO_DECODE_INPUT, "-i", root / "input-2-44100-mka.mka",
             "-map", "0:a:0", "-c:a", encoder, lossy])
        for kind in ("wav", "s16le"):
            output = root / f"lossy-{encoder}-{kind}.output"
            output.write_bytes(b"original")
            refused = subprocess.run(native_command(coreav, lossy, output, after=[
                "-y", "-f", kind, "-acodec", "pcm_s16le",
            ]), capture_output=True, timeout=30, env=env)
            if refused.returncode != 4 or refused.stdout or output.read_bytes() != b"original":
                raise AssertionError(f"lossy PCM source must retain its refusal: {encoder}/{kind}")
    return outcomes


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--coreav", type=Path, required=True)
    parser.add_argument("--oracle-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    try:
        oracle = test_oracle.require(args.oracle_dir / "ffmpeg", args.oracle_dir / "ffprobe")
    except test_oracle.QualificationError as error:
        parser.error(str(error))
    with tempfile.TemporaryDirectory(prefix="coreav-pcm-") as directory:
        outcomes = check(args.coreav.resolve(), oracle, Path(directory))
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps({"outcomes": outcomes}, indent=2) + "\n")
    print(f"PCM output: {len(outcomes)} commands matched samples and ffprobe fields")


if __name__ == "__main__":
    main()
